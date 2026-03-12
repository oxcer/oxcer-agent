//! Semantic Router v1: cost-aware routing from task + context to strategy.
//!
//! First pass: deterministic heuristics only (no LLM).
//! Second pass (optional): borderline cases can be delegated to a small LLM
//! classifier via `route_task_with_classifier`.

use serde::{Deserialize, Serialize};

// -----------------------------------------------------------------------------
// Output types (Router API)
// -----------------------------------------------------------------------------

/// Task category for logging and strategy selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum TaskCategory {
    SimpleQa,
    Code,
    Planning,
    ToolsHeavy,
}

/// Which model (or none) to use.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Strategy {
    CheapModel,
    ExpensiveModel,
    ToolsOnly,
}

/// Flags the Orchestrator consumes (approval expectations, model/tools mixing).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RouterFlags {
    /// If true, orchestrator should expect REQUIRE_APPROVAL for high-risk tools.
    #[serde(default)]
    pub requires_high_risk_approval: bool,
    /// If true, orchestrator may interleave model calls and tool execution.
    #[serde(default)]
    pub allow_model_tools_mix: bool,
}

/// Result of the Semantic Router: category, strategy, and flags.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouterDecision {
    pub category: TaskCategory,
    pub strategy: Strategy,
    pub flags: RouterFlags,
    /// Plugin tool ids that match the task (when route_task_with_capabilities is used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_hints: Option<Vec<String>>,
}

/// Stable string for logging/telemetry: simple_qa | code | planning | tools_heavy.
pub fn category_for_log(c: TaskCategory) -> &'static str {
    match c {
        TaskCategory::SimpleQa => "simple_qa",
        TaskCategory::Code => "code",
        TaskCategory::Planning => "planning",
        TaskCategory::ToolsHeavy => "tools_heavy",
    }
}

/// Stable string for logging/telemetry: cheap_model | expensive_model | tools_only.
pub fn strategy_for_log(s: Strategy) -> &'static str {
    match s {
        Strategy::CheapModel => "cheap_model",
        Strategy::ExpensiveModel => "expensive_model",
        Strategy::ToolsOnly => "tools_only",
    }
}

// -----------------------------------------------------------------------------
// Input types
// -----------------------------------------------------------------------------

/// Context passed to the router (workspace, selection, risk hints).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TaskContext {
    /// Workspace id (if task is scoped to a workspace).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Selected file paths (relative or absolute) — hints "code" or "tools_heavy". Stored as strings for serialization.
    #[serde(default, alias = "selected_files")]
    pub selected_paths: Vec<String>,
    /// If true, caller indicates the task may involve high-risk tools (e.g. user said "delete", "rm -rf", "rename", "move", "chmod").
    #[serde(default)]
    pub risk_hints: bool,
}

/// System config that can influence routing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RouterConfig {
    /// If true, router may prefer tools_only when heuristics allow.
    #[serde(default)]
    pub prefer_tools_only: bool,
    /// Max task length (chars) above which we treat as "planning" (expensive).
    #[serde(default = "default_planning_threshold")]
    pub planning_length_threshold: usize,
    /// If true, borderline cases may be passed to the optional LLM classifier when using route_task_with_classifier.
    #[serde(default)]
    pub use_llm_for_borderline: bool,
}

fn default_planning_threshold() -> usize {
    800
}

/// Full input bundle (for Tauri/JSON boundary where task + context + config are one object).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RouterInput {
    pub task_description: String,
    #[serde(default)]
    pub context: TaskContext,
    #[serde(default)]
    pub config: RouterConfig,
    /// Optional plugin capabilities for tool_hints (when provided, route_task_with_capabilities is used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<crate::plugins::ToolCapability>>,
}

// -----------------------------------------------------------------------------
// Heuristics (first pass — deterministic, no LLM)
// -----------------------------------------------------------------------------

/// Explicit tool verbs -> ToolsHeavy + requires_high_risk_approval.
const TOOL_VERBS: &[&str] = &[
    "list files",
    "list dir",
    "open file",
    "read file",
    "write file",
    "delete",
    "remove",
    "rename",
    "move",
    "copy",
    "rm ",
    "mv ",
    "cp ",
    "shell command",
    "run script",
    "run command",
    "chmod",
    "execute",
];

