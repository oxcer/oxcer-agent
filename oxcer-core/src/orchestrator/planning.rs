//! Plan building: deterministic heuristics that translate a natural-language task
//! into a concrete `Vec<ToolCallIntent>` before any tool is executed.
//!
//! All public items in this module are re-exported from `orchestrator::mod` so
//! callers use `orchestrator::start_session`, not `orchestrator::planning::start_session`.

use crate::prompt_sanitizer::sanitize_task_for_llm;
use crate::semantic_router::{
    has_implicit_file_read_intent, has_implicit_fs_intent, route, Strategy,
};

use super::types::{
    format_tool_call, ExpansionKind, SessionKind, SessionState, TaskState, ToolCallIntent,
};

// -----------------------------------------------------------------------------
// Plan building (heuristic for Sprint 6; LLM planner can be added later)
// -----------------------------------------------------------------------------

/// Deterministic planner for ToolsOnly: no LLM; single FS/Shell commands.
/// All commands go through Security Policy Engine and approval flow (delete/rename/move).
fn build_plan_tools_only(
    task: &str,
    context: &crate::semantic_router::TaskContext,
    default_workspace_id: Option<&str>,
    default_workspace_root: Option<&str>,
) -> Vec<ToolCallIntent> {
    let task_lower = task.to_lowercase();
    let ws_id = context
        .workspace_id
        .as_deref()
        .or(default_workspace_id)
        .unwrap_or("")
        .to_string();
    let ws_root = default_workspace_root.unwrap_or("").to_string();
    if ws_root.is_empty() {
        return Vec::new();
    }

    let mut intents = Vec::new();

    // "list files (in workspace)" / "list dir" / "ls" -> single FsListDir
    if task_lower.contains("list files")
        || task_lower.contains("list dir")
        || task_lower.contains("list the files")
        || (task_lower.contains("list")
            && (task_lower.contains("file") || task_lower.contains("dir")))
        || task_lower.trim() == "ls"
    {
        intents.push(ToolCallIntent::FsListDir {
            workspace_id: ws_id.clone(),
            workspace_root: ws_root.clone(),
            rel_path: ".".to_string(),
        });
        return intents;
    }

    // "delete X" / "remove X" -> single FsDelete (always goes through policy + approval)
    let delete_prefixes = ["delete ", "remove ", "rm "];
    for prefix in delete_prefixes {
        if task_lower.starts_with(prefix) || task_lower.contains(&format!(" {} ", prefix.trim())) {
            let rest = task_lower
                .strip_prefix(prefix)
                .or_else(|| task_lower.split(prefix).nth(1))
                .unwrap_or("")
                .trim();
            let path = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c: char| c == '"' || c == '\'');
            if !path.is_empty() {
                intents.push(ToolCallIntent::FsDelete {
                    workspace_id: ws_id,
                    workspace_root: ws_root,
                    rel_path: path.to_string(),
                });
                return intents;
            }
        }
    }

    intents
}

/// System hint for `SessionKind::Chat` sessions (pure Q&A, no tool workflow).
///
/// Contains no tool names and no document-processing framing so the model
/// answers naturally without triggering narration patterns.  The narration
/// sanitizer never runs for `Chat` sessions, so this hint needs no negation
/// rules about tool calls.
pub(crate) const CHAT_SYSTEM_HINT: &str = "\
You are Oxcer, a local AI assistant. \
Answer the user's question clearly and directly. \
Be concise.";

/// System hint injected into every `LlmGenerate` intent for `Task` sessions.
///
/// Deliberately contains **no tool names, no tool-use examples, and no negative
/// rules about narration**.  The sole purpose is to put the model into
/// "prose writer" mode: it has already received all necessary document content
/// inline in the task prompt; its only job is to write the requested output.
///
/// Must stay in sync with `DESKTOP_AGENT_SYSTEM_PROMPT` and
/// `CLOUD_AGENT_SYSTEM_PROMPT` regarding the absence of tool descriptions —
/// those full prompts govern the *planning* phase and must never be forwarded
/// to a `LlmGenerate` call.
pub(crate) const SUMMARIZER_SYSTEM_HINT: &str = "\
You are a writing assistant. You have been given the full text of one or more \
documents. Your only job is to write the requested output using that text. \
Only use information that appears in the provided content. \
Do not mention tools, tool calls, file paths, or system internals. \
Do not describe what you are going to do. \
Write only the requested prose.";

