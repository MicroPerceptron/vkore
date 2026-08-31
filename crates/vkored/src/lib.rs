//! Bounded host endpoint and conformance harness for HostLink v1.

pub mod backend;
pub mod bulk;
pub mod daemon;
pub mod harness;
#[cfg(unix)]
pub mod hybrid;
pub mod ivshmem;
pub mod llama;
pub mod protocol;
pub mod ui;
pub mod workspace;

pub use vkore_vm as qemu;

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
pub use ivshmem::{
    IVSHMEM_BACKING_BYTES, IvshmemBulkHost, IvshmemError, IvshmemMetrics, frozen_layout,
};
pub use llama::{LlamaServerBackend, LlamaServerConfig, LlamaServerError};
pub use ui::{LiveUi, LiveUiError};
pub use workspace::{WorkspaceBackend, WorkspaceBackendError};
