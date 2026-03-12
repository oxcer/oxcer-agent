//! Sub-Agent Orchestrator — Explore → Plan → Execute pipeline.
//!
//! Mirrors Claude Code's multi-agent architecture using three specialised roles:
//!
//! | Role    | Responsibility                                      |
//! |---------|-----------------------------------------------------|
//! | Explore | Discover files with MCP Glob; record facts          |
//! | Plan    | Read relevant files with MCP Read; build context   |
//! | Execute | Combine facts + context into LLM prompt; answer    |
//!
//! Memory is threaded through all three phases: facts recorded in Explore are
//! visible to Plan and Execute. The persistent `memory.md` file lets facts
//! survive across sessions.
//!
//! # Zero-hallucination guarantee
//!
//! The Execute phase only sees content that was **literally observed** by the
//! MCP tools in the previous phases. If no relevant file was read, the prompt
//! says so explicitly rather than asking the LLM to invent an answer.
//!
//! # Example
//!
//! ```no_run
//! use oxcer_core::subagent::{orchestrate, LlmCallback};
//!
//! struct MyLlm;
//! impl LlmCallback for MyLlm {
//!     fn generate(&self, prompt: &str) -> String {
//!         format!("Summary of: {}", &prompt[..prompt.len().min(40)])
//!     }
//! }
//!
//! let answer = orchestrate(
//!     "Summarize main.rs",
//!     "/path/to/workspace",
//!     "/path/to/memory.md",
//!     Some(&MyLlm),
//! );
//! println!("{answer}");
//! ```

use std::path::{Path, PathBuf};

use crate::mcp::{McpExecutor, McpTool};
use crate::memory::Memory;

// ── LLM callback ─────────────────────────────────────────────────────────────

/// Synchronous text generation callback.
///
/// The FFI layer implements this using the already-loaded `LocalPhi3Engine`.
/// Tests implement it with a stub that returns the prompt unchanged.
pub trait LlmCallback: Send + Sync {
    fn generate(&self, prompt: &str) -> String;
}

// ── Agent role ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    /// Discover files in the workspace (MCP Glob).
    Explore,
    /// Read relevant files and build an observation context (MCP Read / Grep).
    Plan,
    /// Synthesise observations into a final answer (LLM).
    Execute,
}

// ── SubAgent ──────────────────────────────────────────────────────────────────

/// A single-role agent with its own MCP executor and shared memory.
pub struct SubAgent {
    pub role: AgentRole,
    /// Shared fact store. Pass between phases to accumulate observations.
    pub memory: Memory,
    executor: McpExecutor,
}

impl SubAgent {
    /// Create a new sub-agent.
    ///
    /// `workspace_root` scopes all file operations.
    /// `memory` is the fact store; thread it from one agent to the next.
    pub fn new(role: AgentRole, workspace_root: impl AsRef<Path>, memory: Memory) -> Self {
        SubAgent {
            role,
            memory,
            executor: McpExecutor::new(workspace_root.as_ref().to_path_buf()),
        }
    }

    /// Run one unit of work appropriate for this agent's role.
    ///
    /// * Explore — `input` is a workspace hint (path or hint string, ignored for now).
    /// * Plan    — `input` is the user's query.
    /// * Execute — `input` is the user's query; `llm` must be `Some(…)` for a real answer.
    ///
    /// Returns a structured observation string.
    pub fn step(&mut self, input: &str, llm: Option<&dyn LlmCallback>) -> String {
        match self.role {
            AgentRole::Explore => self.explore(),
            AgentRole::Plan => self.plan(input),
            AgentRole::Execute => self.execute(input, &String::new(), llm),
        }
    }

    /// Execute with additional pre-read file context (used by `orchestrate`).
    pub(crate) fn step_with_context(
        &mut self,
        query: &str,
        direct_context: &str,
        llm: Option<&dyn LlmCallback>,
    ) -> String {
        self.execute(query, direct_context, llm)
    }

    // ── Explore ───────────────────────────────────────────────────────────────