/// Placeholder substituted in `LlmGenerate.task` when the orchestrator has
/// accumulated a real filesystem tool result and needs to inject it into the prompt.
pub(crate) const FS_RESULT_PLACEHOLDER: &str = "{{FS_RESULT}}";

/// Placeholder substituted in `FsReadFile.rel_path` by `next_action` after a
/// `FsListDir` step succeeds — replaced with the most recently modified filename
/// from the `sortedByModified` array in the listing payload.
pub(crate) const MOST_RECENT_FILE_PLACEHOLDER: &str = "{{MOST_RECENT_FILE}}";

/// Placeholder substituted in `LlmGenerate.task` with the joined content of all
/// `FsReadFile` results accumulated in `SessionState.content_accumulator`.
/// Used by multi-file summarize and similar workflows.
pub(crate) const FILE_CONTENTS_PLACEHOLDER: &str = "{{FILE_CONTENTS}}";

fn build_plan_with_llm(task: &str, strategy: Strategy) -> Vec<ToolCallIntent> {
    let task_sanitized = sanitize_task_for_llm(task);
    // Pure chat fallback: no tools in the plan.  `SessionKind::from_plan` will
    // therefore classify this session as `Chat`, and the narration sanitizer
    // and precondition guards will not run for it.
    vec![ToolCallIntent::LlmGenerate {
        strategy,
        task: task_sanitized,
        system_hint: Some(CHAT_SYSTEM_HINT.to_string()),
    }]
}

/// Resolves a concrete filesystem path from a natural-language task string.
///
/// Recognises well-known macOS directory names (Desktop, Documents, Downloads)
/// and the user's home directory.  Returns
/// `(workspace_id, absolute_directory_path, rel_path)` where `rel_path` is always
/// `"."` (list the whole directory).
///
/// Returns `None` when no recognisable path can be extracted and no default
/// workspace root is available — the caller must guard against inventing a path.
fn extract_fs_path(
    task: &str,
    default_workspace_id: Option<&str>,
    default_workspace_root: Option<&str>,
) -> Option<(String, String, String)> {
    let task_lower = task.to_lowercase();
    let home = dirs_next::home_dir();

    // Well-known macOS user directories (checked in order — longer/more-specific first).
    let well_known = [
        ("documents", "Documents"),
        ("downloads", "Downloads"),
        ("desktop", "Desktop"),
    ];

    for (keyword, dir_name) in &well_known {
        if task_lower.contains(keyword) {
            let home_ref = home.as_ref()?;
            let full_path = home_ref.join(dir_name);
            let ws_id = default_workspace_id.unwrap_or("").to_string();
            return Some((
                ws_id,
                full_path.to_string_lossy().into_owned(),
                ".".to_string(),
            ));
        }
    }

    // "home folder" / "home directory" / "~" / bare "home"
    if task_lower.contains("home folder")
        || task_lower.contains("home directory")
        || task_lower.contains("home dir")
        || task_lower.contains("~")
        || task_lower.contains(" home")
    {
        let home_path = home?.to_string_lossy().into_owned();
        let ws_id = default_workspace_id.unwrap_or("").to_string();
        return Some((ws_id, home_path, ".".to_string()));
    }

    // Fall back to default workspace root (if provided and non-empty).
    if let Some(root) = default_workspace_root {
        if !root.is_empty() {
            let ws_id = default_workspace_id.unwrap_or("").to_string();
            return Some((ws_id, root.to_string(), ".".to_string()));
        }
    }

    None
}

/// Builds a two-step plan: first list the filesystem path, then ask the LLM
/// to summarise using the real tool result.
///
/// Step 1: `FsListDir` at the resolved path.
/// Step 2: `LlmGenerate` with `{{FS_RESULT}}` in the prompt — `next_action`
///         substitutes the accumulated listing before emitting the intent.
fn build_plan_fs_then_llm(
    task: &str,
    ws_id: String,
    ws_root: String,
    rel_path: String,
    strategy: Strategy,
) -> Vec<ToolCallIntent> {
    let llm_task = format!(
        "The user asked: \"{task}\"\n\n\
         Here is the actual directory listing returned by the filesystem tool:\n\
         {placeholder}\n\n\
         Using ONLY the information above, provide a concise summary. \
         Do NOT invent or add any file names or content that is not in the tool result.",
        task = sanitize_task_for_llm(task),
        placeholder = FS_RESULT_PLACEHOLDER,
    );

    vec![
        ToolCallIntent::FsListDir {
            workspace_id: ws_id,
            workspace_root: ws_root,
            rel_path,
        },
        ToolCallIntent::LlmGenerate {
            strategy,
            task: llm_task,
            system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
        },
    ]
}

