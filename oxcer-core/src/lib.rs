//! Oxcer core: FS Service, Shell Service, Security Policy, Logging.
//! Pure Rust library with no Tauri dependency; launchers (Tauri, SwiftUI, etc.) depend on this crate.
//!
//! ## "Agent = untrusted client" contract
//!
//! The `fs::` and `shell::` modules provide low-level operations. They are **never**
//! exposed directly to the Agent Orchestrator. The launcher (e.g. Tauri) wraps these
//! in commands that go through the Security Policy Engine first. Agents pass
//! `caller: AGENT_ORCHESTRATOR`; the policy engine enforces stricter rules
//! (write/delete/exec -> REQUIRE_APPROVAL). This invariant is enforced at the
//! launcher level — only the Command Router may call `fs::`/`shell::` after
//! policy evaluation.

pub mod agent_session_log;
pub mod data_sensitivity;
pub mod data_sensitivity_config;
pub mod env_filter;
pub mod fs;
pub mod llm;
pub mod plugins;
pub mod network;
pub mod orchestrator;
pub mod prompt_sanitizer;
pub mod security;
pub mod semantic_router;
pub mod llm_metrics;
pub mod shell;
pub mod telemetry;
