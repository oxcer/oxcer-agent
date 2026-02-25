//! Direct tool execution for the FSM agent.
//!
//! `UniversalExecutor` runs native FS operations and shell commands with a
//! workspace-scoped path guard. No serialization layer — callers work with
//! typed `ToolCall` values.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors produced by `UniversalExecutor`.
#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("path traversal denied: '{0}' escapes workspace")]
    PathTraversal(String),

    #[error("shell command not in allowlist: '{0}'")]
    CommandNotAllowed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("command failed (exit {code}): {stderr}")]
    CommandFailed { code: i32, stderr: String },

    #[error("workspace root does not exist: '{0}'")]
    WorkspaceNotFound(String),

    #[error("unsupported document format '{0}': supported types are .txt .pdf .docx .xlsx")]
    UnsupportedFormat(String),

    #[error("document parse failed: {0}")]
    DocumentParse(String),

    #[error("trash error: {0}")]
    Trash(String),
}

/// Shell commands that agents may execute.
const ALLOWED_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "find", "grep", "wc", "echo", "pwd",
];

/// Maximum byte length returned from `read_document`. Content beyond this
/// threshold is truncated with a visible warning so the LLM context is not flooded.
const READ_DOCUMENT_MAX_BYTES: usize = 100 * 1024; // 100 KB

/// A typed tool invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    /// List the contents of a directory.
    FsListDir(PathBuf),
    /// Read the text content of a file.
    FsReadFile(PathBuf),
    /// Run a shell command with zero or more arguments.
    ShellRun {
        command: String,
        args: Vec<String>,
    },
    /// Extract and return text from a TXT / PDF / DOCX / XLSX document.
    ReadDocument(PathBuf),
    /// Move a file from `source` to `dest` within the workspace.
    MoveFile { source: PathBuf, dest: PathBuf },
    /// Move a file or directory to the OS trash (never permanently deletes).
    TrashFile(PathBuf),
}

/// Executes `ToolCall` values within a workspace boundary.
pub struct UniversalExecutor {
    workspace_root: PathBuf,
}

impl UniversalExecutor {
    /// Create an executor rooted at `workspace_root`.
    ///
    /// Returns `Err` if the directory does not exist.
    pub fn new(workspace_root: impl AsRef<Path>) -> Result<Self, ExecutorError> {
        let root = workspace_root.as_ref();
        if !root.exists() {
            return Err(ExecutorError::WorkspaceNotFound(
                root.display().to_string(),
            ));
        }
        Ok(Self {
            workspace_root: root.to_path_buf(),
        })
    }

    /// Execute `tool` and return its output as a UTF-8 string.
    pub fn execute(&self, tool: &ToolCall) -> Result<String, ExecutorError> {
        match tool {
            ToolCall::FsListDir(path) => self.exec_list_dir(path),
            ToolCall::FsReadFile(path) => self.exec_read_file(path),
            ToolCall::ShellRun { command, args } => self.exec_shell(command, args),
            ToolCall::ReadDocument(path) => self.exec_read_document(path),
            ToolCall::MoveFile { source, dest } => self.exec_move_file(source, dest),
            ToolCall::TrashFile(path) => self.exec_trash_file(path),
        }
    }