/// Tries to extract an explicit file path (token ending with a known extension, or an
/// absolute path) from the task string.
///
/// Returns `(workspace_id, workspace_root, rel_path)` or `None` if no recognisable
/// file token is found.
///
/// For absolute paths (starting with `/`): `workspace_root = "/"`, `rel_path = path[1..]`.
/// For bare filenames: the provided default workspace root is used.
fn extract_explicit_file_path(
    task: &str,
    default_workspace_id: Option<&str>,
    default_workspace_root: Option<&str>,
) -> Option<(String, String, String)> {
    const FILE_EXTS: &[&str] = &[
        ".pdf", ".md", ".txt", ".docx", ".doc", ".csv", ".json", ".yaml", ".yml", ".rst", ".tex",
        ".log", ".py", ".rs", ".js", ".ts", ".swift",
    ];
    let ws_id = default_workspace_id.unwrap_or("").to_string();

    for token in task.split_whitespace() {
        let cleaned = token
            .trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | ')' | '(' | '[' | ']'));
        let lower = cleaned.to_lowercase();

        // Token ends with a known file extension
        if FILE_EXTS.iter().any(|ext| lower.ends_with(ext)) {
            if cleaned.starts_with('/') {
                let rel = cleaned.trim_start_matches('/').to_string();
                return Some((ws_id, "/".to_string(), rel));
            } else if let Some(root) = default_workspace_root {
                if !root.is_empty() {
                    return Some((ws_id, root.to_string(), cleaned.to_string()));
                }
            }
        }

        // Absolute path without a known extension but looks like a file
        // (contains a dot, does not end with '/', at least one '/' after the root)
        if cleaned.starts_with('/')
            && cleaned.contains('.')
            && !cleaned.ends_with('/')
            && !cleaned.ends_with('.')
            && cleaned.matches('/').count() >= 2
        {
            let rel = cleaned.trim_start_matches('/').to_string();
            return Some((ws_id, "/".to_string(), rel));
        }
    }

    None
}

/// Builds a two-step plan: first read the specific file, then ask the LLM to
/// summarise using only the real file content.
///
/// Step 1: `FsReadFile` at the resolved path.
/// Step 2: `LlmGenerate` with `{{FS_RESULT}}` in the prompt — `next_action`
///         substitutes the real file content before emitting the intent.
///
/// The LlmGenerate prompt explicitly forbids inventing content not present in
/// the tool output, preventing hallucination on file summaries.
pub(crate) fn build_plan_file_read_then_llm(
    task: &str,
    ws_id: String,
    ws_root: String,
    rel_path: String,
    strategy: Strategy,
) -> Vec<ToolCallIntent> {
    let llm_task = format!(
        "The user asked: \"{task}\"\n\n\
         Here is the actual file content returned by the filesystem tool:\n\
         {placeholder}\n\n\
         Using ONLY the file content above, provide a concise and accurate response. \
         Do NOT add, invent, or infer any information that is not present in the \
         tool result. If the content appears truncated or is unavailable, say so.",
        task = sanitize_task_for_llm(task),
        placeholder = FS_RESULT_PLACEHOLDER,
    );

    vec![
        ToolCallIntent::FsReadFile {
            workspace_id: ws_id,
            workspace_root: ws_root,
            rel_path,
        },
        ToolCallIntent::LlmGenerate {
            strategy,
            task: llm_task,
            system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
        },
    ]
}

/// Returns `true` when the task suggests the user wants to summarise or read
/// the most recently saved or modified file in a well-known directory, without
/// specifying a concrete filename.
///
/// Examples: "summarize the file I just saved in Downloads",
/// "what's in the most recent file in my Desktop folder"
fn has_most_recent_file_intent(task: &str) -> bool {
    let t = task.to_lowercase();
    let has_recency = t.contains("just saved")
        || t.contains("most recent")
        || t.contains("latest file")
        || t.contains("newest file")
        || t.contains("recently saved")
        || t.contains("recently modified");
    let has_location = t.contains("downloads") || t.contains("desktop") || t.contains("documents");
    has_recency && has_location
}

