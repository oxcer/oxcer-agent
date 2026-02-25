//! MCP (Model Context Protocol) Tool Suite for Oxcer.
//!
//! Provides Claude Code–style deterministic file tools that return structured
//! observation strings. All operations are **workspace-scoped**: path traversal
//! via `../..`, symlinks, and absolute escapes are blocked at `guard_path`.
//!
//! # Output format
//!
//! Every tool produces a bracketed observation string that can be injected
//! directly into an LLM prompt:
//!
//! ```text
//! [FILE_CONTENTS:src/main.rs]
//! fn main() { println!("hi"); }
//! [/FILE_CONTENTS] (2 lines)
//!
//! [SEARCH_RESULTS:fn ]
//! src/main.rs:1: fn main() {
//! [/SEARCH_RESULTS]
//!
//! [FILE_LIST:**/*.rs]
//! src/main.rs
//! src/lib.rs
//! [/FILE_LIST]
//!
//! [EDIT_RESULT:ok] Applied edit to src/main.rs
//! ```
//!
//! Errors are returned as `[MCP_ERROR: <message>]` so they flow into LLM context.

use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Limits ────────────────────────────────────────────────────────────────────

/// Maximum bytes for a single Read (4 MiB).
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum matching lines returned by Grep.
const MAX_GREP_LINES: usize = 500;
/// Maximum paths returned by Glob.
const MAX_GLOB_RESULTS: usize = 1000;
/// Directories skipped during recursive walks.
const SKIP_DIRS: &[&str] = &[
    ".git", ".build", "target", "node_modules", ".cargo", ".npm", ".yarn",
    "__pycache__", ".venv", "dist", "build",
];

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq)]
pub enum McpError {
    #[error("Path traversal blocked: {0}")]
    PathTraversal(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Regex error: {0}")]
    Regex(String),

    #[error("File too large: {0} bytes (max {1})")]
    FileTooLarge(u64, u64),

    #[error("Invalid tool JSON: {0}")]
    InvalidJson(String),
}

impl From<std::io::Error> for McpError {
    fn from(e: std::io::Error) -> Self {
        McpError::Io(e.to_string())
    }
}

// ── Tool enum ─────────────────────────────────────────────────────────────────

/// Structured tool call. JSON-serializable for FFI transport.
///
/// Serde tag `"tool"` maps to the variant name so the wire format is:
/// `{"tool":"read","path":"src/main.rs"}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum McpTool {
    /// Read a file's full text content.
    Read { path: String },

    /// Search files under `path` for a regex pattern.
    Grep { pattern: String, path: String },

    /// List files matching a glob under `base_dir` (default: workspace root).
    Glob {
        pattern: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        base_dir: Option<String>,
    },

    /// Replace the **first** occurrence of `old_text` with `new_text` in a file.
    Edit {
        path: String,
        old_text: String,
        new_text: String,
    },
}

// ── Executor ──────────────────────────────────────────────────────────────────

/// Workspace-scoped MCP tool executor.
///
/// All file operations are constrained to `workspace_root`. Attempts to escape
/// via `..`, absolute paths outside root, or symlinks return a path-traversal error.
pub struct McpExecutor {
    workspace_root: PathBuf,
}

impl McpExecutor {
    /// Create an executor. `workspace_root` must be an accessible directory.
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    /// Execute a tool. On error, returns `[MCP_ERROR: …]` so errors are LLM-visible.
    pub fn execute(&self, tool: McpTool) -> String {
        match self.execute_inner(tool) {
            Ok(s) => s,
            Err(e) => format!("[MCP_ERROR: {}]", e),
        }
    }

    /// Execute a tool described by a JSON string (for FFI transport).
    pub fn execute_json(&self, tool_json: &str) -> String {
        match serde_json::from_str::<McpTool>(tool_json) {
            Ok(tool) => self.execute(tool),
            Err(e) => format!("[MCP_ERROR: invalid tool JSON — {}]", e),
        }
    }

    // ── private dispatch ──────────────────────────────────────────────────────