    /// Resolve and boundary-check `path` against the workspace root.
    ///
    /// Exposed so the FSM can compute derived output paths (e.g. `.summary.md`)
    /// from the same canonical form that `execute` uses internally.
    pub fn resolve_path(&self, path: &Path) -> Result<PathBuf, ExecutorError> {
        self.guard_path(path)
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    fn exec_list_dir(&self, path: &Path) -> Result<String, ExecutorError> {
        let guarded = self.guard_path(path)?;
        let mut entries: Vec<String> = std::fs::read_dir(&guarded)?
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                if e.path().is_dir() {
                    format!("{name}/")
                } else {
                    name
                }
            })
            .collect();
        entries.sort_unstable();
        if entries.is_empty() {
            Ok("(empty directory)".to_string())
        } else {
            Ok(entries.join("\n"))
        }
    }

    fn exec_read_file(&self, path: &Path) -> Result<String, ExecutorError> {
        let guarded = self.guard_path(path)?;
        let content = std::fs::read_to_string(&guarded)?;
        Ok(content)
    }

    fn exec_shell(&self, command: &str, args: &[String]) -> Result<String, ExecutorError> {
        // Allowlist check — reject anything not on the safe list.
        if !ALLOWED_COMMANDS.contains(&command) {
            return Err(ExecutorError::CommandNotAllowed(command.to_string()));
        }

        let output = std::process::Command::new(command)
            .args(args)
            .current_dir(&self.workspace_root)
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            let code = output.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Err(ExecutorError::CommandFailed { code, stderr })
        }
    }

    /// Extract text from a document. Supported formats:
    /// `.txt` / `.md` / `.csv` — direct UTF-8 read.
    /// `.pdf` — text extraction via `pdf-extract` (wraps lopdf; pure Rust).
    /// `.docx` — ZIP + XML parse (no external dependency beyond `zip`).
    /// `.xlsx` / `.xls` / `.ods` — `calamine` spreadsheet reader.
    fn exec_read_document(&self, path: &Path) -> Result<String, ExecutorError> {
        let guarded = self.guard_path(path)?;

        let ext = guarded
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        let text = match ext.as_str() {
            "txt" | "md" | "csv" | "log" | "rst" => std::fs::read_to_string(&guarded)?,
            "pdf" => extract_pdf_text(&guarded)?,
            "docx" => extract_docx_text(&guarded)?,
            "xlsx" | "xls" | "ods" => extract_xlsx_text(&guarded)?,
            other => return Err(ExecutorError::UnsupportedFormat(other.to_string())),
        };

        // Truncate to keep LLM context budget safe.
        if text.len() > READ_DOCUMENT_MAX_BYTES {
            let mut truncated = text[..READ_DOCUMENT_MAX_BYTES].to_string();
            truncated.push_str(
                "\n\n[WARNING] Document truncated to avoid exceeding LLM context.",
            );
            Ok(truncated)
        } else {
            Ok(text)
        }
    }

    /// Move `source` to `dest` within the workspace using an atomic OS rename.
    ///
    /// Both paths must resolve inside the workspace root. If `dest`'s parent
    /// directory does not exist, the operation returns an `Io` error — the
    /// caller should surface this as a readable error message.
    fn exec_move_file(&self, source: &Path, dest: &Path) -> Result<String, ExecutorError> {
        let guarded_src = self.guard_path(source)?;
        // guard_path handles non-existent targets via normalize_lexical.
        let guarded_dst = self.guard_path(dest)?;

        std::fs::rename(&guarded_src, &guarded_dst)?;

        Ok(format!(
            "Moved '{}' → '{}'",
            source.display(),
            dest.display()
        ))
    }

    /// Move `path` to the OS trash. Never calls `remove_file` / `remove_dir_all`.
    fn exec_trash_file(&self, path: &Path) -> Result<String, ExecutorError> {
        let guarded = self.guard_path(path)?;

        trash::delete(&guarded)
            .map_err(|e| ExecutorError::Trash(format!("{e}")))?;

        Ok(format!("'{}' moved to trash", path.display()))
    }

    /// Resolve `target` relative to the canonical workspace root and verify
    /// the result stays within the workspace.
    ///
    /// On macOS `/var/` is a symlink to `/private/var/`, so we must
    /// canonicalize the root first and join relative paths against the
    /// canonical form — never the raw path — before verifying containment.
    fn guard_path(&self, target: &Path) -> Result<PathBuf, ExecutorError> {
        let canonical_root = self
            .workspace_root
            .canonicalize()
            .map_err(ExecutorError::Io)?;

        // Join relative paths to the canonical root so that macOS /var symlinks
        // are resolved consistently.
        let abs_target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            canonical_root.join(target)
        };

        // Canonicalize if the path exists; otherwise normalise lexically so
        // that non-existent paths are still guarded.
        let canonical_target = if abs_target.exists() {
            abs_target.canonicalize().map_err(ExecutorError::Io)?
        } else {
            normalize_lexical(&abs_target)
        };

        if !canonical_target.starts_with(&canonical_root) {
            return Err(ExecutorError::PathTraversal(
                target.display().to_string(),
            ));
        }
        Ok(canonical_target)
    }
}

/// Lexically normalise a path (resolve `.` / `..`) without hitting the
/// filesystem. Used for paths that do not yet exist so `canonicalize` would
/// return an error.
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