/// Implicit FS verbs: natural-language phrases that imply a filesystem operation
/// even when the user doesn't use explicit "list files" / "read file" vocabulary.
const IMPLICIT_FS_VERBS: &[&str] = &[
    "what's in",
    "what is in",
    "whats in",
    "what does",
    "show me",
    "describe",
    "summarize",
    "summarise",
    "explain",
    "contents of",
    "what files",
    "list my",
    "browse",
    "overview of",
];

/// Directory / path hints: task must contain at least one of these alongside an
/// implicit FS verb for `has_implicit_fs_intent` to return `true`.
const FS_DIR_HINTS: &[&str] = &[
    "folder",
    "directory",
    "desktop",
    "documents",
    "downloads",
    "home",
];

/// File-content nouns: words that indicate the user is asking about a specific
/// file's contents (not a directory listing).  Used by `has_implicit_file_read_intent`.
const FILE_CONTENT_NOUNS: &[&str] = &[
    "paper",
    "document",
    "essay",
    "report",
    "article",
    "readme",
    "changelog",
    "the file",
    "this file",
    "a file",
    "that file",
    ".pdf",
    ".md",
    ".txt",
    ".docx",
    ".doc",
    ".csv",
    ".log",
];

/// Code markers -> Code; length then picks Cheap vs Expensive.
const CODE_MARKERS: &[&str] = &[
    "fn ",
    "fn(",
    "class ",
    "import ",
    "export ",
    "use ",
    "def ",
    ".rs",
    ".ts",
    ".py",
    ".js",
    ".tsx",
    ".swift",
    ".go",
    ".rs\"",
    ".ts\"",
    "function ",
    "impl ",
];

/// Planning keywords -> Planning.
const PLANNING_KEYWORDS: &[&str] = &[
    "plan",
    "steps",
    "strategy",
    "design",
    "architecture",
    "break down",
    "what order",
    "how should we",
    "first then finally",
];

fn contains_any_lower(s: &str, keywords: &[&str]) -> bool {
    let lower = s.to_lowercase();
    keywords.iter().any(|k| lower.contains(k))
}

fn has_tool_verbs(task: &str) -> bool {
    contains_any_lower(task, TOOL_VERBS)
}

/// Returns `true` when the task uses natural-language phrasing that implies a
/// filesystem operation without using explicit "list files" / "read file" verbs.
///
/// Both an implicit FS verb (e.g. "summarize", "show me") and a directory hint
/// (e.g. "folder", "Desktop", "Documents") must be present.
///
/// Examples that return `true`:
/// - "Please summarize my desktop folder"
/// - "What's in the Documents directory?"
/// - "Give me an overview of my Downloads"
/// - "Describe the contents of my home folder"
pub fn has_implicit_fs_intent(task: &str) -> bool {
    let lower = task.to_lowercase();
    let has_implicit_verb = IMPLICIT_FS_VERBS.iter().any(|v| lower.contains(v));
    let has_dir_hint = FS_DIR_HINTS.iter().any(|d| lower.contains(d));
    has_implicit_verb && has_dir_hint
}

/// Returns `true` when the task implies reading or summarizing a *specific file's
/// contents* (as opposed to listing a directory).
///
/// Fires when the task has a summary/explain verb AND a file-content noun, even
/// without a directory hint ("folder", "Desktop", etc.).  Tasks that already contain
/// a directory hint are left to `has_implicit_fs_intent` (checked first in `route_task`).
///
/// Examples that return `true`:
/// - "Summarize the paper on climate change"
/// - "Describe this document"
/// - "Explain the README"
/// - "What does the report say?"
///
/// Examples that return `false`:
/// - "Summarize my desktop folder"  ← has dir hint → handled by has_implicit_fs_intent
/// - "What is Rust?"                ← no file-content noun
pub fn has_implicit_file_read_intent(task: &str) -> bool {
    let lower = task.to_lowercase();
    // If a directory hint is present, delegate to has_implicit_fs_intent instead.
    if FS_DIR_HINTS.iter().any(|d| lower.contains(d)) {
        return false;
    }
    let has_verb = IMPLICIT_FS_VERBS.iter().any(|v| lower.contains(v));
    let has_noun = FILE_CONTENT_NOUNS.iter().any(|n| lower.contains(n));
    has_verb && has_noun
}

