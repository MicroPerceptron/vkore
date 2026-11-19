//! vKore's runtime: Kore machines on a Linux or macOS host, their volumes,
//! and the projects staged into them.
//!
//! - [`config`] keeps each machine's configuration under vKore's state
//!   directory, written whole by one rename.
//! - [`run`] boots a machine: preflight (firmware, QEMU's capabilities, the
//!   ISO's paging mode and digest, the disk), stages its project into a
//!   terminal's home, follows the boot to readiness, stops it, and collects
//!   and applies what the machine changed under the project's bounds.
//!
//! [`images`] imports a local ISO or downloads one from a caller-provided HTTPS
//! link, retaining its SHA-256. [`inference`] exposes a local HTTP API; its
//! guest backend remains disconnected until libkore exports the Kore channel.

#![cfg(unix)]

pub mod catalog;
pub mod config;
pub mod hardware;
pub mod images;
pub mod inference;
pub mod run;
pub mod state;

pub use config::{MachineConfig, MachineStore, StoreError, WorkspaceConfig, Writeback};
pub use run::{Event, Preflight, RunError, Session};
pub use state::State;