    fn execute_inner(&self, tool: McpTool) -> Result<String, McpError> {
        match tool {
            McpTool::Read { path } => self.read(&path),
            McpTool::Grep { pattern, path } => self.grep(&pattern, &path),
            McpTool::Glob { pattern, base_dir } => self.glob_tool(&pattern, base_dir.as_deref()),
            McpTool::Edit {
                path,
                old_text,
                new_text,
            } => self.edit(&path, &old_text, &new_text),
        }
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    fn read(&self, path_str: &str) -> Result<String, McpError> {
        let safe = self.guard(path_str)?;
        let meta = fs::metadata(&safe)?;
        if meta.len() > MAX_READ_BYTES {
            return Err(McpError::FileTooLarge(meta.len(), MAX_READ_BYTES));
        }
        let content = fs::read_to_string(&safe)?;
        let lines = content.lines().count();
        Ok(format!(
            "[FILE_CONTENTS:{path}]\n{body}\n[/FILE_CONTENTS] ({lines} lines)",
            path = path_str,
            body = content.trim_end(),
        ))
    }

    // ── Grep ──────────────────────────────────────────────────────────────────

    fn grep(&self, pattern: &str, path_str: &str) -> Result<String, McpError> {
        let safe = self.guard(path_str)?;
        let re = Regex::new(pattern).map_err(|e| McpError::Regex(e.to_string()))?;
        let mut results: Vec<String> = Vec::new();
        self.grep_walk(&safe, &re, path_str, &mut results)?;

        let truncated = results.len() > MAX_GREP_LINES;
        results.truncate(MAX_GREP_LINES);

        if results.is_empty() {
            return Ok(format!(
                "[SEARCH_RESULTS:{pattern}] No matches in {path_str}"
            ));
        }

        let mut out = format!("[SEARCH_RESULTS:{pattern}]\n");
        for line in &results {
            out.push_str(line);
            out.push('\n');
        }
        if truncated {
            out.push_str(&format!("… (truncated at {} matches)\n", MAX_GREP_LINES));
        }
        out.push_str("[/SEARCH_RESULTS]");
        Ok(out)
    }

    fn grep_walk(
        &self,
        path: &Path,
        re: &Regex,
        display: &str,
        out: &mut Vec<String>,
    ) -> Result<(), McpError> {
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if SKIP_DIRS.contains(&name.as_ref()) {
                return Ok(());
            }
            let mut entries: Vec<_> = fs::read_dir(path)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let child = entry.path();
                let child_name = child.file_name().unwrap_or_default().to_string_lossy();
                let child_display = format!("{}/{}", display, child_name);
                self.grep_walk(&child, re, &child_display, out)?;
                if out.len() >= MAX_GREP_LINES {
                    break;
                }
            }
        } else if path.is_file() {
            if let Ok(text) = fs::read_to_string(path) {
                for (i, line) in text.lines().enumerate() {
                    if re.is_match(line) {
                        out.push(format!("{}:{}: {}", display, i + 1, line));
                        if out.len() >= MAX_GREP_LINES {
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // ── Glob ──────────────────────────────────────────────────────────────────

    fn glob_tool(&self, pattern: &str, base_dir: Option<&str>) -> Result<String, McpError> {
        let base = match base_dir {
            Some(d) => self.guard(d)?,
            None => {
                // Use canonicalized workspace root so guard_path checks work correctly.
                self.workspace_root
                    .canonicalize()
                    .unwrap_or_else(|_| self.workspace_root.clone())
            }
        };

        let re = glob_to_regex(pattern)?;
        let mut matches: Vec<String> = Vec::new();
        glob_walk(&base, &base, &re, &mut matches)?;
        matches.sort();
        let was_capped = matches.len() >= MAX_GLOB_RESULTS;
        matches.truncate(MAX_GLOB_RESULTS);

        if matches.is_empty() {
            return Ok(format!("[FILE_LIST:{pattern}] No files found"));
        }

        let cap_note = if was_capped {
            format!(" (first {})", MAX_GLOB_RESULTS)
        } else {
            String::new()
        };
        let mut out = format!("[FILE_LIST:{pattern}]{cap_note}\n");
        for p in &matches {
            out.push_str(p);
            out.push('\n');
        }
        out.push_str("[/FILE_LIST]");
        Ok(out)
    }

    // ── Edit ──────────────────────────────────────────────────────────────────

    fn edit(&self, path_str: &str, old_text: &str, new_text: &str) -> Result<String, McpError> {
        let safe = self.guard(path_str)?;
        let content = fs::read_to_string(&safe)?;
        if !content.contains(old_text) {
            return Ok(format!(
                "[EDIT_RESULT:not_found] Pattern not found in {path_str}"
            ));
        }
        let patched = content.replacen(old_text, new_text, 1);
        fs::write(&safe, patched)?;
        Ok(format!("[EDIT_RESULT:ok] Applied edit to {path_str}"))
    }

    // ── path guard ────────────────────────────────────────────────────────────

    fn guard(&self, rel_or_abs: &str) -> Result<PathBuf, McpError> {
        guard_path(&self.workspace_root, Path::new(rel_or_abs))
    }
}

// ── Free functions ────────────────────────────────────────────────────────────

/// Resolve `target` to an absolute path and verify it's inside `workspace_root`.
///
/// Relative paths are joined to the **canonicalized** workspace root so they are
/// resolved against the workspace, not the process CWD. This is important on
/// macOS where `/var/folders/…` and `/private/var/folders/…` refer to the same
/// directory but `Path::canonicalize` always returns the `/private/…` form.
pub fn guard_path(workspace_root: &Path, target: &Path) -> Result<PathBuf, McpError> {
    let canonical_root = workspace_root.canonicalize().map_err(|e| {
        McpError::PathTraversal(format!("workspace root not accessible: {}", e))
    })?;

    // Resolve relative paths against the canonical workspace root (not CWD).
    let abs_target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        canonical_root.join(target)
    };

    let canonical_target = if abs_target.exists() {
        abs_target
            .canonicalize()
            .map_err(|e| McpError::Io(e.to_string()))?
    } else {
        // File doesn't exist yet (e.g. Edit on a new path) — normalize lexically.
        normalize_lexical(&abs_target)
    };

    if !canonical_target.starts_with(&canonical_root) {
        return Err(McpError::PathTraversal(format!(
            "'{}' is outside workspace '{}'",
            canonical_target.display(),
            canonical_root.display(),
        )));
    }
    Ok(canonical_target)
}

/// Lexically normalize `path` — resolve `..` and `.` without touching the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut parts: Vec<_> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                parts.pop();
            }
            std::path::Component::CurDir => {}
            other => parts.push(other),
        }
    }
    parts.iter().collect()
}