// ── Document extraction helpers ───────────────────────────────────────────────

/// Extract plain text from a PDF using `pdf-extract` (pure Rust, wraps lopdf).
fn extract_pdf_text(path: &Path) -> Result<String, ExecutorError> {
    pdf_extract::extract_text(path)
        .map_err(|e| ExecutorError::DocumentParse(format!("PDF parse error: {e}")))
}

/// Extract plain text from a DOCX file.
///
/// DOCX is a ZIP archive containing `word/document.xml`. This function opens
/// the ZIP, reads the XML, and uses a lightweight state machine to collect
/// text runs while inserting newlines at paragraph (`</w:p>`) and line-break
/// (`<w:br/>`) elements.
fn extract_docx_text(path: &Path) -> Result<String, ExecutorError> {
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ExecutorError::DocumentParse(format!("DOCX: not a valid zip archive: {e}")))?;

    let mut xml = String::new();
    {
        let mut doc_entry = archive
            .by_name("word/document.xml")
            .map_err(|_| {
                ExecutorError::DocumentParse(
                    "DOCX: word/document.xml not found — file may not be a valid .docx".to_string(),
                )
            })?;
        doc_entry.read_to_string(&mut xml)?;
    }

    Ok(docx_xml_to_text(&xml))
}

/// Lightweight state machine: collect text outside XML tags, insert newlines
/// at `</w:p>` (paragraph end) and `<w:br/>` / `<w:br ...>` (line break).
fn docx_xml_to_text(xml: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut text_buf = String::new();

    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag_buf.clear();
            }
            '>' => {
                in_tag = false;
                let tag = tag_buf.trim();

                // Paragraph end → flush accumulated text with a newline.
                if tag == "/w:p" {
                    let line = text_buf.trim().to_string();
                    if !line.is_empty() {
                        result.push_str(&line);
                        result.push('\n');
                    }
                    text_buf.clear();
                }

                // Line break element → insert newline immediately.
                if tag == "w:br/" || tag.starts_with("w:br ") {
                    result.push('\n');
                }
            }
            _ => {
                if in_tag {
                    tag_buf.push(ch);
                } else {
                    text_buf.push(ch);
                }
            }
        }
    }

    // Flush any trailing text not closed by </w:p>.
    let remainder = text_buf.trim();
    if !remainder.is_empty() {
        result.push_str(remainder);
    }

    result.trim().to_string()
}