/// Builds a 3-step plan for "Summarise the most recently modified file in a directory":
///
///   1. `FsListDir` — lists the directory; the Swift executor sorts entries
///      newest-first in `sortedByModified`; `next_action` captures that into
///      `SessionState.last_dir_listing_sorted`.
///   2. `FsReadFile` with `rel_path = MOST_RECENT_FILE_PLACEHOLDER` — `next_action`
///      resolves the placeholder to `last_dir_listing_sorted[0]` before emitting.
///   3. `LlmGenerate` — summarises using the real file content injected via
///      `{{FS_RESULT}}`.
///
/// The plan runs to completion without asking the user any follow-up questions.
fn build_plan_list_read_summarize(
    task: &str,
    ws_id: String,
    ws_root: String,
    strategy: Strategy,
) -> Vec<ToolCallIntent> {
    let llm_task = format!(
        "The user asked: \"{task}\"\n\n\
         Here is the file content returned by the filesystem tool:\n\
         {placeholder}\n\n\
         Write a concise, clear summary in English. \
         Base your summary ONLY on the file content above — do not add \
         information not present in the file. \
         If the content appears to be binary or unreadable, say so explicitly.",
        task = sanitize_task_for_llm(task),
        placeholder = FS_RESULT_PLACEHOLDER,
    );
    vec![
        ToolCallIntent::FsListDir {
            workspace_id: ws_id.clone(),
            workspace_root: ws_root.clone(),
            rel_path: ".".to_string(),
        },
        ToolCallIntent::FsReadFile {
            workspace_id: ws_id.clone(),
            workspace_root: ws_root.clone(),
            rel_path: MOST_RECENT_FILE_PLACEHOLDER.to_string(),
        },
        ToolCallIntent::LlmGenerate {
            strategy,
            task: llm_task,
            system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
        },
    ]
}

// -----------------------------------------------------------------------------
// Helper functions for detection and plan building (Workflows 1–3)
// -----------------------------------------------------------------------------

/// Returns `true` when `name` ends with a readable text extension and is not a hidden file.
pub(crate) fn is_readable_file_type(name: &str) -> bool {
    const READABLE: &[&str] = &[
        ".md", ".txt", ".csv", ".json", ".yaml", ".yml", ".log", ".rst",
    ];
    let low = name.to_lowercase();
    READABLE.iter().any(|e| low.ends_with(e)) && !name.starts_with('.')
}

/// Finds a bare (non-absolute) filename token with a recognised extension in the task string.
/// Returns `None` if no such token exists or if the only match is an absolute path.
fn find_file_token(task: &str) -> Option<String> {
    const EXTS: &[&str] = &[
        ".md", ".pdf", ".txt", ".docx", ".csv", ".json", ".yaml", ".yml",
    ];
    for token in task.split_whitespace() {
        let c =
            token.trim_matches(|ch: char| matches!(ch, '"' | '\'' | ',' | ';' | '.' | ')' | '('));
        let low = c.to_lowercase();
        if EXTS.iter().any(|e| low.ends_with(e)) && !c.starts_with('/') && c.len() > 2 {
            return Some(c.to_string());
        }
    }
    None
}

/// Returns `(workspace_id, absolute_dir_path, filename)` when BOTH a well-known directory
/// (Downloads, Desktop, Documents, home) AND a bare filename with a known extension are
/// present in `task`.  The directory takes priority over `default_workspace_root`.
fn extract_file_in_known_dir(
    task: &str,
    default_workspace_id: Option<&str>,
    default_workspace_root: Option<&str>,
) -> Option<(String, String, String)> {
    let file = find_file_token(task)?;
    let (id, dir, _) = extract_fs_path(task, default_workspace_id, default_workspace_root)?;
    Some((id, dir, file))
}

// ── Workflow 2 / 3 helpers — disabled for v0.1, reserved for v0.2+ ─────────
// These functions are kept to preserve the implementation; the match arms that
// call them are commented out in `start_session`.  `#[allow(dead_code)]` silences
// the compiler until they are re-enabled.  See ROADMAP.md.

/// Returns `true` when the task requests a summary/overview of multiple files in a known dir.
///
/// Matches patterns like:
/// - "Summarize the 20 Test2_doc reports in Downloads into one overview"
/// - "Read all files in Downloads and create an overview"
#[allow(dead_code)]
fn has_multi_file_summarize_intent(task: &str) -> bool {
    let t = task.to_lowercase();
    let has_multi = (t.chars().any(|c| c.is_ascii_digit())
        && (t.contains("files") || t.contains("reports") || t.contains("docs")))
        || t.contains("all files")
        || t.contains("all reports")
        || t.contains("each file");
    let has_sum = t.contains("summarize")
        || t.contains("summarise")
        || t.contains("summary")
        || t.contains("overview")
        || t.contains("combine");
    has_multi && has_sum && extract_fs_path(task, None, None).is_some()
}

