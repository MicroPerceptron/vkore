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
//! The ISO is always the caller's: vKore never builds or fetches Kore, and
//! records the SHA-256 of every image it boots.

#![cfg(unix)]

pub mod config;
pub mod run;
pub mod state;

pub use config::{MachineConfig, MachineStore, StoreError, WorkspaceConfig, Writeback};
pub use run::{Event, Preflight, RunError, Session};
pub use state::State;
