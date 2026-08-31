//! Open, capability-scoped tool add-ons for vKore.
//!
//! Registering an add-on never grants authority. The registry exposes a tool
//! to a worker only when a live [`ToolCapability`] names the exact add-on,
//! package hash, tool, agent generation, and prompt.

mod authority;
mod manifest;
mod registry;
mod runtime;

pub use authority::{
    AgentRef, ArgConstraint, AuthorityError, AuthoritySet, CallConstraints, CancellationRecord,
    CapabilityId, DisclosureGrant, Escalation, Invocation, InvocationApproval, InvocationId,
    InvocationResult, InvocationTicket, MediationMode, PromptRef, ResultHandle, ResultSensitivity,
    TOOL_CAPABILITY_SCHEMA_VERSION, TicketIssuer, ToolCapability, ToolRevocation,
    ValidatedInvocation, unix_time_ms,
};
pub use manifest::{
    ADDON_MANIFEST_VERSION, AddonEntrypoint, AddonManifest, ExecutionDomain,
    INVOCATION_PROTOCOL_VERSION, ManifestError, ResourceEffect, RiskClass, ToolManifest,
};
pub use registry::{AddonPackage, AddonRegistry, CatalogError, WorkerTool};
pub use runtime::{HostRuntime, RuntimeError, RuntimeLimits};