/// Returns `true` when the task requests moving files into a new named folder.
///
/// Matches: "Move those 20 Test2_doc files from Downloads into a new folder called Test_folder on Desktop"
#[allow(dead_code)]
fn has_move_to_dir_intent(task: &str) -> bool {
    let t = task.to_lowercase();
    (t.contains("move") || t.contains("copy"))
        && t.contains("into")
        && t.contains("folder")
        && (t.contains("called") || t.contains("named"))
}

/// Extracts a file name-prefix pattern from the part of the task BEFORE "into".
/// Returns a token that contains an underscore (e.g. `"Test2_doc"`) to use as a
/// filter when iterating directory listings.
#[allow(dead_code)]
fn extract_file_pattern(task: &str) -> Option<String> {
    let t = task.to_lowercase();
    // Only search before "into" to avoid capturing the destination folder name.
    let end = t.find(" into ").unwrap_or(t.len());
    for token in task[..end].split_whitespace() {
        let c = token.trim_matches(|ch: char| matches!(ch, '"' | '\''));
        if c.len() >= 3 && c.contains('_') && !c.contains('.') {
            let low = c.to_lowercase();
            if !["downloads", "desktop", "documents"].contains(&low.as_str()) {
                return Some(c.to_string());
            }
        }
    }
    None
}

#[allow(dead_code)]
pub(crate) struct MoveParams {
    pub src_ws_id: String,
    pub src_ws_root: String,
    pub dest_ws_id: String,
    pub dest_ws_root: String,
    pub dest_rel_dir: String,
    pub file_filter: Option<String>,
}

/// Extracts move parameters from a natural-language task.
///
/// Requires all of: `into`, a destination folder name (after "called"/"named"), a
/// source directory keyword, and a destination directory keyword.  Returns `None`
/// when any required piece is missing so the orchestrator can fall back gracefully.
#[allow(dead_code)]
fn extract_move_params(task: &str, default_ws_id: Option<&str>) -> Option<MoveParams> {
    let t = task.to_lowercase();
    let home = dirs_next::home_dir()?;

    // Destination folder name (preserve original casing from `task`).
    let dest_rel_dir = ["called ", "named "].iter().find_map(|kw| {
        let pos = t.find(kw)? + kw.len();
        // Find the corresponding position in the original-case task.
        let orig_after = &task[task[..pos.min(task.len())].len()..];
        orig_after
            .split_whitespace()
            .next()
            .map(|s| {
                s.trim_matches(|c: char| matches!(c, '"' | '\'' | '.' | ','))
                    .to_string()
            })
            .filter(|s| !s.is_empty())
    })?;

    // Split around " into " to determine source (before) and destination (after).
    let into_pos = t.find(" into ")?;
    let before = &t[..into_pos];
    let after = &t[into_pos..];

    let src_root = if before.contains("downloads") {
        home.join("Downloads")
    } else if before.contains("desktop") {
        home.join("Desktop")
    } else if before.contains("documents") {
        home.join("Documents")
    } else {
        return None;
    };

    let dest_root = if after.contains("desktop") {
        home.join("Desktop")
    } else if after.contains("documents") {
        home.join("Documents")
    } else if after.contains("downloads") {
        home.join("Downloads")
    } else {
        return None;
    };

    let ws_id = default_ws_id.unwrap_or("").to_string();
    Some(MoveParams {
        src_ws_id: ws_id.clone(),
        src_ws_root: src_root.to_string_lossy().into_owned(),
        dest_ws_id: ws_id,
        dest_ws_root: dest_root.to_string_lossy().into_owned(),
        dest_rel_dir,
        file_filter: extract_file_pattern(task),
    })
}