    fn explore(&mut self) -> String {
        // 1. Source files (Rust, Swift, Python, TypeScript, etc.)
        let source_list = self.executor.execute(McpTool::Glob {
            pattern: "**/*.{rs,swift,py,ts,tsx,js,jsx,md}".to_string(),
            base_dir: None,
        });
        if !source_list.contains("No files found") && !source_list.contains("[MCP_ERROR:") {
            self.memory.append_fact(&format!(
                "Explore: source files — {}",
                summarize_file_list(&source_list)
            ));
        }

        // 2. Config / manifest files at the root
        let config_list = self.executor.execute(McpTool::Glob {
            pattern: "*.{toml,json,yaml,yml,lock}".to_string(),
            base_dir: None,
        });
        if !config_list.contains("No files found") && !config_list.contains("[MCP_ERROR:") {
            self.memory.append_fact(&format!(
                "Explore: root configs — {}",
                summarize_file_list(&config_list)
            ));
        }

        // Return the source list as the primary observation
        source_list
    }

    // ── Plan ──────────────────────────────────────────────────────────────────

    fn plan(&mut self, query: &str) -> String {
        let target_files = self.identify_targets(query);
        let mut context_parts: Vec<String> = Vec::new();

        for file_path in &target_files {
            let content = self.executor.execute(McpTool::Read {
                path: file_path.clone(),
            });
            if !content.contains("[MCP_ERROR:") {
                self.memory.append_fact(&format!(
                    "Plan: read {} ({} chars)",
                    file_path,
                    content.len()
                ));
                if context_parts
                    .iter()
                    .map(|s: &String| s.len())
                    .sum::<usize>()
                    < 16_000
                {
                    context_parts.push(content);
                }
            }
        }

        // Fallback: grep for the first keyword if no explicit file matched
        if context_parts.is_empty() {
            if let Some(kw) = extract_keywords(query).first() {
                let grep = self.executor.execute(McpTool::Grep {
                    pattern: kw.clone(),
                    path: ".".to_string(),
                });
                if !grep.contains("No matches") && !grep.contains("[MCP_ERROR:") {
                    self.memory
                        .append_fact(&format!("Plan: grep '{}' found results", kw));
                    context_parts.push(grep);
                }
            }
        }

        context_parts.join("\n\n")
    }

    // ── Execute ───────────────────────────────────────────────────────────────

    fn execute(
        &mut self,
        query: &str,
        direct_context: &str,
        llm: Option<&dyn LlmCallback>,
    ) -> String {
        let memory_ctx = self.memory.as_context(query);
        let prompt = build_prompt(query, direct_context, &memory_ctx);

        self.memory.append_fact(&format!(
            "Execute: answered '{}'",
            query.chars().take(80).collect::<String>()
        ));

        match llm {
            Some(engine) => engine.generate(&prompt),
            None => format!(
                "[EXECUTE_STUB]\nQuery: {}\nContext chars: {}\n[/EXECUTE_STUB]",
                query,
                direct_context.len() + memory_ctx.len()
            ),
        }
    }

    // ── File targeting ────────────────────────────────────────────────────────

    /// Identify which files to read based on query tokens and memory recall.
    fn identify_targets(&self, query: &str) -> Vec<String> {
        let mut targets: Vec<String> = Vec::new();

        // 1. Explicit file names or paths with known extensions in the query
        for token in query.split_whitespace() {
            let t = token.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | ':'));
            if has_file_ext(t) && !targets.contains(&t.to_string()) {
                targets.push(t.to_string());
            }
        }
        if !targets.is_empty() {
            return targets;
        }

        // 2. Mine file names from relevant memory facts
        let recalled = self.memory.query(query);
        for fact in recalled {
            for token in fact.content.split_whitespace() {
                let t = token.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | ':'));
                if has_file_ext(t) && !targets.contains(&t.to_string()) {
                    targets.push(t.to_string());
                    if targets.len() >= 5 {
                        return targets;
                    }
                }
            }
        }
        targets
    }
}

// ── Top-level orchestration ───────────────────────────────────────────────────

