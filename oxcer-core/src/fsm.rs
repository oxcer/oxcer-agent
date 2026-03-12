//! Rust-native FSM agent orchestrator.
//!
//! The LLM is called at exactly two states:
//!
//! 1. **ActionSelection** — the model outputs *only* a tool name + arguments
//!    or the literal `[NO_TOOL]`. The prompt is heavily constrained.
//! 2. **Finalize** — the model is given the accumulated tool output and asked
//!    to produce a plain-language answer to the original query.
//!
//! Rust drives every state transition. The LLM is never trusted to produce
//! JSON, reason about state, or decide when the loop ends.

use crate::db::StateDb;
use crate::executor::{ToolCall, UniversalExecutor};
use crate::guardrail::{self, ActionSpec, AgenticError};
use std::fmt;
use std::path::{Path, PathBuf};

/// Trait injected by the FFI layer so `oxcer-core` never depends on the FFI
/// crate or the loaded Phi-3 runtime.
pub trait LlmCallback: Send + Sync {
    fn generate(&self, prompt: &str) -> String;
}

/// States in the FSM agent loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Init,
    RetrieveContext,
    ActionSelection,
    Execution,
    Validation,
    Finalize,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Init => "Init",
            Self::RetrieveContext => "RetrieveContext",
            Self::ActionSelection => "ActionSelection",
            Self::Execution => "Execution",
            Self::Validation => "Validation",
            Self::Finalize => "Finalize",
        };
        write!(f, "{s}")
    }
}

/// Mutable context carried through the FSM loop.
struct FsmContext {
    query: String,
    /// Episodic context prepended to the ActionSelection prompt.
    memory_context: String,
    /// The `ToolCall` chosen at ActionSelection (cleared after Execution).
    pending_tool: Option<ToolCall>,
    /// Output produced by the most-recent tool execution.
    tool_output: Option<String>,
    /// How many ActionSelection+Execution cycles have completed.
    step: usize,
    /// Canonical path of the document last read via `read_document`.
    /// Set during Execution; used in Finalize to write `<path>.summary.md`
    /// when the query looks like a summarization request.
    last_doc_path: Option<PathBuf>,
    /// The last directory listed, or the parent directory of the last file
    /// touched. Injected into ActionSelection so the LLM can resolve relative
    /// paths without knowing the absolute workspace root.
    context_dir: Option<PathBuf>,
    /// Canonical paths of recently accessed files, capped at `RECENT_FILES_MAX`.
    /// Most-recently used is last; injected into ActionSelection so the LLM can
    /// resolve "this file" / "that file" pronouns.
    recent_files: Vec<PathBuf>,
}

/// The FSM agent.
pub struct AgentFsm {
    executor: UniversalExecutor,
    db: StateDb,
    /// Maximum number of ActionSelection→Execution cycles before giving up.
    max_steps: usize,
}

impl AgentFsm {
    /// Construct a new FSM agent.
    pub fn new(executor: UniversalExecutor, db: StateDb, max_steps: usize) -> Self {
        Self {
            executor,
            db,
            max_steps,
        }
    }

