//! Fail-fast output validation for the FSM agent.
//!
//! If the LLM produces output that does not match the expected schema,
//! `validate_action_selection` returns `Err(AgenticError::HallucinationDetected)`.
//! There is no re-prompting — the caller surfaces the error immediately.

use crate::db::DbError;
use crate::executor::{ExecutorError, ToolCall};
use std::path::PathBuf;
use thiserror::Error;

/// All errors that can terminate the FSM agent loop.
#[derive(Debug, Error)]
pub enum AgenticError {
    /// LLM produced output that does not match the required tool schema.
    #[error("hallucination detected: {0}")]
    HallucinationDetected(String),

    /// LLM returned an empty response.
    #[error("LLM returned an empty response")]
    EmptyResponse,

    /// Output failed a post-generation validation rule.
    #[error("validation failed: {0}")]
    ValidationFailed(String),

    /// Propagated from the database layer.
    #[error("database error: {0}")]
    DbError(#[from] DbError),

    /// Propagated from the executor layer.
    #[error("executor error: {0}")]
    ExecutorError(#[from] ExecutorError),

    /// The FSM exhausted its step budget without reaching `Finalize`.
    #[error("step limit of {0} exceeded without reaching Finalize")]
    StepLimitExceeded(usize),
}

/// The parsed result of an `ActionSelection` LLM call.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionSpec {
    /// The LLM indicated no tool is needed (`[NO_TOOL]`).
    NoTool,
    /// The LLM named a specific tool with its arguments.
    Tool(ToolCall),
}

/// Parse the LLM's `ActionSelection` output into an `ActionSpec`.
///
/// # Expected formats
///
/// ```text
/// fs_list_dir <path>
/// fs_read_file <path>
/// read_document <path>
/// move_file <source> | <dest>
/// trash_file <path>
/// shell_run <cmd> [arg1] [arg2] ...
/// [NO_TOOL]
/// ```
///
/// Any other format is a hallucination and returns
/// `Err(AgenticError::HallucinationDetected)`.
pub fn validate_action_selection(output: &str) -> Result<ActionSpec, AgenticError> {
    let trimmed = output.trim();

    if trimmed.is_empty() {
        return Err(AgenticError::EmptyResponse);
    }

    if trimmed == "[NO_TOOL]" {
        return Ok(ActionSpec::NoTool);
    }

    // Split on whitespace; first token is the tool name.
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let tool_name = parts.next().unwrap_or("").trim();
    let rest = parts.next().unwrap_or("").trim();

    match tool_name {
        "fs_list_dir" => {
            if rest.is_empty() {
                return Err(AgenticError::HallucinationDetected(
                    "fs_list_dir requires a path argument".to_string(),
                ));
            }
            Ok(ActionSpec::Tool(ToolCall::FsListDir(PathBuf::from(rest))))
        }

        "fs_read_file" => {
            if rest.is_empty() {
                return Err(AgenticError::HallucinationDetected(
                    "fs_read_file requires a path argument".to_string(),
                ));
            }
            Ok(ActionSpec::Tool(ToolCall::FsReadFile(PathBuf::from(rest))))
        }

        "read_document" => {
            if rest.is_empty() {
                return Err(AgenticError::HallucinationDetected(
                    "read_document requires a path argument".to_string(),
                ));
            }
            Ok(ActionSpec::Tool(ToolCall::ReadDocument(PathBuf::from(
                rest,
            ))))
        }

        "move_file" => {
            // Expected format: move_file <source> | <dest>
            // The pipe character separates source and destination.
            let mut pipe_parts = rest.splitn(2, '|');
            let src = pipe_parts.next().unwrap_or("").trim();
            let dst = pipe_parts.next().unwrap_or("").trim();
            if src.is_empty() || dst.is_empty() {
                return Err(AgenticError::HallucinationDetected(
                    "move_file requires 'source | dest' format, e.g.: move_file a.txt | b.txt"
                        .to_string(),
                ));
            }
            Ok(ActionSpec::Tool(ToolCall::MoveFile {
                source: PathBuf::from(src),
                dest: PathBuf::from(dst),
            }))
        }

        "trash_file" => {
            if rest.is_empty() {
                return Err(AgenticError::HallucinationDetected(
                    "trash_file requires a path argument".to_string(),
                ));
            }
            Ok(ActionSpec::Tool(ToolCall::TrashFile(PathBuf::from(rest))))
        }

        "shell_run" => {
            if rest.is_empty() {
                return Err(AgenticError::HallucinationDetected(
                    "shell_run requires at least a command name".to_string(),
                ));
            }
            let mut cmd_parts = rest.splitn(2, char::is_whitespace);
            let command = cmd_parts.next().unwrap_or("").trim().to_string();
            let args_str = cmd_parts.next().unwrap_or("").trim();
            let args: Vec<String> = if args_str.is_empty() {
                vec![]
            } else {
                args_str.split_whitespace().map(str::to_string).collect()
            };
            Ok(ActionSpec::Tool(ToolCall::ShellRun { command, args }))
        }

        other => Err(AgenticError::HallucinationDetected(format!(
            "unknown tool '{other}'; expected: fs_list_dir, fs_read_file, read_document, \
             move_file, trash_file, shell_run, or [NO_TOOL]"
        ))),
    }
}

/// Validate the LLM's final answer for obvious failure signals.
///
/// Returns the trimmed answer on success, or `Err` if the answer is empty or
/// begins with `[ERROR:`.
pub fn validate_final_answer(output: &str) -> Result<String, AgenticError> {
    let trimmed = output.trim().to_string();

    if trimmed.is_empty() {
        return Err(AgenticError::EmptyResponse);
    }

    if trimmed.starts_with("[ERROR:") {
        return Err(AgenticError::ValidationFailed(format!(
            "LLM returned an error token: {trimmed}"
        )));
    }

    Ok(trimmed)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_action_selection ────────────────────────────────────────────

    #[test]
    fn no_tool_literal() {
        let spec = validate_action_selection("[NO_TOOL]").unwrap();
        assert_eq!(spec, ActionSpec::NoTool);
    }

    #[test]
    fn fs_list_dir_with_path() {
        let spec = validate_action_selection("fs_list_dir /tmp/workspace").unwrap();
        assert!(matches!(spec, ActionSpec::Tool(ToolCall::FsListDir(_))));
        if let ActionSpec::Tool(ToolCall::FsListDir(p)) = spec {
            assert_eq!(p, PathBuf::from("/tmp/workspace"));
        }
    }

    #[test]
    fn fs_list_dir_missing_path_is_hallucination() {
        let err = validate_action_selection("fs_list_dir").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn fs_read_file_with_path() {
        let spec = validate_action_selection("fs_read_file src/main.rs").unwrap();
        assert!(matches!(spec, ActionSpec::Tool(ToolCall::FsReadFile(_))));
        if let ActionSpec::Tool(ToolCall::FsReadFile(p)) = spec {
            assert_eq!(p, PathBuf::from("src/main.rs"));
        }
    }

    #[test]
    fn fs_read_file_missing_path_is_hallucination() {
        let err = validate_action_selection("fs_read_file").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn shell_run_command_only() {
        let spec = validate_action_selection("shell_run ls").unwrap();
        if let ActionSpec::Tool(ToolCall::ShellRun { command, args }) = spec {
            assert_eq!(command, "ls");
            assert!(args.is_empty());
        } else {
            panic!("expected ShellRun");
        }
    }

    #[test]
    fn shell_run_command_with_args() {
        let spec = validate_action_selection("shell_run grep -r pattern src/").unwrap();
        if let ActionSpec::Tool(ToolCall::ShellRun { command, args }) = spec {
            assert_eq!(command, "grep");
            assert_eq!(args, vec!["-r", "pattern", "src/"]);
        } else {
            panic!("expected ShellRun");
        }
    }

    #[test]
    fn shell_run_missing_command_is_hallucination() {
        let err = validate_action_selection("shell_run").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn read_document_with_path() {
        let spec = validate_action_selection("read_document report.pdf").unwrap();
        if let ActionSpec::Tool(ToolCall::ReadDocument(p)) = spec {
            assert_eq!(p, PathBuf::from("report.pdf"));
        } else {
            panic!("expected ReadDocument");
        }
    }

    #[test]
    fn read_document_missing_path_is_hallucination() {
        let err = validate_action_selection("read_document").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn move_file_pipe_format() {
        let spec = validate_action_selection("move_file docs/old.txt | docs/new.txt").unwrap();
        if let ActionSpec::Tool(ToolCall::MoveFile { source, dest }) = spec {
            assert_eq!(source, PathBuf::from("docs/old.txt"));
            assert_eq!(dest, PathBuf::from("docs/new.txt"));
        } else {
            panic!("expected MoveFile");
        }
    }

    #[test]
    fn move_file_missing_dest_is_hallucination() {
        let err = validate_action_selection("move_file old.txt").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn move_file_empty_src_is_hallucination() {
        let err = validate_action_selection("move_file | dest.txt").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn trash_file_with_path() {
        let spec = validate_action_selection("trash_file old_report.docx").unwrap();
        if let ActionSpec::Tool(ToolCall::TrashFile(p)) = spec {
            assert_eq!(p, PathBuf::from("old_report.docx"));
        } else {
            panic!("expected TrashFile");
        }
    }

    #[test]
    fn trash_file_missing_path_is_hallucination() {
        let err = validate_action_selection("trash_file").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn unknown_tool_is_hallucination() {
        let err = validate_action_selection("browse_web https://example.com").unwrap_err();
        assert!(matches!(err, AgenticError::HallucinationDetected(_)));
    }

    #[test]
    fn empty_output_is_empty_response() {
        let err = validate_action_selection("   ").unwrap_err();
        assert!(matches!(err, AgenticError::EmptyResponse));
    }

    #[test]
    fn whitespace_before_no_tool_is_accepted() {
        let spec = validate_action_selection("  [NO_TOOL]  ").unwrap();
        assert_eq!(spec, ActionSpec::NoTool);
    }

    // ── validate_final_answer ────────────────────────────────────────────────

    #[test]
    fn valid_answer_is_returned_trimmed() {
        let ans = validate_final_answer("  The answer is 42.  ").unwrap();
        assert_eq!(ans, "The answer is 42.");
    }

    #[test]
    fn empty_answer_is_error() {
        let err = validate_final_answer("").unwrap_err();
        assert!(matches!(err, AgenticError::EmptyResponse));
    }

    #[test]
    fn error_token_prefix_is_rejected() {
        let err = validate_final_answer("[ERROR: LLM failed to generate]").unwrap_err();
        assert!(matches!(err, AgenticError::ValidationFailed(_)));
    }

    #[test]
    fn answer_containing_error_word_without_prefix_is_accepted() {
        // "error" inside the answer is fine; only the `[ERROR:` prefix is banned.
        let ans = validate_final_answer("There was an error in the code at line 42.").unwrap();
        assert!(ans.contains("error"));
    }
}