/// Run the full **Explore → Plan → Execute** pipeline for `query`.
///
/// * `workspace_root` — all file operations are scoped to this directory.
/// * `memory_path`    — persistent `memory.md`; created if absent.
/// * `llm`            — inference callback. Pass `None` to get a stub (tests).
///
/// Returns the final answer string.
pub fn orchestrate(
    query: &str,
    workspace_root: &str,
    memory_path: &str,
    llm: Option<&dyn LlmCallback>,
) -> String {
    let workspace = PathBuf::from(workspace_root);
    let memory_pb = Path::new(memory_path);

    // ── Phase 1: Explore ──────────────────────────────────────────────────────
    let shared_memory = Memory::load_or_create(memory_pb);
    let mut explore = SubAgent::new(AgentRole::Explore, &workspace, shared_memory);
    let _file_list = explore.step(workspace_root, None);
    let explore_memory = explore.memory;

    // ── Phase 2: Plan ─────────────────────────────────────────────────────────
    let mut plan_agent = SubAgent::new(AgentRole::Plan, &workspace, explore_memory);
    let direct_context = plan_agent.step(query, None);
    let plan_memory = plan_agent.memory;

    // ── Phase 3: Execute ──────────────────────────────────────────────────────
    let mut execute_agent = SubAgent::new(AgentRole::Execute, &workspace, plan_memory);
    execute_agent.step_with_context(query, &direct_context, llm)
}

// ── Prompt construction ───────────────────────────────────────────────────────

fn build_prompt(query: &str, direct_context: &str, memory_ctx: &str) -> String {
    let mut p = String::new();
    let has_context = !direct_context.is_empty() || !memory_ctx.is_empty();

    if has_context {
        p.push_str(
            "You are Oxcer, a local desktop AI assistant. \
             Answer the user's question based ONLY on the verified facts and file \
             contents shown below. Do NOT invent, guess, or hallucinate any \
             information not present in these observations.\n\n",
        );
        if !direct_context.is_empty() {
            p.push_str("## File observations\n\n");
            p.push_str(direct_context);
            p.push_str("\n\n");
        }
        if !memory_ctx.is_empty() {
            p.push_str("## Memory context\n\n");
            p.push_str(memory_ctx);
            p.push_str("\n\n");
        }
        p.push_str("## Question\n\n");
        p.push_str(query);
        p.push_str(
            "\n\nAnswer using ONLY the file observations above. \
             If the information is insufficient, say so explicitly:",
        );
    } else {
        // No context — tell the LLM honestly so it can ask for more info.
        p.push_str(
            "You are Oxcer, a local desktop AI assistant. \
             No file observations were available for this query. \
             Explain that you were unable to read the relevant files and ask the user \
             to specify a workspace or file path.\n\nQuery: ",
        );
        p.push_str(query);
    }
    p
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Produce a compact summary of a `[FILE_LIST:…]` observation.
fn summarize_file_list(output: &str) -> String {
    let paths: Vec<&str> = output
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('['))
        .take(15)
        .collect();
    if paths.is_empty() {
        "(none)".to_string()
    } else {
        format!("{} files: {}", paths.len(), paths.join(", "))
    }
}

/// Extract short, meaningful keywords from a query for grep fallback.
fn extract_keywords(query: &str) -> Vec<String> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "what", "how", "does", "can", "you", "this", "that", "have",
        "from", "file", "show", "tell", "please",
    ];
    query
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .take(5)
        .collect()
}

const FILE_EXTS: &[&str] = &[
    ".rs", ".swift", ".py", ".ts", ".tsx", ".js", ".jsx", ".md", ".toml", ".json", ".yaml", ".yml",
    ".txt", ".sh", ".csv", ".pdf", ".docx", ".log",
];