    /// Run the FSM for `query` and return a plain-language answer.
    pub fn run(&self, query: &str, llm: &dyn LlmCallback) -> Result<String, AgenticError> {
        let mut state = AgentState::Init;
        let mut ctx = FsmContext {
            query: query.to_string(),
            memory_context: String::new(),
            pending_tool: None,
            tool_output: None,
            step: 0,
            last_doc_path: None,
            context_dir: None,
            recent_files: Vec::new(),
        };

        loop {
            tracing::debug!(
                event = "fsm_transition",
                state = %state,
                step = ctx.step,
                query = %ctx.query,
            );

            state = match state {
                AgentState::Init => AgentState::RetrieveContext,

                AgentState::RetrieveContext => {
                    let facts = self.db.get_recent_context(5)?;
                    if facts.is_empty() {
                        ctx.memory_context = String::new();
                    } else {
                        let mut lines = vec!["[MEMORY]".to_string()];
                        for f in &facts {
                            lines.push(format!("Q: {} => {}", f.query, f.observation));
                        }
                        lines.push("[/MEMORY]".to_string());
                        ctx.memory_context = lines.join("\n");
                    }
                    AgentState::ActionSelection
                }

                AgentState::ActionSelection => {
                    if ctx.step >= self.max_steps {
                        return Err(AgenticError::StepLimitExceeded(self.max_steps));
                    }

                    let context_sec =
                        build_context_section(ctx.context_dir.as_deref(), &ctx.recent_files);
                    let prompt = build_action_selection_prompt(
                        &ctx.query,
                        &ctx.memory_context,
                        ctx.tool_output.as_deref(),
                        &context_sec,
                    );
                    let raw = llm.generate(&prompt);
                    let spec = guardrail::validate_action_selection(&raw)?;

                    match spec {
                        ActionSpec::NoTool => AgentState::Finalize,
                        ActionSpec::Tool(call) => {
                            ctx.pending_tool = Some(call);
                            AgentState::Execution
                        }
                    }
                }

                AgentState::Execution => {
                    let tool = ctx
                        .pending_tool
                        .take()
                        .expect("Execution state entered without a pending_tool");

                    // Resolve the tool's primary path once so we can both track
                    // summary context (ReadDocument) and update the navigation context
                    // (context_dir / recent_files) without a second resolve call.
                    let primary_path: Option<PathBuf> = match &tool {
                        ToolCall::FsListDir(p)
                        | ToolCall::FsReadFile(p)
                        | ToolCall::ReadDocument(p) => {
                            Some(self.executor.resolve_path(p).unwrap_or_else(|_| p.clone()))
                        }
                        _ => None,
                    };

                    // For ReadDocument, record the canonical path so Finalize can
                    // write a <path>.summary.md after producing the answer.
                    if matches!(tool, ToolCall::ReadDocument(_)) {
                        ctx.last_doc_path = primary_path.clone();
                    }

                    let output = match self.executor.execute(&tool) {
                        Ok(out) => out,
                        Err(e) => format!("[TOOL_ERROR: {e}]"),
                    };

                    // Update navigation context for natural-language resolution.
                    // FsListDir sets context_dir to the listed directory.
                    // FsReadFile / ReadDocument set context_dir to the file's parent
                    // and push the file into recent_files.
                    match (&tool, primary_path) {
                        (ToolCall::FsListDir(_), Some(dir)) => {
                            ctx.context_dir = Some(dir);
                        }
                        (ToolCall::FsReadFile(_) | ToolCall::ReadDocument(_), Some(file)) => {
                            let parent = file.parent().map(|p| p.to_path_buf());
                            if let Some(dir) = parent {
                                ctx.context_dir = Some(dir);
                            }
                            push_recent_file(&mut ctx.recent_files, file);
                        }
                        _ => {}
                    }

                    // Persist what we observed.
                    let observation = format!("{output:.200}"); // trim very long outputs
                    let _ = self.db.insert_fact(&ctx.query, &observation);

                    ctx.tool_output = Some(output);
                    ctx.step += 1;
                    AgentState::Validation
                }

                AgentState::Validation => {
                    // Currently a pass-through; add output-sanitisation here if needed.
                    AgentState::ActionSelection
                }

                AgentState::Finalize => {
                    let prompt = build_finalize_prompt(&ctx.query, ctx.tool_output.as_deref());
                    let raw = llm.generate(&prompt);
                    let mut answer = guardrail::validate_final_answer(&raw)?;

                    tracing::info!(
                        event = "fsm_complete",
                        steps = ctx.step,
                        answer_len = answer.len(),
                        query = %ctx.query,
                    );

                    // If a document was read AND the query looks like a summarization
                    // request, persist the answer as <doc_path>.summary.md and tell
                    // the user the exact path where it was saved.
                    if let Some(ref doc_path) = ctx.last_doc_path {
                        if is_summarization_query(&ctx.query) {
                            if let Some(saved) = write_summary_beside(doc_path, &answer) {
                                answer.push_str(&format!(
                                    "\n\nSaved summary to: {}",
                                    saved.display()
                                ));
                            }
                        }
                    }

                    return Ok(answer);
                }
            };
        }
    }
}

// ─── Prompt builders ─────────────────────────────────────────────────────────

