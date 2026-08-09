//! Built-in, x0x-native Company templates and resumable provisioning.
//!
//! This module owns the declarative template, deterministic plan, native x0xd
//! provisioner, generated Symphony `WORKFLOW.md`, and durable instance manifest.
//! It never links to the relay or stores Nostr events.

pub mod builtin;
pub mod instantiate;
pub mod parse;
pub mod plan;
pub mod provisioner;
pub mod registry;
pub mod spec;
pub mod symphony_config;
pub mod workflow;

#[cfg(test)]
mod m4_hybrid_tests;

#[cfg(test)]
mod m4_hybrid_runtime_tests;