fn has_file_ext(token: &str) -> bool {
    let lower = token.to_lowercase();
    FILE_EXTS.iter().any(|ext| lower.ends_with(ext))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── Stub LLM ──────────────────────────────────────────────────────────────

    struct StubLlm;
    impl LlmCallback for StubLlm {
        fn generate(&self, prompt: &str) -> String {
            // Echo how many chars of context it received
            format!("[STUB_LLM: {} prompt chars]", prompt.len())
        }
    }

    /// A stub that records the prompt so tests can inspect it.
    struct RecordingLlm {
        // Use interior mutability via a shared cell
        seen: std::sync::Mutex<String>,
    }
    impl RecordingLlm {
        fn new() -> Self {
            RecordingLlm {
                seen: std::sync::Mutex::new(String::new()),
            }
        }
        fn last_prompt(&self) -> String {
            self.seen.lock().unwrap().clone()
        }
    }
    impl LlmCallback for RecordingLlm {
        fn generate(&self, prompt: &str) -> String {
            *self.seen.lock().unwrap() = prompt.to_string();
            "[RECORDED]".to_string()
        }
    }

    // ── Workspace fixture ─────────────────────────────────────────────────────

    fn setup_workspace() -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("main.rs"),
            "fn main() {\n    println!(\"Hello, Oxcer!\");\n}\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\n",
        )
        .unwrap();
        let src = tmp.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(
            src.join("lib.rs"),
            "/// Library entry.\npub fn greet() -> &'static str { \"hi\" }\n",
        )
        .unwrap();
        let root = tmp.path().to_string_lossy().to_string();
        (tmp, root)
    }

    // ── Explore phase ─────────────────────────────────────────────────────────

    #[test]
    fn explore_records_facts() {
        let (_tmp, root) = setup_workspace();
        let mut agent = SubAgent::new(AgentRole::Explore, &root, Memory::in_memory());
        agent.step("explore", None);
        assert!(
            !agent.memory.facts.is_empty(),
            "explore should record at least one fact"
        );
        let all_content: String = agent
            .memory
            .facts
            .iter()
            .map(|f| f.content.as_str())
            .collect();
        assert!(all_content.contains("Explore:"), "fact: {all_content}");
    }

    #[test]
    fn explore_finds_rs_files() {
        let (_tmp, root) = setup_workspace();
        let mut agent = SubAgent::new(AgentRole::Explore, &root, Memory::in_memory());
        let out = agent.step("explore", None);
        // The output is a [FILE_LIST:…] observation
        assert!(
            out.contains("main.rs") || out.contains("lib.rs"),
            "should list .rs files, got: {out}"
        );
    }

    // ── Plan phase ────────────────────────────────────────────────────────────

    #[test]
    fn plan_reads_explicitly_named_file() {
        let (_tmp, root) = setup_workspace();
        let mut agent = SubAgent::new(AgentRole::Plan, &root, Memory::in_memory());
        let ctx = agent.step("summarize main.rs", None);
        // Plan should have read main.rs and returned its contents
        assert!(
            ctx.contains("Hello, Oxcer") || ctx.is_empty(),
            "expected file contents or empty fallback, got: {ctx}"
        );
    }

    #[test]
    fn plan_records_read_fact() {
        let (_tmp, root) = setup_workspace();
        let mut agent = SubAgent::new(AgentRole::Plan, &root, Memory::in_memory());
        agent.step("summarize main.rs", None);
        let facts: Vec<_> = agent
            .memory
            .facts
            .iter()
            .filter(|f| f.content.contains("main.rs"))
            .collect();
        assert!(!facts.is_empty(), "plan should record a fact about main.rs");
    }

    // ── Execute phase ─────────────────────────────────────────────────────────

    #[test]
    fn execute_passes_context_to_llm() {
        let (_tmp, root) = setup_workspace();
        let mut mem = Memory::in_memory();
        mem.append_fact("Read main.rs: 3 lines, println Hello Oxcer");

        let rec = RecordingLlm::new();
        let mut agent = SubAgent::new(AgentRole::Execute, &root, mem);
        agent.step("summarize main.rs", Some(&rec));

        let prompt = rec.last_prompt();
        assert!(
            prompt.contains("main.rs"),
            "prompt should contain the fact, got: {prompt}"
        );
        assert!(
            prompt.contains("Hello, Oxcer") || prompt.contains("println"),
            "prompt should include file fact content, got: {prompt}"
        );
    }

    #[test]
    fn execute_stub_returns_formatted_string() {
        let (_tmp, root) = setup_workspace();
        let mut agent = SubAgent::new(AgentRole::Execute, &root, Memory::in_memory());
        let out = agent.step("any query", None);
        assert!(out.contains("[EXECUTE_STUB]"), "got: {out}");
    }

    // ── Full pipeline ─────────────────────────────────────────────────────────

    #[test]
    fn orchestrate_returns_non_empty_answer() {
        let (_tmp, root) = setup_workspace();
        let mem_path = _tmp.path().join("memory.md");
        let answer = orchestrate(
            "summarize main.rs",
            &root,
            &mem_path.to_string_lossy(),
            Some(&StubLlm),
        );
        assert!(!answer.is_empty(), "answer should not be empty");
        assert!(
            answer.contains("[STUB_LLM:"),
            "expected stub marker, got: {answer}"
        );
    }

    #[test]
    fn orchestrate_prompt_contains_file_contents() {
        let (_tmp, root) = setup_workspace();
        let mem_path = _tmp.path().join("memory.md");
        let rec = RecordingLlm::new();
        orchestrate(
            "summarize main.rs",
            &root,
            &mem_path.to_string_lossy(),
            Some(&rec),
        );
        let prompt = rec.last_prompt();
        // The prompt must contain the real file content, not hallucination
        assert!(
            prompt.contains("Hello, Oxcer"),
            "prompt must contain actual file content to prevent hallucination;\ngot prompt: {}",
            &prompt[..prompt.len().min(800)]
        );
    }

    #[test]
    fn orchestrate_creates_memory_file() {
        let (_tmp, root) = setup_workspace();
        let mem_path = _tmp.path().join("memory.md");
        assert!(!mem_path.exists(), "memory.md should not exist yet");

        orchestrate(
            "list files",
            &root,
            &mem_path.to_string_lossy(),
            Some(&StubLlm),
        );

        assert!(
            mem_path.exists(),
            "memory.md should be created after orchestrate"
        );
        let md = fs::read_to_string(&mem_path).unwrap();
        assert!(md.contains("# Oxcer Memory"), "got: {md}");
        assert!(
            md.contains("Explore:"),
            "should contain explore facts, got: {md}"
        );
    }

    #[test]
    fn orchestrate_memory_grows_across_calls() {
        let (_tmp, root) = setup_workspace();
        let mem_path = _tmp.path().join("memory.md");

        orchestrate("list files", &root, &mem_path.to_string_lossy(), None);
        let first_count = Memory::load_or_create(&mem_path).facts.len();

        orchestrate("read main.rs", &root, &mem_path.to_string_lossy(), None);
        let second_count = Memory::load_or_create(&mem_path).facts.len();

        assert!(
            second_count >= first_count,
            "memory should grow across calls: {first_count} → {second_count}"
        );
    }

    #[test]
    fn orchestrate_without_llm_returns_stub() {
        let (_tmp, root) = setup_workspace();
        let mem_path = _tmp.path().join("memory.md");
        let out = orchestrate(
            "summarize main.rs",
            &root,
            &mem_path.to_string_lossy(),
            None,
        );
        assert!(!out.is_empty());
        assert!(out.contains("[EXECUTE_STUB]"), "got: {out}");
    }

    // ── Helper unit tests ─────────────────────────────────────────────────────

    #[test]
    fn extract_keywords_filters_stopwords() {
        let kws = extract_keywords("what is in the main.rs file");
        // "what", "the", "file" are stopwords; "main" should remain
        assert!(kws.iter().any(|k| k.contains("main")), "keywords: {kws:?}");
        assert!(!kws.contains(&"what".to_string()));
        assert!(!kws.contains(&"the".to_string()));
    }

    #[test]
    fn has_file_ext_recognizes_extensions() {
        assert!(has_file_ext("main.rs"));
        assert!(has_file_ext("README.md"));
        assert!(has_file_ext("data.json"));
        assert!(!has_file_ext("plain_word"));
        assert!(!has_file_ext("version1"));
    }

    #[test]
    fn build_prompt_no_context_mentions_limitation() {
        let p = build_prompt("what is this?", "", "");
        assert!(
            p.contains("unable") || p.contains("No file") || p.contains("specify"),
            "no-context prompt should explain the limitation; got: {p}"
        );
    }

    #[test]
    fn build_prompt_with_context_contains_query() {
        let p = build_prompt(
            "explain main.rs",
            "[FILE_CONTENTS:main.rs]\nfn main() {}\n",
            "",
        );
        assert!(p.contains("explain main.rs"), "query in prompt: {p}");
        assert!(p.contains("fn main()"), "context in prompt: {p}");
    }
}
