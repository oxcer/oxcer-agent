//! ReAct-style deterministic terminal executor.
//!
//! Parses an `Action: <cmd>` directive from LLM output and executes the
//! corresponding shell command, returning a `[LS_OUTPUT: …]` fact string that
//! can be injected back into the LLM context on the next turn.
//!
//! Only `ls` is supported. Any other action returns [`TerminalError::UnsupportedAction`].

use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;
use thiserror::Error;

// ── Errors ────────────────────────────────────────────────────────────────────

#[derive(Debug, Error, PartialEq)]
pub enum TerminalError {
    #[error("No 'Action:' directive found in LLM output")]
    NoActionFound,

    #[error("Unsupported action '{0}' — only 'ls' is currently allowed")]
    UnsupportedAction(String),

    #[error("Command execution failed: {0}")]
    CommandFailed(String),
}

// ── TerminalExecutor ──────────────────────────────────────────────────────────

/// Stateless executor. All methods are static; no instance state.
pub struct TerminalExecutor;

impl TerminalExecutor {
    /// Parse `llm_output` for an `Action: ls` directive and run `ls -a <dir>`.
    ///
    /// * `llm_output` — raw text from the LLM (may include prose before/after the action line)
    /// * `working_dir` — directory to list; pass `None` to use the process CWD
    ///
    /// On success returns `"[LS_OUTPUT: <entries…>]"`.
    pub fn execute_llm_action(
        llm_output: &str,
        working_dir: Option<&str>,
    ) -> Result<String, TerminalError> {
        // Compile the regex once per process (OnceLock = zero-cost after first call).
        static ACTION_RE: OnceLock<Regex> = OnceLock::new();
        let re = ACTION_RE.get_or_init(|| {
            Regex::new(r"(?i)Action:\s*(\S+)").expect("hardcoded regex is valid")
        });

        let cap = re.captures(llm_output).ok_or(TerminalError::NoActionFound)?;
        let action = cap.get(1).unwrap().as_str();

        match action.to_lowercase().as_str() {
            "ls" => Self::run_ls(working_dir),
            other => Err(TerminalError::UnsupportedAction(other.to_string())),
        }
    }

    // ── private ───────────────────────────────────────────────────────────────

    fn run_ls(dir: Option<&str>) -> Result<String, TerminalError> {
        let mut cmd = Command::new("ls");
        cmd.arg("-a");

        if let Some(d) = dir {
            cmd.arg(d);
        } else {
            cmd.arg(".");
        }

        let output = cmd
            .output()
            .map_err(|e| TerminalError::CommandFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(TerminalError::CommandFailed(if stderr.is_empty() {
                format!("ls exited with status {}", output.status)
            } else {
                stderr
            }));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(format!("[LS_OUTPUT: {}]", stdout))
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ls_action_and_returns_output() {
        // Uses the process CWD (oxcer repo root during `cargo test`).
        let result = TerminalExecutor::execute_llm_action("Action: ls", None);
        assert!(result.is_ok(), "ls should succeed: {result:?}");
        let out = result.unwrap();
        assert!(out.starts_with("[LS_OUTPUT:"), "expected prefix, got: {out}");
        assert!(out.ends_with(']'), "expected closing bracket, got: {out}");
    }

    #[test]
    fn parses_ls_action_case_insensitive() {
        let result = TerminalExecutor::execute_llm_action("Action: LS", None);
        assert!(result.is_ok(), "LS (uppercase) should succeed: {result:?}");
    }

    #[test]
    fn ls_with_explicit_dir() {
        let dir = std::env::temp_dir();
        let dir_str = dir.to_string_lossy();
        let result =
            TerminalExecutor::execute_llm_action("Action: ls", Some(dir_str.as_ref()));
        assert!(result.is_ok(), "ls /tmp should succeed: {result:?}");
        let out = result.unwrap();
        assert!(out.starts_with("[LS_OUTPUT:"), "{out}");
    }

    #[test]
    fn returns_err_when_no_action_directive() {
        let result =
            TerminalExecutor::execute_llm_action("Just some prose without any action", None);
        assert_eq!(result, Err(TerminalError::NoActionFound));
    }

    #[test]
    fn returns_err_for_unsupported_action() {
        let result = TerminalExecutor::execute_llm_action("Action: rm", None);
        assert!(matches!(result, Err(TerminalError::UnsupportedAction(_))));
    }

    #[test]
    fn returns_err_for_unsupported_action_curl() {
        let result = TerminalExecutor::execute_llm_action("Action: curl http://example.com", None);
        // "curl" — unsupported
        assert!(matches!(result, Err(TerminalError::UnsupportedAction(_))));
    }

    #[test]
    fn prose_before_action_is_ignored() {
        let llm_output = "I will now list the files.\nThought: I should use ls.\nAction: ls";
        let result = TerminalExecutor::execute_llm_action(llm_output, None);
        assert!(result.is_ok(), "should parse action from multi-line output: {result:?}");
    }

    #[test]
    fn returns_err_for_nonexistent_dir() {
        let result = TerminalExecutor::execute_llm_action(
            "Action: ls",
            Some("/definitely/does/not/exist/abc123"),
        );
        assert!(matches!(result, Err(TerminalError::CommandFailed(_))));
    }
}