fn build_action_selection_prompt(
    query: &str,
    memory_context: &str,
    previous_output: Option<&str>,
    context_section: &str,
) -> String {
    let mut parts: Vec<String> = vec![
        "You are a local OS Agent. You have authorization to read, move, and trash files."
            .to_string(),
        "Output EXACTLY one line — the tool call or [NO_TOOL]. No explanation, no markdown."
            .to_string(),
        String::new(),
        "Available tools:".to_string(),
        "  fs_list_dir <path>              — list directory contents".to_string(),
        "  fs_read_file <path>             — read a plain-text file".to_string(),
        "  read_document <path>            — extract text from TXT, PDF, DOCX, or XLSX".to_string(),
        "  move_file <source> | <dest>     — move a file within the workspace".to_string(),
        "  trash_file <path>               — move a file/folder to the OS recycle bin".to_string(),
        "  shell_run <command> [args...]   — run an allowed shell command".to_string(),
        "  [NO_TOOL]                       — no tool needed; proceed to answer".to_string(),
        String::new(),
        "Rules:".to_string(),
        "  - Do not apologize for modifying files. Execute the user's intent directly.".to_string(),
        "  - When asked to summarize a document, call read_document first, then [NO_TOOL]."
            .to_string(),
        "    A <filename>.summary.md will be written automatically beside the source file."
            .to_string(),
        "  - move_file args are separated by ' | ' (space-pipe-space).".to_string(),
        "  - Use absolute paths from [CONTEXT] when the user says 'Desktop', 'Downloads',"
            .to_string(),
        "    'Documents', 'this file', or 'that file'.".to_string(),
        String::new(),
        format!("Query: {query}"),
    ];

    if !context_section.is_empty() {
        parts.push(String::new());
        parts.push(context_section.to_string());
    }

    if !memory_context.is_empty() {
        parts.push(String::new());
        parts.push(memory_context.to_string());
    }

    if let Some(out) = previous_output {
        parts.push(String::new());
        parts.push(format!(
            "[PREVIOUS_TOOL_OUTPUT]\n{}\n[/PREVIOUS_TOOL_OUTPUT]",
            &out[..out.len().min(1000)]
        ));
        parts.push("Do you need another tool, or is [NO_TOOL] sufficient?".to_string());
    }

    parts.join("\n")
}

fn build_finalize_prompt(query: &str, tool_output: Option<&str>) -> String {
    let mut parts: Vec<String> = vec![
        "You are an answer-synthesis module.".to_string(),
        "Given the tool output below, answer the query in plain language.".to_string(),
        "Be concise. Do not mention the tools you used.".to_string(),
        String::new(),
        format!("Query: {query}"),
    ];

    if let Some(out) = tool_output {
        parts.push(String::new());
        parts.push(format!(
            "[TOOL_OUTPUT]\n{}\n[/TOOL_OUTPUT]",
            &out[..out.len().min(2000)]
        ));
    } else {
        parts.push(String::new());
        parts.push("[TOOL_OUTPUT]\n(no tool was run)\n[/TOOL_OUTPUT]".to_string());
    }

    parts.push(String::new());
    parts.push("Answer:".to_string());
    parts.join("\n")
}

// ─── Summary writing helpers ──────────────────────────────────────────────────

/// Returns `true` when the query suggests the user wants a summary or overview
/// of a document. Used to decide whether to write `<doc>.summary.md`.
fn is_summarization_query(query: &str) -> bool {
    let q = query.to_lowercase();
    q.contains("summar")     // summarize / summarise / summary
        || q.contains("overview")
        || q.contains("brief")
        || q.contains("tldr")
        || q.contains("tl;dr")
        || q.contains("condense")
}

/// Write `summary` as UTF-8 to `<doc_path>.summary.md`.
///
/// Returns `Some(summary_path)` on success so the caller can tell the user
/// the exact path. Logs a warning on failure and returns `None` — the caller
/// already has the answer and must not discard it due to a write failure.
fn write_summary_beside(doc_path: &Path, summary: &str) -> Option<PathBuf> {
    let mut name = doc_path.as_os_str().to_os_string();
    name.push(".summary.md");
    let summary_path = PathBuf::from(name);

    match std::fs::write(&summary_path, summary.as_bytes()) {
        Ok(()) => {
            tracing::info!(
                event = "summary_written",
                path = %summary_path.display(),
                bytes = summary.len(),
                "summary.md written beside source document"
            );
            Some(summary_path)
        }
        Err(e) => {
            tracing::warn!(
                event = "summary_write_failed",
                path = %summary_path.display(),
                err = %e,
                "failed to write summary file; answer still returned to caller"
            );
            None
        }
    }
}