/// Convert a glob pattern to a `Regex` for matching relative file paths.
///
/// Supported syntax:
/// - `**`  — matches any sequence of characters including `/`
/// - `*`   — matches any sequence except `/`
/// - `?`   — matches one character except `/`
/// - `{a,b,c}` — alternation (becomes `(a|b|c)`)
pub fn glob_to_regex(pattern: &str) -> Result<Regex, McpError> {
    let mut out = String::from("^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if i + 1 < chars.len() && chars[i + 1] == '*' => {
                out.push_str(".*");
                i += 2;
                // Consume optional trailing slash: `**/` → `.*`
                if i < chars.len() && chars[i] == '/' {
                    i += 1;
                }
            }
            '*' => {
                out.push_str("[^/]*");
                i += 1;
            }
            '?' => {
                out.push_str("[^/]");
                i += 1;
            }
            '.' => {
                out.push_str("\\.");
                i += 1;
            }
            '{' => {
                out.push('(');
                i += 1;
            }
            '}' => {
                out.push(')');
                i += 1;
            }
            ',' => {
                out.push('|');
                i += 1;
            }
            c if "^$+|[]\\()".contains(c) => {
                out.push('\\');
                out.push(c);
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out.push('$');
    Regex::new(&out).map_err(|e| McpError::Regex(e.to_string()))
}

/// Walk `dir` recursively, collecting paths relative to `base` that match `re`.
fn glob_walk(
    dir: &Path,
    base: &Path,
    re: &Regex,
    results: &mut Vec<String>,
) -> Result<(), McpError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let dir_name = dir.file_name().unwrap_or_default().to_string_lossy();
    if dir != base && SKIP_DIRS.contains(&dir_name.as_ref()) {
        return Ok(());
    }

    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| McpError::Io(e.to_string()))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let rel = path
            .strip_prefix(base)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/"); // Windows compatibility

        if path.is_file() && re.is_match(&rel) {
            results.push(rel);
        }
        if path.is_dir() {
            glob_walk(&path, base, re, results)?;
        }
        if results.len() >= MAX_GLOB_RESULTS {
            break;
        }
    }
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── Fixture ───────────────────────────────────────────────────────────────

    fn setup() -> (TempDir, McpExecutor) {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("hello.txt"), "line1\nline2\nline3\n").unwrap();
        fs::write(
            tmp.path().join("main.rs"),
            "fn main() {\n    println!(\"hi\");\n}\n",
        )
        .unwrap();
        let src = tmp.path().join("src");
        fs::create_dir(&src).unwrap();
        fs::write(
            src.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
        )
        .unwrap();
        let exec = McpExecutor::new(tmp.path().to_path_buf());
        (tmp, exec)
    }

    // ── Read ──────────────────────────────────────────────────────────────────

    #[test]
    fn read_returns_file_contents() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Read {
            path: "hello.txt".to_string(),
        });
        assert!(out.starts_with("[FILE_CONTENTS:hello.txt]"), "got: {out}");
        assert!(out.contains("line1"), "got: {out}");
        assert!(out.contains("(3 lines)"), "got: {out}");
        assert!(out.contains("[/FILE_CONTENTS]"), "got: {out}");
    }

    #[test]
    fn read_subdirectory_file() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Read {
            path: "src/lib.rs".to_string(),
        });
        assert!(out.contains("[FILE_CONTENTS:src/lib.rs]"), "got: {out}");
        assert!(out.contains("pub fn add"), "got: {out}");
    }

    #[test]
    fn read_blocks_path_traversal() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Read {
            path: "../../etc/passwd".to_string(),
        });
        assert!(out.starts_with("[MCP_ERROR:"), "expected error, got: {out}");
    }

    #[test]
    fn read_missing_file_returns_error() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Read {
            path: "nonexistent.txt".to_string(),
        });
        assert!(out.starts_with("[MCP_ERROR:"), "got: {out}");
    }

    // ── Grep ──────────────────────────────────────────────────────────────────

    #[test]
    fn grep_finds_matches_in_tree() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Grep {
            pattern: "fn ".to_string(),
            path: ".".to_string(),
        });
        assert!(out.starts_with("[SEARCH_RESULTS:fn ]"), "got: {out}");
        assert!(out.contains("main.rs"), "got: {out}");
        assert!(out.contains("[/SEARCH_RESULTS]"), "got: {out}");
    }

    #[test]
    fn grep_finds_in_subdir() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Grep {
            pattern: "pub fn".to_string(),
            path: "src".to_string(),
        });
        assert!(out.contains("lib.rs"), "got: {out}");
    }

    #[test]
    fn grep_no_matches_message() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Grep {
            pattern: "xyz_no_match_xyz_4921".to_string(),
            path: ".".to_string(),
        });
        assert!(out.contains("No matches"), "got: {out}");
    }

    #[test]
    fn grep_invalid_regex_returns_error() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Grep {
            pattern: "[invalid".to_string(),
            path: ".".to_string(),
        });
        assert!(out.starts_with("[MCP_ERROR:"), "got: {out}");
    }

    // ── Glob ──────────────────────────────────────────────────────────────────

    #[test]
    fn glob_finds_all_rs_files() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Glob {
            pattern: "**/*.rs".to_string(),
            base_dir: None,
        });
        assert!(out.starts_with("[FILE_LIST:**/*.rs]"), "got: {out}");
        assert!(out.contains("main.rs"), "got: {out}");
        assert!(out.contains("lib.rs"), "got: {out}"); // in src/
        assert!(out.contains("[/FILE_LIST]"), "got: {out}");
    }

    #[test]
    fn glob_root_pattern_excludes_subdirs() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Glob {
            pattern: "*.txt".to_string(),
            base_dir: None,
        });
        assert!(out.contains("hello.txt"), "got: {out}");
        // src/lib.rs should not appear
        assert!(!out.contains("lib.rs"), "should not include subdir files: {out}");
    }

    #[test]
    fn glob_brace_alternation() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Glob {
            pattern: "**/*.{rs,txt}".to_string(),
            base_dir: None,
        });
        assert!(out.contains("main.rs"), "got: {out}");
        assert!(out.contains("hello.txt"), "got: {out}");
    }

    #[test]
    fn glob_no_match_message() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Glob {
            pattern: "**/*.py".to_string(),
            base_dir: None,
        });
        assert!(out.contains("No files found"), "got: {out}");
    }

    // ── Edit ──────────────────────────────────────────────────────────────────

    #[test]
    fn edit_applies_first_replacement() {
        let (tmp, exec) = setup();
        let out = exec.execute(McpTool::Edit {
            path: "hello.txt".to_string(),
            old_text: "line1".to_string(),
            new_text: "REPLACED".to_string(),
        });
        assert!(out.contains("[EDIT_RESULT:ok]"), "got: {out}");
        let body = fs::read_to_string(tmp.path().join("hello.txt")).unwrap();
        assert!(body.contains("REPLACED"), "file not updated: {body}");
        assert!(!body.contains("line1"), "old text not removed: {body}");
        assert!(body.contains("line2"), "other lines untouched: {body}");
    }

    #[test]
    fn edit_not_found_does_not_modify() {
        let (tmp, exec) = setup();
        let original = fs::read_to_string(tmp.path().join("hello.txt")).unwrap();
        let out = exec.execute(McpTool::Edit {
            path: "hello.txt".to_string(),
            old_text: "this_does_not_exist".to_string(),
            new_text: "new".to_string(),
        });
        assert!(out.contains("[EDIT_RESULT:not_found]"), "got: {out}");
        let after = fs::read_to_string(tmp.path().join("hello.txt")).unwrap();
        assert_eq!(original, after, "file must be unchanged");
    }

    #[test]
    fn edit_blocks_traversal() {
        let (_tmp, exec) = setup();
        let out = exec.execute(McpTool::Edit {
            path: "../../../etc/passwd".to_string(),
            old_text: "root".to_string(),
            new_text: "evil".to_string(),
        });
        assert!(out.starts_with("[MCP_ERROR:"), "got: {out}");
    }

    // ── JSON dispatch ─────────────────────────────────────────────────────────

    #[test]
    fn execute_json_reads_file() {
        let (_tmp, exec) = setup();
        let json = r#"{"tool":"read","path":"hello.txt"}"#;
        let out = exec.execute_json(json);
        assert!(out.contains("[FILE_CONTENTS:hello.txt]"), "got: {out}");
    }

    #[test]
    fn execute_json_invalid_returns_error() {
        let (_tmp, exec) = setup();
        let out = exec.execute_json("{not valid json");
        assert!(out.starts_with("[MCP_ERROR:"), "got: {out}");
    }

    #[test]
    fn execute_json_glob() {
        let (_tmp, exec) = setup();
        let json = r#"{"tool":"glob","pattern":"**/*.rs"}"#;
        let out = exec.execute_json(json);
        assert!(out.contains("[FILE_LIST:**/*.rs]"), "got: {out}");
    }

    // ── Glob regex ────────────────────────────────────────────────────────────

    #[test]
    fn glob_to_regex_star_star_slash() {
        let re = glob_to_regex("**/*.rs").unwrap();
        assert!(re.is_match("main.rs"), "root-level file");
        assert!(re.is_match("src/main.rs"), "one dir");
        assert!(re.is_match("src/sub/main.rs"), "two dirs");
        assert!(!re.is_match("main.txt"), "wrong extension");
    }

    #[test]
    fn glob_to_regex_star_only() {
        let re = glob_to_regex("*.txt").unwrap();
        assert!(re.is_match("hello.txt"));
        assert!(!re.is_match("src/hello.txt"), "star doesn't cross /");
        assert!(!re.is_match("hello.rs"));
    }

    #[test]
    fn glob_to_regex_brace_alternation() {
        let re = glob_to_regex("**/*.{rs,toml}").unwrap();
        assert!(re.is_match("Cargo.toml"));
        assert!(re.is_match("src/lib.rs"));
        assert!(!re.is_match("src/lib.py"));
    }
}
