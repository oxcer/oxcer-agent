//! Oxcer core: FS Service, Shell Service, Security Policy, Logging.
//! Pure Rust library with no Tauri dependency; launchers (Tauri, SwiftUI, etc.) depend on this crate.
//!
//! ## "Agent = untrusted client" contract
//!
//! The `fs::` and `shell::` modules provide low-level operations. They are **never**
//! exposed directly to the Agent Orchestrator. The launcher (e.g. Tauri) wraps these
//! in commands that go through the Security Policy Engine first. Agents pass
//! `caller: AGENT_ORCHESTRATOR`; the policy engine enforces stricter rules
//! (write/delete/exec → REQUIRE_APPROVAL). This invariant is enforced at the
//! launcher level — only the Command Router may call `fs::`/`shell::` after
//! policy evaluation.

pub mod fs;
pub mod network;
pub mod security;
pub mod shell;
