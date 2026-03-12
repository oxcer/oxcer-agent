//! Oxcer library: event log, router, settings, commands.
//!
//! The binary (`main.rs`) builds the Tauri app and uses these modules.
//! Integration tests in `tests/` use this crate as a library to exercise
//! event_log, workspace cleanup, and approval flows via public APIs.

pub mod agent_sessions;
pub mod commands;
pub mod event_log;
pub mod router;
pub mod scrubbing_log;
pub mod settings;
pub mod setup;
pub mod telemetry_viewer;

#[cfg(feature = "test")]
pub mod test_support;