// ─── Context tracking helpers ─────────────────────────────────────────────────

/// Maximum number of file paths kept in `FsmContext::recent_files`.
const RECENT_FILES_MAX: usize = 8;

/// Add `path` to the MRU list, deduplicating and evicting the oldest entry
/// when the cap is exceeded.
fn push_recent_file(recent: &mut Vec<PathBuf>, path: PathBuf) {
    recent.retain(|p| p != &path); // move-to-back dedup
    recent.push(path);
    if recent.len() > RECENT_FILES_MAX {
        recent.remove(0);
    }
}

/// Build a `[CONTEXT]` block that is injected into the ActionSelection prompt.
///
/// Provides:
/// - Absolute paths for well-known macOS directories so the LLM can resolve
///   names like "Desktop" → `/Users/alice/Desktop`.
/// - The current context directory (last directory listed or parent of the last
///   file touched) so the LLM can resolve bare filenames like `report.pdf`.
/// - Paths of recently accessed files so the LLM can resolve "this file" /
///   "that file" pronouns.
///
/// Returns an empty string when no context is available yet (e.g. the very
/// first query before any tools have run and before `dirs_next` returns paths).
fn build_context_section(context_dir: Option<&Path>, recent_files: &[PathBuf]) -> String {
    let mut known: Vec<(&str, PathBuf)> = Vec::new();
    if let Some(p) = dirs_next::desktop_dir() {
        known.push(("Desktop", p));
    }
    if let Some(p) = dirs_next::download_dir() {
        known.push(("Downloads", p));
    }
    if let Some(p) = dirs_next::document_dir() {
        known.push(("Documents", p));
    }
    if let Some(p) = dirs_next::home_dir() {
        known.push(("Home", p));
    }

    if known.is_empty() && context_dir.is_none() && recent_files.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = vec!["[CONTEXT]".to_string()];

    for (name, path) in &known {
        lines.push(format!("  {name} = {}", path.display()));
    }

    if let Some(dir) = context_dir {
        lines.push(format!("  current_dir = {}", dir.display()));
    }

    if !recent_files.is_empty() {
        lines.push("  recent_files:".to_string());
        for f in recent_files.iter().rev() {
            // most-recently used first
            lines.push(format!("    - {}", f.display()));
        }
    }

    lines.push("[/CONTEXT]".to_string());
    lines.join("\n")
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::StateDb;
    use crate::executor::UniversalExecutor;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// A scripted LLM that pops responses from a queue.
    struct ScriptedLlm {
        responses: Mutex<Vec<String>>,
    }

    impl ScriptedLlm {
        fn new(mut responses: Vec<String>) -> Self {
            responses.reverse(); // pop() from back = first element
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl LlmCallback for ScriptedLlm {
        fn generate(&self, _prompt: &str) -> String {
            self.responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or_else(|| "[NO_TOOL]".to_string())
        }
    }

    fn setup_fsm(tmp: &tempfile::TempDir) -> AgentFsm {
        let executor = UniversalExecutor::new(tmp.path()).unwrap();
        let db = StateDb::open_in_memory().unwrap();
        AgentFsm::new(executor, db, 5)
    }

    #[test]
    fn no_tool_path_reaches_finalize() {
        let tmp = tempdir().unwrap();
        let fsm = setup_fsm(&tmp);
        // ActionSelection → [NO_TOOL], Finalize → answer
        let llm = ScriptedLlm::new(vec![
            "[NO_TOOL]".to_string(),
            "The answer is 42.".to_string(),
        ]);
        let ans = fsm.run("What is 2+2?", &llm).unwrap();
        assert_eq!(ans, "The answer is 42.");
    }

    #[test]
    fn list_dir_then_finalize() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("hello.txt"), "hi").unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            // Step 1: list the directory
            "fs_list_dir .".to_string(),
            // Step 2: no more tools needed
            "[NO_TOOL]".to_string(),
            // Finalize
            "The directory contains hello.txt.".to_string(),
        ]);

        let ans = fsm.run("What files are here?", &llm).unwrap();
        assert_eq!(ans, "The directory contains hello.txt.");
    }

    #[test]
    fn read_file_then_finalize() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("note.txt"), "secret content").unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            format!("fs_read_file note.txt"),
            "[NO_TOOL]".to_string(),
            "The note says: secret content.".to_string(),
        ]);

        let ans = fsm.run("What does note.txt say?", &llm).unwrap();
        assert_eq!(ans, "The note says: secret content.");
    }

    #[test]
    fn hallucination_in_action_selection_returns_error() {
        let tmp = tempdir().unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec!["browse_web https://example.com".to_string()]);

        let err = fsm.run("Search the web", &llm).unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn step_limit_exceeded_returns_error() {
        let tmp = tempdir().unwrap();
        let executor = UniversalExecutor::new(tmp.path()).unwrap();
        let db = StateDb::open_in_memory().unwrap();
        // max_steps = 2, but LLM always asks for another tool
        let fsm = AgentFsm::new(executor, db, 2);

        let llm = ScriptedLlm::new(vec![
            "fs_list_dir .".to_string(),
            "fs_list_dir .".to_string(),
            "fs_list_dir .".to_string(), // never reached
        ]);

        let err = fsm.run("Loop forever", &llm).unwrap_err();
        assert!(matches!(err, AgenticError::StepLimitExceeded(2)));
    }

    #[test]
    fn episodic_memory_populated_after_tool_run() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "content").unwrap();
        let executor = UniversalExecutor::new(tmp.path()).unwrap();
        let db = StateDb::open_in_memory().unwrap();
        let fsm = AgentFsm::new(executor, db.clone(), 5);

        let llm = ScriptedLlm::new(vec![
            "fs_list_dir .".to_string(),
            "[NO_TOOL]".to_string(),
            "Done.".to_string(),
        ]);

        fsm.run("List files", &llm).unwrap();
        assert!(db.fact_count().unwrap() >= 1);
    }

    #[test]
    fn finalize_empty_answer_is_error() {
        let tmp = tempdir().unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            "[NO_TOOL]".to_string(),
            // Finalize returns empty
            "   ".to_string(),
        ]);

        let err = fsm.run("What?", &llm).unwrap_err();
        assert!(matches!(err, AgenticError::EmptyResponse));
    }

    #[test]
    fn prompt_builders_contain_query() {
        let p = build_action_selection_prompt("find the logs", "", None, "");
        assert!(p.contains("find the logs"));
        // New tools must appear in the prompt.
        assert!(p.contains("read_document"));
        assert!(p.contains("move_file"));
        assert!(p.contains("trash_file"));

        let p2 = build_finalize_prompt("find the logs", Some("log output here"));
        assert!(p2.contains("find the logs"));
        assert!(p2.contains("log output here"));
    }

    #[test]
    fn read_document_txt_then_finalize() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("report.txt"), "Annual revenue: $1M").unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            "read_document report.txt".to_string(),
            "[NO_TOOL]".to_string(),
            "The report shows annual revenue of $1M.".to_string(),
        ]);

        let ans = fsm.run("What does report.txt say?", &llm).unwrap();
        assert!(ans.contains("$1M"));
    }

    #[test]
    fn read_document_writes_summary_for_summarization_query() {
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("notes.txt"),
            "Meeting notes: decided on Q3 goals.",
        )
        .unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            "read_document notes.txt".to_string(),
            "[NO_TOOL]".to_string(),
            "The notes summarize Q3 goal decisions.".to_string(),
        ]);

        let ans = fsm.run("Summarize notes.txt", &llm).unwrap();

        let summary_path = tmp.path().join("notes.txt.summary.md");
        assert!(
            summary_path.exists(),
            "expected {summary_path:?} to be written"
        );
        let content = fs::read_to_string(&summary_path).unwrap();
        assert!(content.contains("Q3 goal"));
        // The answer returned to the caller must tell the user where the file was saved.
        assert!(
            ans.contains("Saved summary to:"),
            "expected 'Saved summary to:' in answer, got: {ans:?}"
        );
    }

    #[test]
    fn read_document_does_not_write_summary_for_non_summary_query() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("data.txt"), "Some data").unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            "read_document data.txt".to_string(),
            "[NO_TOOL]".to_string(),
            "The file contains some data.".to_string(),
        ]);

        fsm.run("What is in data.txt?", &llm).unwrap();

        // Not a summarization query — no .summary.md should be written.
        assert!(!tmp.path().join("data.txt.summary.md").exists());
    }

    #[test]
    fn move_file_within_workspace_succeeds() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("orig.txt"), "content").unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            "move_file orig.txt | renamed.txt".to_string(),
            "[NO_TOOL]".to_string(),
            "File moved successfully.".to_string(),
        ]);

        let ans = fsm.run("Move orig.txt to renamed.txt", &llm).unwrap();
        assert!(ans.contains("moved") || ans.contains("File"));
        assert!(!tmp.path().join("orig.txt").exists());
        assert!(tmp.path().join("renamed.txt").exists());
    }

    #[test]
    fn is_summarization_query_detects_summarize() {
        assert!(is_summarization_query("Please summarize this document"));
        assert!(is_summarization_query("Give me an overview of the PDF"));
        assert!(is_summarization_query("brief description of notes.txt"));
        assert!(is_summarization_query("tl;dr of the report"));
        assert!(!is_summarization_query("What files are in the folder?"));
        assert!(!is_summarization_query("Move this file to the archive"));
    }

    // ── push_recent_file ─────────────────────────────────────────────────────

    #[test]
    fn push_recent_file_deduplicates_and_caps() {
        let mut recent: Vec<PathBuf> = Vec::new();

        // Fill to max.
        for i in 0..RECENT_FILES_MAX {
            push_recent_file(&mut recent, PathBuf::from(format!("file{i}.txt")));
        }
        assert_eq!(recent.len(), RECENT_FILES_MAX);

        // Adding a new file evicts the oldest (file0.txt).
        push_recent_file(&mut recent, PathBuf::from("new.txt"));
        assert_eq!(recent.len(), RECENT_FILES_MAX);
        assert!(!recent.contains(&PathBuf::from("file0.txt")));
        assert_eq!(recent.last().unwrap(), &PathBuf::from("new.txt"));

        // Re-adding an existing entry deduplicates it and moves it to the end.
        push_recent_file(&mut recent, PathBuf::from("file1.txt"));
        assert_eq!(recent.last().unwrap(), &PathBuf::from("file1.txt"));
        assert!(recent.len() <= RECENT_FILES_MAX);
    }

    // ── build_context_section ────────────────────────────────────────────────

    #[test]
    fn build_context_section_includes_known_dirs_and_recent() {
        let section = build_context_section(
            Some(Path::new("/tmp/workspace")),
            &[PathBuf::from("/tmp/workspace/doc.txt")],
        );
        assert!(section.contains("[CONTEXT]"), "missing [CONTEXT] block");
        assert!(
            section.contains("current_dir = /tmp/workspace"),
            "missing current_dir"
        );
        assert!(
            section.contains("/tmp/workspace/doc.txt"),
            "missing recent file"
        );
        // Well-known dirs vary by OS; the block must be non-empty.
        assert!(!section.is_empty());
    }

    #[test]
    fn build_context_section_does_not_panic_with_empty_inputs() {
        // On a machine where dirs_next returns None for everything the section would
        // be empty, but on macOS at least Home is always defined. Either way the
        // function must not panic.
        let _ = build_context_section(None, &[]);
    }

    // ── summary path reported in answer ─────────────────────────────────────

    #[test]
    fn summary_answer_includes_saved_path() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("report.txt"), "Q4 results: up 10%").unwrap();
        let fsm = setup_fsm(&tmp);

        let llm = ScriptedLlm::new(vec![
            "read_document report.txt".to_string(),
            "[NO_TOOL]".to_string(),
            "The report shows Q4 results are up 10%.".to_string(),
        ]);

        let ans = fsm.run("Summarize report.txt", &llm).unwrap();
        assert!(
            ans.contains("Saved summary to:"),
            "expected 'Saved summary to:' in answer, got: {ans:?}"
        );
        assert!(
            ans.contains("report.txt.summary.md"),
            "expected summary filename in answer, got: {ans:?}"
        );
    }
}
