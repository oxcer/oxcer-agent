//! Security & Guardrails layer for Oxcer.
//!
//! Philosophy: **"Agents are untrusted clients."** All high-risk operations
//! must go through a human-in-the-loop (HITL) approval flow by default.
//!
//! The Policy Engine sits in front of the Command Router and acts as the
//! final authority for all privileged actions.
//!
//! Policies are defined as data (YAML/JSON); see `policy_config` and
//! `policies/default.yaml`. Invalid policy → secure default (default-deny).

pub mod policy_config;
pub mod policy_engine;
