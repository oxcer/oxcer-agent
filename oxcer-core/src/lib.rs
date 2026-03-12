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
//!
//! ## Agent architectures
//!
//! Two agent subsystems exist in this crate:
//!
//! ### `orchestrator` — **Production** (used by the Swift/UniFFI launcher)
//!
//! A plan-first, step-driven agent loop connected to the launcher over the
//! UniFFI FFI boundary. The launcher calls `start_session` to build a plan,
//! then drives the loop with repeated `ffi_agent_step` calls — each step
//! executes one `ToolCallIntent` and hands control back to the launcher for
//! UI updates and per-step user approval. All demo workflows (single-file
//! summary, multi-file overview, folder move) run through this path.
//!
//! ### `fsm` — **Experimental** (not connected to the launcher)
//!
//! An earlier ReAct-style FSM prototype. It is gated behind the
//! `experimental` Cargo feature and is **not** referenced by the production
//! launcher or the UniFFI bindings. It exists as a research sandbox.
//! To enable: `cargo build --features experimental`.
//!
//! When adding new workflows, extend `orchestrator` — not `fsm`.

pub mod agent_session_log;
pub mod cloud_provider;
pub mod data_sensitivity;
pub mod data_sensitivity_config;
pub mod db;
pub mod env_filter;
pub mod executor;
pub mod fs;
#[cfg(feature = "experimental")]
pub mod fsm;
pub mod guardrail;
pub mod llm;
pub mod llm_metrics;
pub mod mcp;
pub mod memory;
pub mod network;
pub mod orchestrator;
pub mod plugins;
pub mod prompt_sanitizer;
pub mod security;
pub mod semantic_router;
pub mod shell;
pub mod subagent;
pub mod telemetry;
pub mod terminal;