fn has_code_markers(task: &str) -> bool {
    contains_any_lower(task, CODE_MARKERS)
}

fn has_planning_keywords(task: &str) -> bool {
    contains_any_lower(task, PLANNING_KEYWORDS)
}

/// Short task with no file/tool verbs -> SimpleQa + CheapModel.
/// v1 conservative: "What is Rust?" etc. are treated as borderline (language/code-adjacent) and return false so they route to Planning.
fn looks_like_simple_qa(task: &str, context: &TaskContext) -> bool {
    let t = task.trim();
    if t.len() > 200 {
        return false;
    }
    if has_tool_verbs(t) || has_code_markers(t) || context.risk_hints {
        return false;
    }
    let lower = t.to_lowercase();
    // Borderline: language names often imply code context -> route to Planning.
    if lower.starts_with("what is ")
        && (lower.contains("rust") || lower.contains("python") || lower.contains("code"))
    {
        return false;
    }
    lower.ends_with('?')
        || lower.starts_with("what is ")
        || lower.starts_with("how do i ")
        || lower.starts_with("why ")
        || lower.starts_with("when ")
        || (lower.contains("explain") && !lower.contains("code"))
}

/// Routes with optional plugin capabilities. When capabilities are provided, populates
/// tool_hints for tools whose category_hint or tags match the task.
/// Uses CapabilityRegistry::matching_ids_for_task for indexed lookup when a registry is passed.
pub fn route_task_with_capabilities(
    task_description: &str,
    context: &TaskContext,
    config: &RouterConfig,
    capabilities: &[crate::plugins::ToolCapability],
) -> RouterDecision {
    let mut decision = route_task(task_description, context, config);
    let hints = crate::plugins::matching_ids_for_task(capabilities, task_description);
    if !hints.is_empty() {
        decision.tool_hints = Some(hints);
    }
    decision
}

/// Routes with a capability registry. Uses the registry's index for efficient lookup.
pub fn route_task_with_registry(
    task_description: &str,
    context: &TaskContext,
    config: &RouterConfig,
    registry: &crate::plugins::CapabilityRegistry,
) -> RouterDecision {
    let mut decision = route_task(task_description, context, config);
    let hints = registry.matching_ids_for_task(task_description);
    if !hints.is_empty() {
        decision.tool_hints = Some(hints);
    }
    decision
}

/// Public API: route using heuristics only. Borderline cases get a default (Code + CheapModel).
pub fn route_task(
    task_description: &str,
    context: &TaskContext,
    config: &RouterConfig,
) -> RouterDecision {
    let task = task_description.trim();
    let risk_hints = context.risk_hints || has_tool_verbs(task);

    let flags = RouterFlags {
        requires_high_risk_approval: risk_hints,
        allow_model_tools_mix: true, // default allow
    };

    // 1) Explicit tool verbs -> ToolsHeavy + requires_high_risk_approval
    if has_tool_verbs(task) {
        let strategy = if config.prefer_tools_only && task.len() < 300 {
            Strategy::ToolsOnly
        } else {
            Strategy::CheapModel
        };
        return RouterDecision {
            category: TaskCategory::ToolsHeavy,
            strategy,
            flags,
            tool_hints: None,
        };
    }

    // 1b) Implicit FS intent: "summarize my Desktop folder", "what's in Documents?" etc.
    // Route as ToolsHeavy + CheapModel so the orchestrator can build a two-step
    // FsListDir/FsReadFile → LlmGenerate(with real results) plan.
    if has_implicit_fs_intent(task) {
        return RouterDecision {
            category: TaskCategory::ToolsHeavy,
            strategy: Strategy::CheapModel,
            flags,
            tool_hints: None,
        };
    }

    // 1c) Implicit file-read intent: "summarize the paper", "describe this document", etc.
    // Does not require a directory hint — the file-content noun is sufficient.
    // The orchestrator uses the explicit path (if present) to build FsReadFile → LlmGenerate,
    // or falls back to FsListDir → LlmGenerate("identify file, don't fabricate content").
    if has_implicit_file_read_intent(task) {
        return RouterDecision {
            category: TaskCategory::ToolsHeavy,
            strategy: Strategy::CheapModel,
            flags,
            tool_hints: None,
        };
    }

    // 2) Many selected paths + short task -> ToolsHeavy
    if context.selected_paths.len() >= 3 && task.len() < 150 {
        let strategy = if config.prefer_tools_only {
            Strategy::ToolsOnly
        } else {
            Strategy::CheapModel
        };
        return RouterDecision {
            category: TaskCategory::ToolsHeavy,
            strategy,
            flags,
            tool_hints: None,
        };
    }

    // 3) Planning keywords or long task -> Planning + ExpensiveModel
    if task.len() >= config.planning_length_threshold || has_planning_keywords(task) {
        return RouterDecision {
            category: TaskCategory::Planning,
            strategy: Strategy::ExpensiveModel,
            flags,
            tool_hints: None,
        };
    }

    // 4) Code markers or selected_paths -> Code only when task is long enough (v1 conservative: short code-ish -> Planning).
    let code_related = has_code_markers(task) || !context.selected_paths.is_empty();
    if code_related && task.len() >= 25 {
        let strategy = if task.len() >= config.planning_length_threshold {
            Strategy::ExpensiveModel
        } else {
            Strategy::CheapModel
        };
        return RouterDecision {
            category: TaskCategory::Code,
            strategy,
            flags,
            tool_hints: None,
        };
    }

    // 5) Simple Q&A (short, no file/tool verbs); v1 conservative: e.g. "What is Rust?" treated as borderline -> Planning.
    if looks_like_simple_qa(task, context) {
        return RouterDecision {
            category: TaskCategory::SimpleQa,
            strategy: Strategy::CheapModel,
            flags,
            tool_hints: None,
        };
    }

    // 6) Borderline / default: Planning + ExpensiveModel (v1 conservative; caller can override via classifier).
    RouterDecision {
        category: TaskCategory::Planning,
        strategy: Strategy::ExpensiveModel,
        flags,
        tool_hints: None,
    }
}

