//! native_symphony — typed Tauri commands over the supervised x0x-symphonyd.
//!
//! Each command resolves the supervised loopback endpoint from [`AppState`],
//! reads the transient bearer token from the daemon data dir, builds a
//! [`SymphonyClient`], and proxies one daemon call. The token is never stored
//! on the command/state; errors propagate as structured [`String`]s that carry
//! the daemon's HTTP status + body (so `conflict`/`not_found` distinctions
//! surface to the operator).
//!
//! No Nostr relay event is emitted by any path here.
//!
//! The module is split across cohesive submodules under `native_symphony/`:
//! [`proxy`] holds the thin Symphony daemon proxy commands, [`company`] holds
//! the Company template types + instantiate/resume commands, [`lifecycle`]
//! holds the shared Company lifecycle/reconciliation internals, [`instances`]
//! holds persisted-instance management + boot reconciliation, and [`fs_util`]
//! holds private filesystem helpers. Public commands and types are re-exported
//! from here so existing callsites and the invoke registration are unchanged.

mod company;
mod fs_util;
mod instances;
mod lifecycle;
mod proxy;

#[cfg(test)]
mod test_injection;
#[cfg(test)]
mod tests;

pub use company::*;
pub use instances::*;
pub use proxy::*;