/// Sentinel plan for multi-file summarize:
///   `[FsListDir, LlmGenerate("…{{FILE_CONTENTS}}…")]`
///
/// `do_expand_plan` (called from `next_action` after FsListDir succeeds) inserts the
/// concrete `FsReadFile` steps between `FsListDir` and `LlmGenerate`.
#[allow(dead_code)]
fn build_plan_list_then_multi_summarize(
    _task: &str,
    ws_id: String,
    ws_root: String,
    strategy: Strategy,
) -> Vec<ToolCallIntent> {
    let llm_task = format!(
        "You are summarizing a set of documents whose contents have already been \
         loaded for you.\n\
         All necessary tool calls have already completed before this prompt.\n\
         You cannot call tools. You can only write.\n\n\
         Write a clear, comprehensive English overview of the following documents, \
         as if you are explaining them to a technically literate user.\n\
         Only use information that appears in the document contents below. \
         Do not invent tools or steps that are not described there.\n\n\
         ---\n\
         DOCUMENT CONTENTS:\n\
         {placeholder}\n\
         ---\n\n\
         Overview:",
        placeholder = FILE_CONTENTS_PLACEHOLDER,
    );
    vec![
        ToolCallIntent::FsListDir {
            workspace_id: ws_id.clone(),
            workspace_root: ws_root.clone(),
            rel_path: ".".to_string(),
        },
        ToolCallIntent::LlmGenerate {
            strategy,
            task: llm_task,
            system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
        },
    ]
}

/// Sentinel plan for create-folder + move:
///   `[FsListDir(src), LlmGenerate(confirmation)]`
///
/// `do_expand_plan` inserts `[FsCreateDir, FsMove×N]` between them after FsListDir.
#[allow(dead_code)]
fn build_plan_list_then_move(task: &str, ws_id: String, ws_root: String) -> Vec<ToolCallIntent> {
    let llm_task = format!(
        "The user asked: \"{task}\"\n\n\
         All files have been moved to the destination folder. \
         Confirm this to the user and list the files that were moved. \
         Do not invent file names — only report what was actually moved.",
        task = sanitize_task_for_llm(task),
    );
    vec![
        ToolCallIntent::FsListDir {
            workspace_id: ws_id,
            workspace_root: ws_root,
            rel_path: ".".to_string(),
        },
        ToolCallIntent::LlmGenerate {
            strategy: Strategy::CheapModel,
            task: llm_task,
            system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
        },
    ]
}

/// Applies a `pending_expansion` to the live plan, inserting concrete tool steps
/// immediately after the `FsListDir` that just completed.
///
/// Called by `next_action` when `session.pending_expansion` is `Some` and the step
/// that just finished was a `FsListDir`.
pub(crate) fn do_expand_plan(session: &mut SessionState, expansion: ExpansionKind) {
    let insert_idx = session.step_index; // insert before the current LlmGenerate sentinel
    let ws_root = session.confirmed_root.clone().unwrap_or_default();
    // Recover workspace_id from the FsListDir step that just completed.
    let ws_id = match session.plan.get(session.step_index.saturating_sub(1)) {
        Some(ToolCallIntent::FsListDir { workspace_id, .. }) => workspace_id.clone(),
        _ => String::new(),
    };

    match expansion {
        ExpansionKind::ReadAndSummarize { file_filter } => {
            let reads: Vec<ToolCallIntent> = session
                .last_dir_listing_sorted
                .iter()
                .filter(|f| {
                    let matches_filter = file_filter
                        .as_deref()
                        .map(|p| f.contains(p))
                        .unwrap_or(true);
                    matches_filter && is_readable_file_type(f)
                })
                .map(|name| ToolCallIntent::FsReadFile {
                    workspace_id: ws_id.clone(),
                    workspace_root: ws_root.clone(),
                    rel_path: name.clone(),
                })
                .collect();
            session.plan.splice(insert_idx..insert_idx, reads);
        }
        ExpansionKind::MoveToDir {
            dest_workspace_id,
            dest_workspace_root,
            dest_rel_dir,
            file_filter,
        } => {
            let mut new_steps = vec![ToolCallIntent::FsCreateDir {
                workspace_id: dest_workspace_id.clone(),
                workspace_root: dest_workspace_root.clone(),
                rel_path: dest_rel_dir.clone(),
            }];
            for name in session.last_dir_listing_sorted.iter().filter(|f| {
                file_filter
                    .as_deref()
                    .map(|p| f.contains(p))
                    .unwrap_or(!f.starts_with('.'))
            }) {
                new_steps.push(ToolCallIntent::FsMove {
                    workspace_id: ws_id.clone(),
                    workspace_root: ws_root.clone(),
                    rel_path: name.clone(),
                    dest_workspace_root: dest_workspace_root.clone(),
                    dest_rel_path: format!("{}/{}", dest_rel_dir, name),
                });
            }
            session.plan.splice(insert_idx..insert_idx, new_steps);
        }
    }
}

// -----------------------------------------------------------------------------
// start_session
// -----------------------------------------------------------------------------

