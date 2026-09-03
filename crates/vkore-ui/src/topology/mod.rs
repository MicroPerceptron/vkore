//! The topology views: the agent spawn lineage and the permission graph.
//!
//! These are not a separate window. The harness header's `AGENTS` and
//! `FILESYSTEM` buttons replace the session body with them, and each brings
//! its own inspector.

pub mod filesystem;
pub mod inspector;
pub mod layout;
pub mod model;
pub mod view;