/// Extract plain text from an XLSX / XLS / ODS file using `calamine`.
///
/// Each worksheet is prefixed with its name and rendered as tab-separated
/// rows. Empty cells produce empty columns. Sheets are separated by a blank
/// line.
///
/// calamine 0.24 renamed the cell-value enum from `DataType` to `Data`;
/// `worksheet_range` returns `Result<Range<Data>, Error>` (no `Option` wrapper).
fn extract_xlsx_text(path: &Path) -> Result<String, ExecutorError> {
    use calamine::{open_workbook_auto, Data, Reader};

    let mut workbook = open_workbook_auto(path)
        .map_err(|e| ExecutorError::DocumentParse(format!("XLSX open failed: {e}")))?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut output = String::new();

    for sheet_name in &sheet_names {
        let range: calamine::Range<Data> = match workbook.worksheet_range(sheet_name) {
            Ok(r) => r,
            Err(e) => {
                output.push_str(&format!("## Sheet: {sheet_name}\n[read error: {e}]\n"));
                continue;
            }
        };

        if !output.is_empty() {
            output.push_str("\n\n");
        }
        output.push_str("## Sheet: ");
        output.push_str(sheet_name);
        output.push('\n');

        for row in range.rows() {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| match cell {
                    Data::String(s) => s.clone(),
                    Data::Float(f) => {
                        // Display whole numbers without a decimal point.
                        if f.fract() == 0.0 && f.abs() < 1e15_f64 {
                            format!("{}", *f as i64)
                        } else {
                            format!("{f}")
                        }
                    }
                    Data::Int(i) => i.to_string(),
                    Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
                    Data::Empty => String::new(),
                    _ => String::new(),
                })
                .collect();
            output.push_str(&cells.join("\t"));
            output.push('\n');
        }
    }

    Ok(output)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn setup() -> (tempfile::TempDir, UniversalExecutor) {
        let tmp = tempdir().unwrap();
        let exec = UniversalExecutor::new(tmp.path()).unwrap();
        (tmp, exec)
    }

    #[test]
    fn new_returns_err_for_missing_workspace() {
        let result = UniversalExecutor::new("/this/path/does/not/exist/oxcer_test");
        assert!(result.is_err());
    }

    #[test]
    fn list_dir_returns_sorted_entries() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("b.txt"), "b").unwrap();
        fs::write(tmp.path().join("a.txt"), "a").unwrap();
        fs::create_dir(tmp.path().join("subdir")).unwrap();

        let out = exec.execute(&ToolCall::FsListDir(PathBuf::from("."))).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&"a.txt"));
        assert!(lines.contains(&"b.txt"));
        assert!(lines.contains(&"subdir/"));
        // Sorted order: a.txt < b.txt < subdir/
        let a_pos = lines.iter().position(|&l| l == "a.txt").unwrap();
        let b_pos = lines.iter().position(|&l| l == "b.txt").unwrap();
        assert!(a_pos < b_pos);
    }

    #[test]
    fn list_dir_empty_directory() {
        let (tmp, exec) = setup();
        let sub = tmp.path().join("empty");
        fs::create_dir(&sub).unwrap();
        let out = exec.execute(&ToolCall::FsListDir(sub)).unwrap();
        assert_eq!(out, "(empty directory)");
    }

    #[test]
    fn read_file_returns_content() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("hello.txt"), "hello world").unwrap();
        let out = exec
            .execute(&ToolCall::FsReadFile(tmp.path().join("hello.txt")))
            .unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn path_traversal_denied_for_list_dir() {
        let (_tmp, exec) = setup();
        let result = exec.execute(&ToolCall::FsListDir(PathBuf::from("../../etc")));
        assert!(matches!(result, Err(ExecutorError::PathTraversal(_))));
    }

    #[test]
    fn path_traversal_denied_for_read_file() {
        let (_tmp, exec) = setup();
        let result = exec.execute(&ToolCall::FsReadFile(PathBuf::from("../../etc/passwd")));
        assert!(matches!(result, Err(ExecutorError::PathTraversal(_))));
    }

    #[test]
    fn shell_run_allowed_command() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("test.txt"), "line").unwrap();
        let out = exec
            .execute(&ToolCall::ShellRun {
                command: "ls".to_string(),
                args: vec![],
            })
            .unwrap();
        assert!(out.contains("test.txt"));
    }

    #[test]
    fn shell_run_disallows_rm() {
        let (_tmp, exec) = setup();
        let result = exec.execute(&ToolCall::ShellRun {
            command: "rm".to_string(),
            args: vec!["-rf".to_string(), ".".to_string()],
        });
        assert!(matches!(result, Err(ExecutorError::CommandNotAllowed(_))));
    }

    #[test]
    fn shell_run_disallows_curl() {
        let (_tmp, exec) = setup();
        let result = exec.execute(&ToolCall::ShellRun {
            command: "curl".to_string(),
            args: vec!["http://example.com".to_string()],
        });
        assert!(matches!(result, Err(ExecutorError::CommandNotAllowed(_))));
    }

    // ── read_document ────────────────────────────────────────────────────────

    #[test]
    fn read_document_reads_txt_file() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("note.txt"), "Hello from TXT").unwrap();
        let out = exec
            .execute(&ToolCall::ReadDocument(PathBuf::from("note.txt")))
            .unwrap();
        assert_eq!(out, "Hello from TXT");
    }

    #[test]
    fn read_document_reads_md_file() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("readme.md"), "# Title\n\nBody text.").unwrap();
        let out = exec
            .execute(&ToolCall::ReadDocument(PathBuf::from("readme.md")))
            .unwrap();
        assert!(out.contains("Title"));
        assert!(out.contains("Body text."));
    }

    #[test]
    fn read_document_unsupported_format_returns_err() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("binary.exe"), &[0u8; 16]).unwrap();
        let result = exec.execute(&ToolCall::ReadDocument(PathBuf::from("binary.exe")));
        assert!(matches!(result, Err(ExecutorError::UnsupportedFormat(_))));
    }

    #[test]
    fn read_document_truncates_large_file() {
        let (tmp, exec) = setup();
        // Write a file slightly over the 100 KB limit.
        let large = "A".repeat(READ_DOCUMENT_MAX_BYTES + 100);
        fs::write(tmp.path().join("big.txt"), &large).unwrap();
        let out = exec
            .execute(&ToolCall::ReadDocument(PathBuf::from("big.txt")))
            .unwrap();
        assert!(out.len() <= READ_DOCUMENT_MAX_BYTES + 200); // room for the warning suffix
        assert!(out.contains("[WARNING] Document truncated"));
    }

    #[test]
    fn read_document_path_traversal_denied() {
        let (_tmp, exec) = setup();
        let result =
            exec.execute(&ToolCall::ReadDocument(PathBuf::from("../../etc/passwd")));
        assert!(matches!(result, Err(ExecutorError::PathTraversal(_))));
    }

    // ── move_file ────────────────────────────────────────────────────────────

    #[test]
    fn move_file_renames_within_workspace() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("old.txt"), "content").unwrap();

        let out = exec
            .execute(&ToolCall::MoveFile {
                source: PathBuf::from("old.txt"),
                dest: PathBuf::from("new.txt"),
            })
            .unwrap();

        assert!(out.contains("Moved"));
        assert!(!tmp.path().join("old.txt").exists());
        assert!(tmp.path().join("new.txt").exists());
        assert_eq!(fs::read_to_string(tmp.path().join("new.txt")).unwrap(), "content");
    }

    #[test]
    fn move_file_source_traversal_denied() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("dst.txt"), "").unwrap();
        let result = exec.execute(&ToolCall::MoveFile {
            source: PathBuf::from("../../etc/passwd"),
            dest: PathBuf::from("dst.txt"),
        });
        assert!(matches!(result, Err(ExecutorError::PathTraversal(_))));
    }

    #[test]
    fn move_file_dest_traversal_denied() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("src.txt"), "data").unwrap();
        let result = exec.execute(&ToolCall::MoveFile {
            source: PathBuf::from("src.txt"),
            dest: PathBuf::from("../../tmp/leaked.txt"),
        });
        assert!(matches!(result, Err(ExecutorError::PathTraversal(_))));
    }

    // ── trash_file ───────────────────────────────────────────────────────────

    #[test]
    fn trash_file_path_traversal_denied() {
        let (_tmp, exec) = setup();
        let result =
            exec.execute(&ToolCall::TrashFile(PathBuf::from("../../etc/passwd")));
        assert!(matches!(result, Err(ExecutorError::PathTraversal(_))));
    }

    // ── docx_xml_to_text ─────────────────────────────────────────────────────

    #[test]
    fn docx_xml_to_text_basic_paragraph() {
        let xml = r#"<w:body><w:p><w:r><w:t>Hello world</w:t></w:r></w:p></w:body>"#;
        let text = docx_xml_to_text(xml);
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn docx_xml_to_text_two_paragraphs() {
        let xml = r#"<w:p><w:t>First</w:t></w:p><w:p><w:t>Second</w:t></w:p>"#;
        let text = docx_xml_to_text(xml);
        assert!(text.contains("First"));
        assert!(text.contains("Second"));
        // They should be separated by a newline.
        assert!(text.contains('\n'));
    }

    #[test]
    fn docx_xml_to_text_empty_paragraphs_skipped() {
        let xml = r#"<w:p></w:p><w:p><w:t>Content</w:t></w:p>"#;
        let text = docx_xml_to_text(xml);
        // Only one newline, no spurious blank lines.
        assert_eq!(text, "Content");
    }

    // ── resolve_path ─────────────────────────────────────────────────────────

    #[test]
    fn resolve_path_returns_canonical_for_existing_file() {
        let (tmp, exec) = setup();
        fs::write(tmp.path().join("check.txt"), "x").unwrap();
        let resolved = exec.resolve_path(Path::new("check.txt")).unwrap();
        assert!(resolved.is_absolute());
        assert!(resolved.ends_with("check.txt"));
    }

    #[test]
    fn resolve_path_denies_traversal() {
        let (_tmp, exec) = setup();
        let result = exec.resolve_path(Path::new("../../etc"));
        assert!(matches!(result, Err(ExecutorError::PathTraversal(_))));
    }
}