/// Builds initial session: run router and build plan. Call once at task start.
// ── Extension checklist ───────────────────────────────────────────────────────
// To add a new workflow or ToolCallIntent variant:
//   1. Add the new ToolCallIntent variant in types.rs (no FFI regen needed —
//      it appears as a new `kind` string in the existing FfiToolIntent).
//   2. Add a detection function (has_*_intent) above this block.
//   3. Add an extraction helper if parameters need to be parsed (extract_*).
//   4. Add a plan builder (build_plan_*) and optionally an ExpansionKind variant.
//   5. Add a new `Strategy::CheapModel if has_*_intent(&task) =>` arm in the
//      match below, BEFORE the existing fallback arms.  Priority: most-specific
//      guards first (move > most-recent > file-in-known-dir > multi-file > implicit-fs
//      > implicit-file-read > default).
//   6. Add the new tool kind to SwiftAgentExecutor.swift (handleFs*, dispatch case)
//      and to AgentRunner.approvalRequiredKinds if it requires user approval.
//   7. Add unit tests covering: start_session plan shape, do_expand_plan if used,
//      and the full accumulate+substitute round-trip in next_action if applicable.
// ─────────────────────────────────────────────────────────────────────────────
pub fn start_session(
    session_id: String,
    input: crate::semantic_router::RouterInput,
    default_workspace_id: Option<String>,
    default_workspace_root: Option<String>,
) -> (SessionState, Option<ToolCallIntent>) {
    let router_output = route(&input);
    let task = input.task_description.clone();
    let context = input.context.clone();

    // Workflow 2 (multi-file summarize) and Workflow 3 (move-to-dir) are disabled
    // for v0.1 — their plan-expansion paths have not been validated end-to-end
    // on real files and could trigger FsMove / large content accumulation on
    // unvalidated code paths.  Re-enable by restoring the commented arms below
    // and changing `let` back to `let mut`.  See ROADMAP.md.
    let pending_expansion: Option<ExpansionKind> = None;
    let plan: Vec<ToolCallIntent> = match router_output.strategy {
        Strategy::ToolsOnly => build_plan_tools_only(
            &task,
            &context,
            default_workspace_id.as_deref(),
            default_workspace_root.as_deref(),
        ),
        // Most-specific first (priority order):
        // 1. Move-to-dir (Workflow 3) — DISABLED for v0.1. See ROADMAP.md.
        //    Re-enable: Strategy::CheapModel if has_move_to_dir_intent(&task) => { ... }

        // 2. Most-recent file: "just saved / most recent file in Downloads/Desktop/Documents"
        //    must win before the generic has_implicit_fs_intent (which also matches "Downloads").
        Strategy::CheapModel if has_most_recent_file_intent(&task) => {
            // User wants to summarise the most recently saved/modified file in a
            // well-known directory. The concrete filename is unknown at planning time —
            // `build_plan_list_read_summarize` leaves `{{MOST_RECENT_FILE}}` in
            // the `FsReadFile.rel_path`; `next_action` resolves it after the listing.
            match extract_fs_path(
                &task,
                default_workspace_id.as_deref(),
                default_workspace_root.as_deref(),
            ) {
                Some((ws_id, ws_root, _)) => {
                    build_plan_list_read_summarize(&task, ws_id, ws_root, Strategy::CheapModel)
                }
                None => build_plan_with_llm(&task, Strategy::CheapModel),
            }
        }
        // 3. Single named file in a known directory: "Summarize Test1_doc.md in Downloads".
        //    `has_implicit_file_read_intent` has a "no dir hint" constraint so this task
        //    would otherwise fall through to has_implicit_fs_intent and build a listing plan.
        Strategy::CheapModel
            if extract_file_in_known_dir(
                &task,
                default_workspace_id.as_deref(),
                default_workspace_root.as_deref(),
            )
            .is_some() =>
        {
            let (ws_id, ws_root, file_name) = extract_file_in_known_dir(
                &task,
                default_workspace_id.as_deref(),
                default_workspace_root.as_deref(),
            )
            .unwrap();
            build_plan_file_read_then_llm(&task, ws_id, ws_root, file_name, Strategy::CheapModel)
        }
        // 4. Multi-file summarise (Workflow 2) — DISABLED for v0.1.
        //    Context-budget handling (content_accumulator vs 8K context limit)
        //    needs end-to-end validation before this path is safe for real users.
        //    Re-enable: Strategy::CheapModel if has_multi_file_summarize_intent(&task) => { ... }
        //    See ROADMAP.md.
        Strategy::CheapModel if has_implicit_fs_intent(&task) => {
            // Two-step FS-first plan: list the real directory, then let the LLM
            // summarise using the actual listing (not invented content).
            match extract_fs_path(
                &task,
                default_workspace_id.as_deref(),
                default_workspace_root.as_deref(),
            ) {
                Some((ws_id, ws_root, rel_path)) => {
                    build_plan_fs_then_llm(&task, ws_id, ws_root, rel_path, Strategy::CheapModel)
                }
                None => {
                    // No resolvable path: guide the LLM to ask rather than invent.
                    vec![ToolCallIntent::LlmGenerate {
                        strategy: Strategy::CheapModel,
                        task: format!(
                            "The user asked: \"{task}\"\n\n\
                             You could not determine which folder or file they meant. \
                             Ask them to specify a full path (for example /Users/me/Desktop) \
                             rather than inventing or guessing a folder structure."
                        ),
                        system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
                    }]
                }
            }
        }
        Strategy::CheapModel if has_implicit_file_read_intent(&task) => {
            // User wants to summarize or describe a specific file's content.
            // Sub-case A: explicit file path in task → read the file directly.
            // Sub-case B: no explicit path → list workspace so the model can identify
            //             the file and avoid fabricating its contents.
            if let Some((ws_id, ws_root, rel_path)) = extract_explicit_file_path(
                &task,
                default_workspace_id.as_deref(),
                default_workspace_root.as_deref(),
            ) {
                build_plan_file_read_then_llm(&task, ws_id, ws_root, rel_path, Strategy::CheapModel)
            } else {
                match extract_fs_path(
                    &task,
                    default_workspace_id.as_deref(),
                    default_workspace_root.as_deref(),
                ) {
                    Some((ws_id, ws_root, rel_path)) => {
                        // Reuse the existing FsListDir → LlmGenerate plan.
                        // The prompt already forbids fabricating content not in the tool result.
                        build_plan_fs_then_llm(
                            &task,
                            ws_id,
                            ws_root,
                            rel_path,
                            Strategy::CheapModel,
                        )
                    }
                    None => vec![ToolCallIntent::LlmGenerate {
                        strategy: Strategy::CheapModel,
                        task: format!(
                            "The user asked: \"{task}\"\n\n\
                             You could not determine which file they meant. \
                             Ask them to specify the full path to the file rather than \
                             guessing or inventing its contents.",
                            task = sanitize_task_for_llm(&task),
                        ),
                        system_hint: Some(SUMMARIZER_SYSTEM_HINT.to_string()),
                    }],
                }
            }
        }
        Strategy::CheapModel | Strategy::ExpensiveModel => {
            build_plan_with_llm(&task, router_output.strategy)
        }
    };

    let session = SessionState {
        session_id: session_id.clone(),
        task_description: task,
        state: TaskState::Executing,
        // Classify the session once from the built plan.  A plan that is purely
        // [LlmGenerate] (the chat fallback) is Chat; any plan with an FS/shell
        // tool is Task.  This is the only place `kind` is ever set.
        kind: SessionKind::from_plan(&plan),
        router_output: Some(router_output),
        plan: plan.clone(),
        step_index: 0,
        accumulated_response: None,
        tool_traces: Vec::new(),
        approvals_requested: Vec::new(),
        intermediate_observations: Vec::new(),
        confirmed_root: None,
        last_dir_listing_sorted: Vec::new(),
        content_accumulator: Vec::new(),
        pending_expansion,
        debug_tag: None,
    };

    // ── Plan-built log ────────────────────────────────────────────────────────
    // Emit the full ordered plan so callers can trace exactly which tools will run.
    // Tag field is `session.debug_tag` when set, enabling grep filtering.
    {
        let tag = session.debug_tag.as_deref().unwrap_or("");
        let steps: Vec<String> = session.plan.iter().map(format_tool_call).collect();
        let steps_str = steps
            .iter()
            .enumerate()
            .map(|(i, s)| format!("[{i}] {s}"))
            .collect::<Vec<_>>()
            .join(", ");
        agent_event!(DEBUG, session_id, "plan_built",
            tag = tag,
            plan_len = session.plan.len(),
            plan = %steps_str,
        );
    }
    // ─────────────────────────────────────────────────────────────────────────

    let first_intent = session.plan.first().cloned();

    (session, first_intent)
}