/// Optional second pass: for borderline cases, call the classifier (e.g. small LLM that returns JSON `{ "category": "...", "strategy": "..." }`).
/// Heuristics run first; if the result is the default borderline (Planning + ExpensiveModel), the classifier is invoked.
/// The classifier should keep token budget small.
///
/// **Observability:** If the router uses a small LLM classifier, the caller should wrap that call with
/// additional `llm_client` telemetry (tokens, latency) so classification cost is tracked separately from main model calls.
pub fn route_task_with_classifier<F>(
    task_description: &str,
    context: &TaskContext,
    config: &RouterConfig,
    classifier: F,
) -> RouterDecision
where
    F: FnOnce(&str, &TaskContext, RouterDecision) -> RouterDecision,
{
    let heuristic = route_task(task_description, context, config);
    let task = task_description.trim();

    // Consider borderline: default fallback (no code/planning/tool markers, not simple_qa)
    let is_borderline = !has_tool_verbs(task)
        && !has_code_markers(task)
        && context.selected_paths.len() < 3
        && !has_planning_keywords(task)
        && task.len() < config.planning_length_threshold
        && !looks_like_simple_qa(task, context);

    if config.use_llm_for_borderline && is_borderline {
        classifier(task_description, context, heuristic)
    } else {
        heuristic
    }
}

// -----------------------------------------------------------------------------
/// Routes from a bundled RouterInput. Uses route_task_with_capabilities when capabilities are provided.
pub fn route_with_capabilities(input: &RouterInput) -> RouterDecision {
    match &input.capabilities {
        Some(caps) => route_task_with_capabilities(
            &input.task_description,
            &input.context,
            &input.config,
            caps,
        ),
        None => route_task(&input.task_description, &input.context, &input.config),
    }
}

// Compatibility: RouterInput-based entry and legacy names
// -----------------------------------------------------------------------------

