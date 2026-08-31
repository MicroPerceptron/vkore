//! Local policy records and authenticated user approvals for vKore.
//!
//! This crate produces bounded approval ceilings. Kore's resident broker still
//! validates live holdings and performs the authoritative transaction.

mod dashboard;
mod engine;
mod store;
mod types;

pub use dashboard::{AuthorityDiff, DashboardView, render_dashboard};
pub use engine::{AuthorityTransport, PolicyEngine, PolicyError, UserActionAuthenticator};
pub use store::{PolicyStore, StoreError};
pub use types::*;
