//! Bounded host endpoint and conformance harness for HostLink v1.

pub mod approval;
pub mod backend;
pub mod bulk;
pub mod daemon;
mod frontend_updates;
pub mod harness;
#[cfg(unix)]
pub mod hybrid;
pub mod interactive;
pub mod ivshmem;
pub mod kore_runtime;
pub mod llama;
pub mod project;
pub mod protocol;
pub mod settings;
pub mod ui;
pub mod workspace;

pub use vkore_vm as qemu;

pub use approval::UiApprovalService;
pub use backend::{
    BackendError, BackendEvent, BackendFailure, BackendLimits, BackendMetrics, BackendRole,
    DeterministicBackend, InferenceBackend, InferenceObservation, ObservationKind, RequestUsage,
    SequenceIdentity, SequenceMetrics,
};
pub use daemon::{
    AuthorityObservation, BenchmarkEvent, BenchmarkSink, Config, DaemonError, HostLinkDaemon,
    Metrics, TcpConfig,
};
#[cfg(unix)]
pub use hybrid::{HybridError, HybridHostLink, HybridMetrics};
pub use interactive::{
    AuthorityUpdateRequest, AuthorityUpdateState, AuthorityUpdateStatus, BackendHarness,
    CreateSession, DeterministicGuest, EventKind, HarnessEvent, HarnessHandle, HarnessHttpServer,
    HarnessService, PromptRequest, PromptStatus, SessionState, SessionStatus, TaskRuntime,
};
pub use ivshmem::{
    IVSHMEM_BACKING_BYTES, IvshmemBulkHost, IvshmemError, IvshmemMetrics, frozen_layout,
};
pub use kore_runtime::{KoreVmRuntime, KoreVmRuntimeConfig};
pub use llama::{LlamaServerBackend, LlamaServerConfig, LlamaServerError};
pub use project::{
    AddonSelection, ApprovalProfile, InferenceConfig, NewProjectOutcome, ProjectConfig,
    ProjectError, ProjectStore, ResourceLimits, RetentionConfig, RuntimeBackend, WorkspaceAccess,
    WorkspaceConfig, WorkspacePreflight,
};
pub use settings::{
    ActivitySummary, AppSettings, BodyFont, ModelOption, SettingsError, SettingsStore,
    recent_activity,
};
pub use ui::{LiveUi, LiveUiError};
pub use workspace::{WorkspaceBackend, WorkspaceBackendError};