/// Routes from a bundled RouterInput (e.g. from Tauri). Uses capabilities when present.
pub fn route(input: &RouterInput) -> RouterDecision {
    route_with_capabilities(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_empty() -> TaskContext {
        TaskContext::default()
    }

    /// POLICY (v1 conservative): Borderline short Q&A (e.g. "What is Rust?") is routed to Planning,
    /// not SimpleQa, because language names imply code context. SimpleQa reserved for clearly short/safe questions.
    #[test]
    fn route_task_simple_qa() {
        let out = route_task("What is Rust?", &ctx_empty(), &RouterConfig::default());
        assert_eq!(
            out.category,
            TaskCategory::Planning,
            "Conservative v1: route this borderline input to Planning, not SimpleQa"
        );
        assert_eq!(out.strategy, Strategy::ExpensiveModel);
    }

    #[test]
    fn route_task_tools_heavy_and_approval() {
        let out = route_task(
            "Delete the temp file and move src to backup",
            &ctx_empty(),
            &RouterConfig::default(),
        );
        assert_eq!(out.category, TaskCategory::ToolsHeavy);
        assert!(out.flags.requires_high_risk_approval);
    }

    /// POLICY (v1 conservative): Short code-ish prompt with one selected_path is borderline -> Planning.
    /// Code is reserved for clearly longer code tasks (task len ≥ 25) or multiple paths.
    #[test]
    fn route_task_code_cheap() {
        let mut ctx = TaskContext::default();
        ctx.selected_paths.push("src/main.rs".to_string());
        let out = route_task("Fix the bug in main.rs", &ctx, &RouterConfig::default());
        assert_eq!(
            out.category,
            TaskCategory::Planning,
            "Conservative v1: route this borderline input to Planning, not Code"
        );
        assert_eq!(out.strategy, Strategy::ExpensiveModel);
    }

    /// When task length ≥ planning_length_threshold, Planning takes precedence (checked before Code).
    #[test]
    fn route_task_planning_by_length_when_over_threshold() {
        let long_task = "We need to refactor the entire module. The function is too long and the class has too many responsibilities. We should split the implementation and add tests. Also consider the imports and exports.";
        let mut config = RouterConfig::default();
        config.planning_length_threshold = 100;
        let out = route_task(long_task, &ctx_empty(), &config);
        assert_eq!(out.category, TaskCategory::Planning);
        assert_eq!(out.strategy, Strategy::ExpensiveModel);
    }

    #[test]
    fn route_task_planning() {
        let out = route_task(
            "I need a plan and strategy for refactoring. Break down into steps.",
            &ctx_empty(),
            &RouterConfig::default(),
        );
        assert_eq!(out.category, TaskCategory::Planning);
        assert_eq!(out.strategy, Strategy::ExpensiveModel);
    }

    #[test]
    fn route_task_tools_only_prefer() {
        let out = route_task(
            "delete foo.txt",
            &ctx_empty(),
            &RouterConfig {
                prefer_tools_only: true,
                ..Default::default()
            },
        );
        assert_eq!(out.category, TaskCategory::ToolsHeavy);
        assert_eq!(out.strategy, Strategy::ToolsOnly);
    }

    #[test]
    fn route_task_with_classifier_borderline() {
        let mut config = RouterConfig::default();
        config.use_llm_for_borderline = true;
        let out = route_task_with_classifier(
            "Do something useful with the project",
            &ctx_empty(),
            &config,
            |_task, _ctx, suggested| RouterDecision {
                category: TaskCategory::Planning,
                strategy: Strategy::ExpensiveModel,
                flags: suggested.flags.clone(),
                tool_hints: Some(vec![]),
            },
        );
        assert_eq!(out.category, TaskCategory::Planning);
        assert_eq!(out.strategy, Strategy::ExpensiveModel);
    }

    /// POLICY (v1 conservative): route(&RouterInput) uses same heuristics as route_task. "What is Rust?"
    /// is borderline (language name) -> Planning, not SimpleQa.
    #[test]
    fn route_from_router_input() {
        let input = RouterInput {
            task_description: "What is Rust?".to_string(),
            context: TaskContext::default(),
            config: RouterConfig::default(),
            capabilities: None,
        };
        let out = route(&input);
        assert_eq!(
            out.category,
            TaskCategory::Planning,
            "Conservative v1: route this borderline input to Planning, not SimpleQa"
        );
        assert_eq!(out.strategy, Strategy::ExpensiveModel);
    }

    /// "delete file X" -> ToolsHeavy, high-risk flag, and with prefer_tools_only -> ToolsOnly.
    #[test]
    fn route_task_delete_file_x_tools_heavy_high_risk() {
        let out = route_task(
            "delete file foo.txt",
            &ctx_empty(),
            &RouterConfig::default(),
        );
        assert_eq!(out.category, TaskCategory::ToolsHeavy);
        assert!(out.flags.requires_high_risk_approval);

        let out_prefer = route_task(
            "delete file foo.txt",
            &ctx_empty(),
            &RouterConfig {
                prefer_tools_only: true,
                ..Default::default()
            },
        );
        assert_eq!(out_prefer.category, TaskCategory::ToolsHeavy);
        assert_eq!(out_prefer.strategy, Strategy::ToolsOnly);
    }

    /// Long multi-step task with "plan" language -> Planning + ExpensiveModel.
    #[test]
    fn route_task_long_plan_expensive() {
        let out = route_task(
            "I have a large codebase. I need a detailed plan: first we should design the architecture, then break down into steps, then implement each module. What strategy do you recommend?",
            &ctx_empty(),
            &RouterConfig::default(),
        );
        assert_eq!(out.category, TaskCategory::Planning);
        assert_eq!(out.strategy, Strategy::ExpensiveModel);
    }

    /// Implicit FS intents must route to ToolsHeavy + CheapModel so the orchestrator
    /// can build a two-step FsListDir → LlmGenerate plan instead of hallucinating.
    #[test]
    fn route_implicit_fs_intents_to_tools_heavy() {
        let cases = [
            "Please summarize my desktop folder",
            "What's in the Documents directory?",
            "Give me an overview of my Downloads",
            "Describe the contents of my home folder",
            "Show me what's in my Desktop",
        ];
        for task in &cases {
            let out = route_task(task, &ctx_empty(), &RouterConfig::default());
            assert_eq!(
                out.category,
                TaskCategory::ToolsHeavy,
                "implicit FS task should be ToolsHeavy: '{}'",
                task
            );
            assert_eq!(
                out.strategy,
                Strategy::CheapModel,
                "implicit FS task should use CheapModel: '{}'",
                task
            );
        }
    }

    #[test]
    fn has_implicit_fs_intent_positive_and_negative() {
        assert!(has_implicit_fs_intent("summarize my desktop folder"));
        assert!(has_implicit_fs_intent("what's in the Documents directory?"));
        assert!(has_implicit_fs_intent("describe my Downloads"));
        assert!(has_implicit_fs_intent("show me what's in my home folder"));
        // Negative: no directory hint
        assert!(!has_implicit_fs_intent("summarize the meeting"));
        // Negative: no implicit verb
        assert!(!has_implicit_fs_intent("delete the desktop folder"));
        // Negative: bare question
        assert!(!has_implicit_fs_intent("what is Rust?"));
    }

    /// Implicit file-read requests must route to ToolsHeavy + CheapModel so the
    /// orchestrator can build a FsReadFile/FsListDir → LlmGenerate plan instead of hallucinating.
    #[test]
    fn route_implicit_file_read_to_tools_heavy() {
        let cases = [
            "Summarize the paper on climate change",
            "Describe this document",
            "What does the report say?",
            "Give me an overview of that article",
            "Explain the README",
        ];
        for task in &cases {
            let out = route_task(task, &ctx_empty(), &RouterConfig::default());
            assert_eq!(
                out.category,
                TaskCategory::ToolsHeavy,
                "file-read task should be ToolsHeavy: '{}'",
                task
            );
            assert_eq!(
                out.strategy,
                Strategy::CheapModel,
                "file-read task should use CheapModel: '{}'",
                task
            );
        }
    }

    #[test]
    fn has_implicit_file_read_intent_positive_and_negative() {
        // Positive: verb + file-content noun, no directory hint
        assert!(has_implicit_file_read_intent(
            "Summarize the paper on climate change"
        ));
        assert!(has_implicit_file_read_intent("Describe this document"));
        assert!(has_implicit_file_read_intent("What does the report say?"));
        assert!(has_implicit_file_read_intent(
            "Give me an overview of that article"
        ));
        assert!(has_implicit_file_read_intent("Explain the README"));
        assert!(has_implicit_file_read_intent("Summarize paper.pdf"));
        // Negative: has a directory hint → handled by has_implicit_fs_intent
        assert!(!has_implicit_file_read_intent(
            "Summarize my desktop folder"
        ));
        assert!(!has_implicit_file_read_intent(
            "Show me what's in my Documents directory"
        ));
        // Negative: no file-content noun
        assert!(!has_implicit_file_read_intent("What is Rust?"));
        assert!(!has_implicit_file_read_intent("Summarize the meeting"));
    }
}
