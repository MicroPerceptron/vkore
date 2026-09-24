//! Interactive harness runtime backed by a real Kore VM and HostLink.

use crate::{
    AuthorityUpdateRequest, AuthorityUpdateStatus, BackendLimits, EventKind, HostLinkDaemon,
    InferenceBackend, LlamaServerBackend, LlamaServerConfig, ProjectConfig, ProjectStore,
    PromptRequest, PromptStatus, RuntimeBackend, SessionState, TaskRuntime, WorkspaceAccess,
    WorkspaceBackend, interactive::emit_inference_observation,
};
use kore_hostlink::{AuthorityKind, FrontendIntentId, FrontendResultKind};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use vkore_vm::{QemuConfig, QemuProcess};

const DEFAULT_TICK: Duration = Duration::from_millis(1);
const VM_PAUSE_DRAIN: Duration = Duration::from_millis(500);
const BOOT_LOG_GRACE: Duration = Duration::from_secs(1);
const BOOT_LOG_POLL: Duration = Duration::from_millis(250);
const PROMPT_PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
const BOOT_LOG_TAIL_BYTES: u64 = 128 * 1024;
const CONVERSATIONS_MAX: usize = 32;
const CONVERSATION_IDLE: Duration = Duration::from_secs(30 * 60);

struct ConversationContext {
    exchanges: Vec<(bool, String)>,
    last_used: Instant,
}

fn retire_process<P>(
    process: &mut Option<P>,
    terminate: impl FnOnce(&mut P) -> std::io::Result<()>,
) -> std::io::Result<()> {
    if let Some(child) = process.as_mut() {
        terminate(child)?;
        *process = None;
    }
    Ok(())
}

/// Correlates the authenticated API update with the guest's independent
/// revision. Never consume another prompt's outcome or publish a chat answer.
pub(crate) struct LiveUpdateBridge {
    intent: FrontendIntentId,
    pending: Option<(String, u32)>,
}

impl LiveUpdateBridge {
    pub(crate) fn new(intent: FrontendIntentId) -> Self {
        Self {
            intent,
            pending: None,
        }
    }

    pub(crate) fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn submit<B: InferenceBackend, S: crate::BenchmarkSink>(
        &mut self,
        core: &mut crate::daemon::LinkCore<B, S>,
        updates: &dyn Fn() -> Option<AuthorityUpdateRequest>,
        emit: &mut dyn FnMut(EventKind, bool, Value),
    ) {
        if self.pending.is_some() {
            return;
        }
        let Some(update) = updates() else {
            return;
        };
        match core.queue_frontend_update(self.intent, update.text) {
            Ok(revision) => self.pending = Some((update.update_id, revision)),
            Err(error) => emit(
                EventKind::AuthorityUpdate,
                true,
                json!({
                    "update_id": update.update_id, "status": AuthorityUpdateStatus::Refused,
                    "message": format!("The active prompt could not accept this authority update: {error:?}"),
                    "source": "host_frontend"
                }),
            ),
        }
    }

    pub(crate) fn receive<B: InferenceBackend, S: crate::BenchmarkSink>(
        &mut self,
        core: &mut crate::daemon::LinkCore<B, S>,
        emit: &mut dyn FnMut(EventKind, bool, Value),
    ) {
        let Some((id, revision)) = self.pending.as_ref() else {
            return;
        };
        let Some(result) = core.pop_frontend_update_result_for(self.intent, *revision) else {
            return;
        };
        use kore_hostlink::FrontendUpdateOutcome as Outcome;
        let status = match result.outcome {
            Outcome::Applied => AuthorityUpdateStatus::Applied,
            Outcome::Refused => AuthorityUpdateStatus::Refused,
            Outcome::Failed => AuthorityUpdateStatus::Failed,
            Outcome::Cancelled => AuthorityUpdateStatus::Cancelled,
        };
        emit(
            EventKind::AuthorityUpdate,
            true,
            json!({
                "update_id": id, "revision": revision, "status": status,
                "message": result.text, "source": "hostlink"
            }),
        );
        self.pending = None;
    }
}

#[derive(Clone, Debug)]
pub struct KoreVmRuntimeConfig {
    pub qemu_executable: PathBuf,
    pub qemu_arguments: Vec<OsString>,
    pub project_root: PathBuf,
    /// Compatibility flag; live inference is mandatory even when false.
    pub require_real: bool,
    /// Explicit headless optimization. Interactive desktops must keep running
    /// both between prompts and while the host performs inference.
    pub headless_park: bool,
}

pub struct KoreVmRuntime {
    config: KoreVmRuntimeConfig,
    runtime: Option<ProjectRuntime>,
    incarnation: u64,
    conversations: BTreeMap<(String, String), ConversationContext>,
    supervisor: Option<crate::backend::SequenceIdentity>,
}

struct ProjectRuntime {
    config: ProjectConfig,
    daemon: HostLinkDaemon<WorkspaceBackend<Box<dyn InferenceBackend + Send>>>,
    qemu: QemuConfig,
    process: Option<QemuProcess>,
    session_id: Option<String>,
    stop_pending: bool,
    serial_log: Option<PathBuf>,
    boot_attempt: u8,
    next_boot_log_check: Option<Instant>,
    hostlink_trace_cursor: usize,
    active_model_turns: usize,
    vm_pause_due: Option<Instant>,
    vm_suspended: bool,
    resource_identities: BTreeMap<String, PathBuf>,
    task_catalog: Vec<(String, bool)>,
    focus_attached: bool,
    headless_park: bool,
    recovering: bool,
    retiring_supervisor: Option<crate::backend::SequenceIdentity>,
    supervisor_stop_sent: bool,
}

impl KoreVmRuntime {
    pub fn new(config: KoreVmRuntimeConfig) -> Result<Self, String> {
        let projects =
            ProjectStore::open(&config.project_root).map_err(|error| error.to_string())?;
        crate::SettingsStore::open(
            config
                .project_root
                .parent()
                .unwrap_or(&config.project_root)
                .join("settings.json"),
        )
        .map_err(|error| error.to_string())?
        .migrate_runtime(&projects)
        .map_err(|error| error.to_string())?;
        Ok(Self {
            config,
            runtime: None,
            incarnation: 1,
            conversations: BTreeMap::new(),
            supervisor: None,
        })
    }

    fn ensure_project(
        &mut self,
        project_id: &str,
        query: &str,
        emit: &mut dyn FnMut(EventKind, bool, Value),
    ) -> Result<&mut ProjectRuntime, String> {
        // The HTTP project API owns a separate store instance. Reload its
        // atomic JSON files here so newly created or edited projects become
        // effective without restarting the harness daemon.
        let mut project = ProjectStore::open(&self.config.project_root)
            .map_err(|error| error.to_string())?
            .get(project_id)
            .map_err(|error| error.to_string())?;
        if project.inference.backend == RuntimeBackend::Deterministic {
            return Err(
                "real-only runtime requires llama.cpp; deterministic inference is forbidden".into(),
            );
        }
        if self.runtime.is_none() {
            return Err(
                "Assistant runtime is unavailable. Wait for preparation or use Retry.".into(),
            );
        }
        let state = self
            .config
            .project_root
            .parent()
            .unwrap_or(&self.config.project_root);
        let settings = crate::SettingsStore::open(state.join("settings.json"))
            .map_err(|e| e.to_string())?
            .get()
            .map_err(|e| e.to_string())?;
        let discovery_started = Instant::now();
        let mut discovery = crate::personal::search(query, &settings.authorized_locations)?;
        let focus_match = !project.workspace.virtual_only
            && discovery.prefer_focus_root(&project.workspace.root)?;
        emit(
            EventKind::Validation,
            false,
            json!({"stage":"personal_discovery_completed",
            "elapsed_ms":discovery_started.elapsed().as_millis(), "entries_scanned":discovery.scanned,
            "candidate_count":discovery.files.len(), "bounded":discovery.bounded,
            "focus_match":focus_match}),
        );
        let runtime = self
            .runtime
            .as_mut()
            .ok_or("Application runtime is not prepared")?;
        if runtime.config.inference != settings.default_inference {
            return Err("The application model changed. Restart the shared assistant from Settings before sending another message.".into());
        }
        // Legacy project model settings remain discoverable migration inputs;
        // execution uses the one explicitly selected application profile.
        project.inference = runtime.config.inference.clone();
        // The pinned guest retains its imported root and standing write grant.
        // Replacing only the host adapter leaves the guest with stale authority.
        // Reuse an unchanged root (including its grant) across prompts/chats;
        // replace attachments only after the exact old VM has been reaped.
        let focus_attached = discovery.files.is_empty() && !project.workspace.virtual_only;
        let reuse_attachment = runtime.focus_attached
            && discovery.files.is_empty()
            && runtime.config.workspace == project.workspace;
        if runtime.recovering || !runtime.daemon.broker_idle() {
            return Err("Task attachment requires reclaimed broker idle".into());
        }
        runtime.resource_identities = discovery.identities.into_iter().collect();
        if !reuse_attachment {
            runtime.stop_process()?;
            runtime
                .daemon
                .with_disconnected_backend(|backend| {
                    if discovery.files.is_empty() {
                        backend.attach_task_root(
                            &project.workspace.root,
                            &project.workspace.state,
                            project.workspace.access == WorkspaceAccess::AutomaticWriteback,
                        )
                    } else {
                        backend.attach_selected(&state.join("runtime/empty"), discovery.files)
                    }?;
                    Ok::<_, crate::workspace::WorkspaceBackendError>(backend.task_catalog())
                })
                .map_err(|error| format!("Task attachment requires a stopped guest: {error:?}"))?
                .map(|catalog| runtime.task_catalog = catalog)
                .map_err(|error| format!("Task filesystem attachment failed: {error:?}"))?;
            timing_marker(emit, "workspace_guest_restart_required");
        } else {
            runtime.task_catalog = runtime
                .daemon
                .with_idle_backend(|backend| backend.task_catalog())
                .map_err(|error| format!("Task catalog unavailable: {error:?}"))?;
        }
        runtime.focus_attached = focus_attached;
        if runtime.focus_attached {
            runtime
                .resource_identities
                .insert("root".into(), project.workspace.root.clone());
            runtime
                .resource_identities
                .insert("project".into(), project.workspace.root.clone());
            for (path, _) in &runtime.task_catalog {
                runtime
                    .resource_identities
                    .insert(path.clone(), project.workspace.root.join(path));
            }
        }
        timing_marker(emit, "task_resources_attached");
        runtime.config = project;
        Ok(runtime)
    }

    fn application_profile(&self) -> Result<ProjectConfig, String> {
        use crate::project::*;
        let state = self
            .config
            .project_root
            .parent()
            .unwrap_or(&self.config.project_root);
        let settings = crate::SettingsStore::open(state.join("settings.json"))
            .map_err(|e| e.to_string())?
            .get()
            .map_err(|e| e.to_string())?;
        let empty = state.join("runtime/empty");
        let journal = state.join("runtime/empty-state");
        std::fs::create_dir_all(&empty).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&journal).map_err(|e| e.to_string())?;
        Ok(ProjectConfig {
            schema_version: PROJECT_SCHEMA_VERSION,
            id: "application".into(),
            name: "Assistant".into(),
            workspace: WorkspaceConfig {
                virtual_only: false,
                root: empty,
                state: journal,
                access: WorkspaceAccess::ReadOnly,
                exclusions: vec![],
            },
            inference: settings.default_inference,
            addons: vec![],
            approvals: ApprovalProfile {
                default_scope: "once".into(),
                confirm_external_writes: true,
                confirm_destructive: true,
                standing_policy_enabled: false,
            },
            retention: RetentionConfig {
                transcript_days: 30,
                clean_kv_on_close: true,
                retain_raw_model_output: false,
            },
            resources: ResourceLimits {
                memory_mib: settings.runtime.memory_mib,
                virtual_cpus: settings.runtime.virtual_cpus,
                prompt_seconds: 300,
                agent_limit: 1,
            },
            archived: false,
        })
    }
}

impl ProjectRuntime {
    fn start(
        runtime: &KoreVmRuntimeConfig,
        config: ProjectConfig,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, String> {
        let inference: Box<dyn InferenceBackend + Send> =
            match config.inference.backend {
                RuntimeBackend::Deterministic => return Err(
                    "real-only runtime requires llama.cpp; deterministic inference is forbidden"
                        .into(),
                ),
                RuntimeBackend::LlamaCpp => {
                    let executable = config
                        .inference
                        .llama_server
                        .clone()
                        .ok_or("Configure the application llama.cpp server in Settings")?;
                    let model = config
                        .inference
                        .model
                        .clone()
                        .ok_or("Choose an installed application model in Settings")?;
                    let slots = usize::from(config.inference.slots).max(2);
                    let limits = BackendLimits {
                        sequences: slots,
                        brokers: 1,
                        agents: slots.saturating_sub(1),
                        max_output_tokens: config.inference.output_tokens,
                        // The interactive guest keeps its clock running during
                        // inference and can request 900,000 ticks. The outer
                        // prompt deadline still bounds actual wall-clock time.
                        request_timeout_ticks: 1_000_000,
                        ..BackendLimits::default()
                    };
                    timing_marker(emit, "model_server_starting");
                    Box::new(
                        LlamaServerBackend::start_cancellable(
                            LlamaServerConfig {
                                executable,
                                executable_args: config
                                    .inference
                                    .extra_args
                                    .iter()
                                    .map(OsString::from)
                                    .collect(),
                                model,
                                port: reserve_loopback_port()?,
                                total_context_tokens: config.inference.context_tokens,
                                threads: config.inference.threads,
                                startup_timeout: Duration::from_secs(600),
                                io_timeout: Duration::from_millis(25),
                                limits,
                                trace: std::env::var_os("VKORE_LLAMA_TRACE").is_some(),
                            },
                            cancelled,
                        )
                        .map_err(|error| format!("llama.cpp startup failed: {error:?}"))?,
                    )
                }
            };
        timing_marker(emit, "model_server_ready");
        timing_marker(emit, "workspace_open_starting");
        let allow_writeback = config.workspace.access == WorkspaceAccess::AutomaticWriteback;
        let backend = WorkspaceBackend::open_with_inference(
            &config.workspace.root,
            Some(&config.workspace.state),
            allow_writeback,
            inference,
        )
        .map_err(|error| format!("workspace backend failed: {error:?}"))?;
        timing_marker(emit, "workspace_open_ready");
        timing_marker(emit, "hostlink_bind_starting");
        let (daemon, hostlink) = HostLinkDaemon::bind_dynamic_tcp(backend, ())
            .map_err(|error| format!("HostLink bind failed: {error:?}"))?;
        timing_marker(emit, "hostlink_bind_ready");
        let kore_iso = config
            .inference
            .kore_image
            .clone()
            .ok_or("Configure the headless Kore image in application settings")?;
        let serial_log = qemu_serial_log(&runtime.qemu_arguments);
        let mut arguments = without_qemu_resource_arguments(&runtime.qemu_arguments)?;
        arguments.extend([
            OsString::from("-m"),
            OsString::from(config.resources.memory_mib.to_string()),
            OsString::from("-smp"),
            OsString::from(config.resources.virtual_cpus.to_string()),
        ]);
        let qemu = QemuConfig {
            executable: runtime.qemu_executable.clone(),
            kore_iso,
            arguments,
            hostlink_arguments: vkore_vm::tcp_arguments(hostlink.control, hostlink.bulk)
                .map_err(|error| format!("HostLink QEMU topology failed: {error:?}"))?,
        };
        Ok(Self {
            config,
            daemon,
            qemu,
            process: None,
            session_id: None,
            stop_pending: false,
            serial_log,
            boot_attempt: 0,
            next_boot_log_check: None,
            hostlink_trace_cursor: 0,
            active_model_turns: 0,
            vm_pause_due: None,
            vm_suspended: false,
            resource_identities: BTreeMap::new(),
            task_catalog: Vec::new(),
            focus_attached: false,
            headless_park: runtime.headless_park,
            recovering: false,
            retiring_supervisor: None,
            supervisor_stop_sent: false,
        })
    }

    fn stop_process(&mut self) -> Result<(), String> {
        self.stop_pending = true;
        self.active_model_turns = 0;
        self.vm_pause_due = None;
        self.next_boot_log_check = None;
        self.daemon.force_disconnect();
        // Host process termination does not require QMP/vCPU resumption.
        // Keep the exact child and its private QMP socket on any failure.
        retire_process(&mut self.process, |process| process.terminate().map(|_| ()))
            .map_err(|error| format!("Kore VM termination was not confirmed: {error}"))?;
        self.vm_suspended = false;
        self.stop_pending = false;
        Ok(())
    }

    fn stop(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) -> bool {
        if let Err(error) = self.stop_process() {
            emit(
                EventKind::Error,
                false,
                json!({"message": error, "stage": "vm_stop_failed"}),
            );
            return false;
        }
        // Retiring the VM is not closing the logical conversation. Retain
        // ownership so an explicit session close can clear its model context.
        self.boot_attempt = 0;
        true
    }

    fn recover_supervisor(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        if !self.recovering {
            self.recovering = true;
            self.retiring_supervisor = self.daemon.supervisor_identity();
            self.supervisor_stop_sent = false;
        }
        // Keep the desktop and native cleanup owner scheduled. A failed QMP
        // resume is reported, never converted into implicit VM termination.
        if let Err(error) = self.resume_vm_for_prompt(emit) {
            emit(
                EventKind::Error,
                false,
                json!({"message":error,"stage":"supervisor_recovery_pending"}),
            );
            return;
        }
        if !self.supervisor_stop_sent
            && let Some(identity) = self.retiring_supervisor
        {
            self.supervisor_stop_sent = self.daemon.stop_supervisor(identity);
        }
    }

    fn observe_supervisor_ready(&mut self) -> Option<crate::backend::SequenceIdentity> {
        let identity = self.daemon.supervisor_ready()?;
        if self.recovering {
            if self.retiring_supervisor == Some(identity) {
                return None;
            }
            self.recovering = false;
            self.retiring_supervisor = None;
            self.supervisor_stop_sent = false;
            self.daemon.release_interrupted_frontend();
        }
        Some(identity)
    }

    fn spawn_vm(&mut self) -> Result<(), String> {
        if self.process.is_some() || self.stop_pending {
            return Err("Previous Kore VM termination must be confirmed before startup".into());
        }
        if let Some(path) = self.serial_log.as_deref() {
            let _ = OpenOptions::new().write(true).truncate(true).open(path);
        }
        self.process = Some(
            self.qemu
                .clone()
                .spawn()
                .map_err(|error| format!("Kore VM startup failed: {error}"))?,
        );
        self.boot_attempt = self.boot_attempt.saturating_add(1);
        self.next_boot_log_check = Some(Instant::now() + BOOT_LOG_GRACE);
        Ok(())
    }

    fn suspend_vm_between_prompts(
        &mut self,
        emit: &mut dyn FnMut(EventKind, bool, Value),
    ) -> Result<(), String> {
        if !self.headless_park || self.vm_suspended {
            return Ok(());
        }
        self.process
            .as_ref()
            .ok_or_else(|| "QEMU process is unavailable".to_owned())?
            .suspend()
            .map_err(|error| format!("cannot park Kore between prompts: {error}"))?;
        self.vm_suspended = true;
        emit(
            EventKind::Validation,
            false,
            json!({"stage": "kore_vm_parked_between_prompts", "guest": "kore"}),
        );
        Ok(())
    }

    fn resume_vm_for_prompt(
        &mut self,
        emit: &mut dyn FnMut(EventKind, bool, Value),
    ) -> Result<(), String> {
        if self.stop_pending {
            return Err("Kore VM termination is still pending".into());
        }
        if !self.vm_suspended {
            return Ok(());
        }
        self.process
            .as_ref()
            .ok_or_else(|| "QEMU process is unavailable".to_owned())?
            .resume()
            .map_err(|error| format!("cannot resume Kore for the next prompt: {error}"))?;
        self.vm_suspended = false;
        emit(
            EventKind::Validation,
            false,
            json!({"stage": "kore_vm_resumed_for_prompt", "guest": "kore"}),
        );
        Ok(())
    }

    fn boot_failure(&mut self) -> Option<String> {
        let now = Instant::now();
        let due = self.next_boot_log_check?;
        if now < due {
            return None;
        }
        self.next_boot_log_check = Some(now + BOOT_LOG_POLL);
        let path = self.serial_log.as_deref()?;
        let tail = read_tail(path, BOOT_LOG_TAIL_BYTES).ok()?;
        [
            "kore: BOOT DEGRADED",
            "first-shell: FAIL",
            "first-userland-boot: FAIL",
        ]
        .into_iter()
        .find(|marker| tail.contains(marker))
        .map(str::to_owned)
    }

    fn model_turn_started(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        if !self.headless_park || self.vm_suspended || self.vm_pause_due.is_some() {
            return;
        }
        // Keep Kore running long enough to consume the inference admission
        // acknowledgement and park its route. TCG can need substantially more
        // host time than the deterministic backend before that edge drains.
        self.vm_pause_due = Some(Instant::now() + VM_PAUSE_DRAIN);
        emit(
            EventKind::Validation,
            false,
            json!({"stage": "host_inference_vm_pause_scheduled", "guest": "kore"}),
        );
    }

    fn maybe_suspend_vm(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        if self.daemon.core().frontend_requires_progress() {
            self.vm_pause_due = None;
            return;
        }
        let Some(due) = self.vm_pause_due else {
            return;
        };
        if self.active_model_turns == 0 {
            self.vm_pause_due = None;
            return;
        }
        if Instant::now() < due {
            return;
        }
        let result = self
            .process
            .as_ref()
            .ok_or_else(|| "QEMU process is unavailable".to_owned())
            .and_then(|process| process.suspend().map_err(|error| error.to_string()));
        match result {
            Ok(()) => {
                self.vm_pause_due = None;
                self.vm_suspended = true;
                emit(
                    EventKind::Validation,
                    false,
                    json!({"stage": "host_inference_vm_suspended", "guest": "kore"}),
                );
            }
            Err(error) => {
                self.vm_pause_due = None;
                emit(
                    EventKind::Error,
                    false,
                    json!({"message": format!("cannot suspend Kore during host inference: {error}")}),
                );
            }
        }
    }

    fn model_turn_finished(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        if self.active_model_turns == 0 {
            self.vm_pause_due = None;
        }
        if self.active_model_turns != 0 || !self.vm_suspended {
            return;
        }
        let result = self
            .process
            .as_ref()
            .ok_or_else(|| "QEMU process is unavailable".to_owned())
            .and_then(|process| process.resume().map_err(|error| error.to_string()));
        match result {
            Ok(()) => {
                self.vm_suspended = false;
                emit(
                    EventKind::Validation,
                    false,
                    json!({"stage": "host_inference_vm_resumed", "guest": "kore"}),
                );
            }
            Err(error) => emit(
                EventKind::Error,
                false,
                json!({"message": format!("cannot resume Kore after host inference: {error}")}),
            ),
        }
    }

    fn drain_events(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        let transcript = self
            .daemon
            .core()
            .transcript()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if self.hostlink_trace_cursor > transcript.len() {
            self.hostlink_trace_cursor = 0;
        }
        for entry in transcript.iter().skip(self.hostlink_trace_cursor) {
            if std::env::var_os("VKORE_HOSTLINK_TRACE").is_some() {
                eprintln!("hostlink: {entry}");
            }
            let message = entry
                .split_once(' ')
                .map_or(entry.as_str(), |(_, message)| message);
            if entry.ends_with("epoch established") {
                timing_marker(emit, "hostlink_epoch_established");
            }
            if entry.ends_with("frontend intent claimed by broker") {
                emit(
                    EventKind::Validation,
                    false,
                    json!({
                        "stage": "kore_broker_claimed_intent",
                        "trusted_user_input": true,
                        "guest": "kore",
                        "transport": "hostlink",
                    }),
                );
            } else if entry.ends_with("frontend broker poll parked") {
                emit(
                    EventKind::Validation,
                    false,
                    json!({
                        "stage": "kore_broker_waiting_for_intent",
                        "guest": "kore",
                        "transport": "hostlink",
                    }),
                );
            } else if entry.ends_with("frontend intent claimed by agent") {
                emit(
                    EventKind::Validation,
                    false,
                    json!({
                        "stage": "kore_agent_claimed_intent",
                        "guest": "kore",
                        "transport": "hostlink",
                    }),
                );
            }
            if let Some(failure) = message.strip_prefix("backend failed ") {
                // This is a per-request inference terminal, not a transport
                // failure. The guest still owns its frontend result and cleanup.
                emit(
                    EventKind::Error,
                    false,
                    json!({
                        "stage": "inference_failure", "message": format!("Inference backend reported {failure}; awaiting the guest outcome.")
                    }),
                );
            }
        }
        self.hostlink_trace_cursor = transcript.len();
        while let Some(observation) = self.daemon.take_inference_observation() {
            emit_inference_observation(observation, emit);
        }
        while let Some(observation) = self.daemon.take_authority_observation() {
            let alias = observation.label.strip_prefix("workspace/");
            let mut directory_contents = Vec::new();
            if observation.kind == AuthorityKind::View
                && let Some(alias) = alias
            {
                for (path, directory) in &self.task_catalog {
                    if matches!(alias, "root" | "project")
                        || path
                            .strip_prefix(alias)
                            .is_some_and(|suffix| suffix.starts_with('/'))
                    {
                        directory_contents.push(json!({"kind":if *directory { "directory" } else { "file" },
                            "path":format!("workspace/{path}"), "backing_path":self.resource_identities.get(path),
                            "rights":observation.rights, "inherited":true}));
                    }
                }
            }
            emit(
                EventKind::Capability,
                false,
                json!({
                    "capabilities": directory_contents,
                    "state": format!("{:?}", observation.state),
                    "kind": format!("{:?}", observation.kind),
                    "agent_slot": observation.agent_slot,
                    "generation": observation.generation,
                    "rights": observation.rights,
                    "backing_path": observation.label.strip_prefix("workspace/")
                        .and_then(|alias| self.resource_identities.get(alias)),
                    "label": observation.label,
                }),
            );
        }
        // Runtime ownership, not telemetry, drives the VM pause barrier.
        // One pending request may emit several templated prompts during a
        // correction; workspace traffic is excluded by WorkspaceBackend.
        self.active_model_turns = self.daemon.core().backend_metrics().pending;
        if self.daemon.core().frontend_requires_progress() {
            self.vm_pause_due = None;
            if let Err(error) = self.resume_vm_for_prompt(emit) {
                emit(
                    EventKind::Error,
                    false,
                    json!({"stage": "guest_settlement_resume_failed", "message": error}),
                );
            }
        } else if self.active_model_turns == 0 {
            self.model_turn_finished(emit);
        } else {
            self.model_turn_started(emit);
        }
    }
}

impl Drop for ProjectRuntime {
    fn drop(&mut self) {
        let _ = self.stop_process();
    }
}

impl KoreVmRuntime {
    fn run_prompt_inner(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
        response_published: &dyn Fn() -> bool,
        context: &[(bool, String)],
        updates: &dyn Fn() -> Option<AuthorityUpdateRequest>,
    ) -> PromptStatus {
        timing_marker(emit, "runtime_lookup_starting");
        let runtime = match self.ensure_project(&session.project_id, &prompt.text, emit) {
            Ok(runtime) => runtime,
            Err(error) => {
                emit(EventKind::Error, true, json!({"message": error}));
                return PromptStatus::Failed;
            }
        };
        timing_marker(emit, "runtime_ready");
        if runtime.session_id.as_deref() != Some(&session.session_id) {
            runtime.session_id = Some(session.session_id.clone());
        }
        if let Err(error) = runtime.daemon.set_conversation_context(context) {
            emit(
                EventKind::Error,
                true,
                json!({"message": format!("conversation context refused: {error:?}")}),
            );
            return PromptStatus::Failed;
        }
        let intent_id = match runtime.daemon.queue_frontend_intent(prompt.text.clone()) {
            Ok(intent_id) => intent_id,
            Err(error) => {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("frontend queue failed: {error:?}")}),
                );
                return PromptStatus::Failed;
            }
        };
        let mut boot_timing =
            BootTiming::new(runtime.serial_log.clone(), runtime.process.is_none());
        if runtime.process.is_none() {
            timing_marker(emit, "vm_spawn_starting");
            runtime.boot_attempt = 0;
            if let Err(error) = runtime.spawn_vm() {
                runtime.daemon.cancel_frontend_intent(intent_id);
                emit(EventKind::Error, true, json!({"message": error}));
                return PromptStatus::Failed;
            }
            timing_marker(emit, "vm_process_spawned");
        } else if let Err(error) = runtime.resume_vm_for_prompt(emit) {
            runtime.daemon.cancel_frontend_intent(intent_id);
            runtime.recover_supervisor(emit);
            emit(EventKind::Error, true, json!({"message": error}));
            return PromptStatus::Failed;
        }
        emit(
            EventKind::Validation,
            false,
            json!({
                "intent_id": hex16(intent_id),
                "stage": "host_intent_queued",
                "trusted_user_input": true,
                "transport": "hostlink",
                "guest": "kore",
            }),
        );
        let mut deadline = PromptDeadline::new(
            Instant::now(),
            Duration::from_secs(u64::from(runtime.config.resources.prompt_seconds.max(1))),
        );
        let started = Instant::now();
        let mut next_progress = started;
        let tick_interval = hostlink_tick_interval();
        let mut pending_terminal = None;
        let mut response_at = None;
        let mut loop_timing = LoopTiming::default();
        let mut authority_updates = LiveUpdateBridge::new(intent_id);
        let mut cancellation_seen = false;
        let mut cancellation_pending = false;
        let status = loop {
            boot_timing.poll(emit);
            if !cancellation_seen && cancelled() {
                cancellation_seen = true;
                match runtime
                    .daemon
                    .core_mut()
                    .request_frontend_cancellation(intent_id)
                {
                    crate::daemon::FrontendCancelOutcome::QueuedRemoved => {
                        // No broker ever claimed this intent. Preserve the VM
                        // and every unrelated route; there is no guest cleanup
                        // or receipt obligation for work that never reached it.
                        emit(
                            EventKind::Error,
                            true,
                            json!({"message": "Prompt cancelled", "stage": "agent_failure"}),
                        );
                        break PromptStatus::Cancelled;
                    }
                    crate::daemon::FrontendCancelOutcome::Requested => {
                        cancellation_pending = true;
                        deadline.cancelling(Instant::now());
                        // A paused guest cannot poll its control decision or
                        // cancel its own exact inference request. Keep it awake
                        // through publication and native retirement.
                        if let Err(error) = runtime.resume_vm_for_prompt(emit) {
                            runtime.recover_supervisor(emit);
                            emit(EventKind::Error, true, json!({"message": error}));
                            break PromptStatus::Failed;
                        }
                        emit(
                            EventKind::Validation,
                            false,
                            json!({"stage": "kore_cancellation_requested", "intent_id": hex16(intent_id)}),
                        );
                    }
                    crate::daemon::FrontendCancelOutcome::AlreadySettled => {}
                    crate::daemon::FrontendCancelOutcome::Unknown => {
                        runtime.recover_supervisor(emit);
                        emit(
                            EventKind::Error,
                            true,
                            json!({"message": "Cancellation could not resolve the active intent."}),
                        );
                        break PromptStatus::Failed;
                    }
                }
            }
            if deadline.expired(Instant::now()) {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": if pending_terminal.is_some() { "Kore cleanup was not confirmed" } else if cancellation_pending { "Kore cancellation was not confirmed" } else { "Kore prompt timed out" }, "stage": "agent_failure"}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.recover_supervisor(emit);
                break PromptStatus::Failed;
            }
            if !cancellation_pending && pending_terminal.is_none() {
                authority_updates.submit(runtime.daemon.core_mut(), updates, emit);
            }
            let tick_started = Instant::now();
            let restarts_before = runtime.daemon.core().backend_metrics().restarts;
            let tick_result = if runtime.vm_suspended {
                runtime.daemon.tick_frozen()
            } else {
                runtime.daemon.tick()
            };
            let tick_elapsed = tick_started.elapsed();
            loop_timing.record_tick(tick_elapsed);
            // Restarting the local model server is synchronous and has its own
            // startup bound. During that wait the host cannot service guest
            // receipts, so it must not count as a guest cancellation/reap timeout.
            // Credit at most one observed restart; repeated failures stay bounded.
            if (cancellation_pending || pending_terminal.is_some())
                && runtime.daemon.core().backend_metrics().restarts > restarts_before
                && deadline.backend_restart(tick_elapsed)
            {
                if let Some(published_at) = response_at.as_mut() {
                    *published_at += tick_elapsed;
                }
                emit(
                    EventKind::Validation,
                    false,
                    json!({
                        "stage": "cancellation_backend_restart_wait",
                        "elapsed_ms": tick_elapsed.as_millis(),
                    }),
                );
            }
            if let Err(error) = tick_result {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("HostLink failed: {error:?}")}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.recover_supervisor(emit);
                break PromptStatus::Failed;
            }
            runtime.drain_events(emit);
            authority_updates.receive(runtime.daemon.core_mut(), emit);
            if runtime.daemon.core().frontend_interrupted(intent_id) {
                // The old guest may have applied tool effects. A new link
                // epoch cannot authorize replay, nor prove native retirement.
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.recover_supervisor(emit);
                emit(
                    EventKind::Error,
                    pending_terminal.is_none(),
                    json!({
                        "message": "HostLink disconnected after the guest claimed this prompt; execution or cleanup could not be confirmed. The prompt was not replayed.",
                        "stage": "guest_connection_lost",
                    }),
                );
                break PromptStatus::Failed;
            }
            if !cancellation_pending {
                runtime.maybe_suspend_vm(emit);
            }
            let now = Instant::now();
            if now >= next_progress {
                let last_hostlink = runtime
                    .daemon
                    .core()
                    .transcript()
                    .last()
                    .unwrap_or("no HostLink event yet")
                    .to_owned();
                let last_hostlink = bounded_progress_text(&last_hostlink, 512);
                emit(
                    EventKind::Validation,
                    false,
                    json!({
                        "stage": "prompt_progress",
                        "elapsed_ms": started.elapsed().as_millis(),
                        "qemu_pid": runtime.process.as_ref().map(QemuProcess::id),
                        "boot_attempt": runtime.boot_attempt,
                        "active_model_turns": runtime.active_model_turns,
                        "vm_suspended": runtime.vm_suspended,
                        "last_hostlink": last_hostlink,
                    }),
                );
                next_progress = now + PROMPT_PROGRESS_INTERVAL;
            }
            if let Some(marker) = runtime.boot_failure() {
                let attempts = runtime.boot_attempt;
                let serial_log = runtime.serial_log.clone();
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.recover_supervisor(emit);
                emit(
                    EventKind::Error,
                    true,
                    json!({
                        "message": format!(
                            "Kore boot failed after {attempts} attempts ({marker}){}",
                            serial_log.as_deref().map_or_else(String::new, |path| format!(
                                "; serial log: {}",
                                path.display()
                            ))
                        )
                    }),
                );
                break PromptStatus::Failed;
            }
            while pending_terminal.is_none()
                && let Some(result) = runtime.daemon.take_frontend_result()
            {
                if result.intent_id != intent_id {
                    continue;
                }
                let status = match result.kind {
                    FrontendResultKind::Completed => {
                        emit(
                            EventKind::Assistant,
                            true,
                            json!({"text": result.text, "source": "kore_guest"}),
                        );
                        PromptStatus::Completed
                    }
                    FrontendResultKind::Cancelled => {
                        emit(
                            EventKind::Error,
                            true,
                            json!({"message": result.text, "stage": "agent_failure", "source": "kore_guest"}),
                        );
                        PromptStatus::Cancelled
                    }
                    FrontendResultKind::Refused | FrontendResultKind::Failed => {
                        emit(
                            EventKind::Error,
                            true,
                            json!({"message": result.text, "stage": "agent_failure", "source": "kore_guest"}),
                        );
                        PromptStatus::Failed
                    }
                };
                // A backend tick can block while stopping/restarting inference.
                // Once the guest result arrives, allow its receipt and retirement
                // to settle under their own bounded deadline.
                deadline.settling(Instant::now());
                pending_terminal = Some(status);
            }
            if pending_terminal.is_some() && response_published() {
                if response_at.is_none() {
                    response_at = Some(Instant::now());
                    timing_marker(emit, "kore_response_published");
                }
                // The result is durable and available to the client event
                // stream. Only now may the guest release the prompt's worker.
                runtime.daemon.acknowledge_frontend_result(intent_id);
            }
            if let Some(status) = pending_terminal
                && response_published()
                && !authority_updates.pending()
                && runtime.daemon.core_mut().finish_frontend_intent(intent_id)
            {
                timing_marker(emit, "kore_frontend_ready");
                if let Err(error) = runtime.suspend_vm_between_prompts(emit) {
                    runtime.recover_supervisor(emit);
                    emit(EventKind::Error, true, json!({"message": error}));
                    break PromptStatus::Failed;
                }
                break status;
            }
            if unconfirmed_cleanup_timed_out(
                pending_terminal,
                response_published(),
                runtime.daemon.core().frontend_ready(intent_id),
                response_at.map(|at: Instant| at.elapsed()),
            ) {
                // Reclamation can fail after any response, not only an
                // explicit worker failure. Never strand a published final,
                // or pretend the guest confirmed cleanup: report a failed
                // cleanup and request supervisor containment after publication.
                emit(
                    EventKind::Error,
                    false,
                    json!({"message": "Agent cleanup was not confirmed after the response. Supervisor recovery was requested; the desktop remains running. The prompt will not be replayed.", "stage": "agent_cleanup_failed"}),
                );
                emit(
                    EventKind::Validation,
                    false,
                    json!({"stage": "supervisor_recovery_requested", "reason": "no guest reclamation confirmation after published response"}),
                );
                runtime.recover_supervisor(emit);
                break PromptStatus::Failed;
            }
            let exited = runtime
                .process
                .as_mut()
                .and_then(|process| process.try_wait().ok().flatten());
            if let Some(status) = exited {
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.process = None;
                runtime.daemon.force_disconnect();
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("Kore VM exited before replying: {status}")}),
                );
                break PromptStatus::Failed;
            }
            let sleep_started = Instant::now();
            std::thread::sleep(tick_interval);
            loop_timing.sleep += sleep_started.elapsed();
            if Instant::now() >= loop_timing.next_report {
                loop_timing.report(emit, false);
            }
        };
        // VM/link closure can settle a delivered update after the last tick.
        // Preserve that exact outcome before the harness handles unknown work.
        authority_updates.receive(runtime.daemon.core_mut(), emit);
        loop_timing.report(emit, true);
        status
    }
}

impl TaskRuntime for KoreVmRuntime {
    fn prepare(
        &mut self,
        status: &mut dyn FnMut(Value),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let profile = self.application_profile()?;
        let started = Instant::now();
        // A deliberate model/configuration replacement has a different scope
        // from recovering an optional application inside a healthy OS.
        if self
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.config.inference != profile.inference)
        {
            self.runtime.as_mut().unwrap().stop_process()?;
            self.runtime = None;
            self.supervisor = None;
            self.incarnation = self
                .incarnation
                .checked_add(1)
                .ok_or("Runtime generation exhausted")?;
            self.conversations.clear();
        }
        if self.runtime.is_none() {
            self.runtime = Some(ProjectRuntime::start(
                &self.config,
                profile,
                &mut |_, _, payload| {
                    let phase = match payload["stage"].as_str() {
                        Some("model_server_starting") => "model_starting",
                        Some("model_server_ready") => "model_ready",
                        _ => "preparing",
                    };
                    status(
                        json!({"phase":phase,"detail":payload,"elapsed_ms":started.elapsed().as_millis()}),
                    );
                },
                cancelled,
            )?);
        }
        let runtime = self.runtime.as_mut().unwrap();
        if runtime.process.is_none() {
            status(json!({"phase":"vm_booting","elapsed_ms":started.elapsed().as_millis()}));
            runtime.spawn_vm()?;
        } else if runtime.recovering || !runtime.daemon.broker_idle() {
            runtime.recover_supervisor(&mut |_, _, detail| {
                status(json!({"phase":"supervisor_starting","detail":detail}))
            });
        }
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut connected = false;
        let mut boot_timing = BootTiming::new(runtime.serial_log.clone(), true);
        let identity = loop {
            if cancelled() {
                return Err("Assistant startup cancelled".into());
            }
            runtime
                .daemon
                .tick()
                .map_err(|e| format!("HostLink preparation failed: {e:?}"))?;
            if runtime.recovering {
                runtime.recover_supervisor(&mut |_, _, detail| {
                    status(json!({"phase":"supervisor_starting","detail":detail}))
                });
            }
            if let Some(identity) = runtime.observe_supervisor_ready()
                && runtime
                    .daemon
                    .with_idle_backend(|backend| backend.healthy())
                    .unwrap_or(false)
            {
                break identity;
            }
            boot_timing.poll(&mut |_, _, detail| {
                status(json!({
                    "phase":if connected { "supervisor_starting" } else { "vm_booting" },
                    "detail":detail,"elapsed_ms":started.elapsed().as_millis(),
                }))
            });
            if !connected && runtime.daemon.core().epoch().is_some() {
                connected = true;
                status(
                    json!({"phase":"hostlink_connected","elapsed_ms":started.elapsed().as_millis()}),
                );
                status(
                    json!({"phase":"supervisor_starting","elapsed_ms":started.elapsed().as_millis()}),
                );
            }
            if runtime
                .process
                .as_mut()
                .and_then(|p| p.try_wait().ok().flatten())
                .is_some()
            {
                runtime.process = None;
                runtime.daemon.force_disconnect();
                return Err("Kore exited during application startup".into());
            }
            if Instant::now() >= deadline {
                // Retain the OS process and all pending native owners. Retry
                // observes/retires the supervisor; it does not discard the VM.
                return Err("Supervisor readiness was not confirmed. Kore remains running; verify that the selected image explicitly enables the supervisor.".into());
            }
            std::thread::sleep(DEFAULT_TICK);
        };
        if self.supervisor.is_some_and(|previous| previous != identity) {
            self.incarnation = self
                .incarnation
                .checked_add(1)
                .ok_or("Runtime generation exhausted")?;
            self.conversations.clear();
        }
        self.supervisor = Some(identity);
        runtime.drain_events(&mut |_, _, _| {});
        runtime.suspend_vm_between_prompts(&mut |_, _, _| {})?;
        status(
            json!({"phase":"supervisor_ready","incarnation":self.incarnation,
            "supervisor_generation":identity.generation.get(),
            "qemu_pid":runtime.process.as_ref().map(QemuProcess::id),
            "ai_server_pid":runtime.daemon.core().backend_metrics().process_id,
            "elapsed_ms":started.elapsed().as_millis()}),
        );
        Ok(())
    }

    fn supports_authority_updates(&self) -> bool {
        true
    }

    fn run_prompt(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
        response_published: &dyn Fn() -> bool,
    ) -> PromptStatus {
        self.run_prompt_with_updates(
            session,
            prompt,
            emit,
            cancelled,
            response_published,
            &|| None,
        )
    }

    fn run_prompt_with_updates(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
        response_published: &dyn Fn() -> bool,
        updates: &dyn Fn() -> Option<AuthorityUpdateRequest>,
    ) -> PromptStatus {
        let timing_started = Instant::now();
        let mut emit_time = Duration::ZERO;
        let mut event_count = 0u64;
        let mut timed_emit = |kind, public, mut payload: Value| {
            if let Some(object) = payload.as_object_mut() {
                object.insert(
                    "runtime_elapsed_ms".into(),
                    json!(timing_started.elapsed().as_secs_f64() * 1000.0),
                );
            }
            event_count += 1;
            let emit_started = Instant::now();
            emit(kind, public, payload);
            emit_time += emit_started.elapsed();
        };
        timing_marker(&mut timed_emit, "runtime_prompt_started");
        let key = (session.project_id.clone(), session.session_id.clone());
        self.conversations
            .retain(|_, context| context.last_used.elapsed() < CONVERSATION_IDLE);
        let context = self
            .conversations
            .get(&key)
            .map(|c| c.exchanges.clone())
            .unwrap_or_default();
        let mut response = None;
        let status = self.run_prompt_inner(
            session,
            prompt,
            &mut |kind, public, payload| {
                if kind == EventKind::Assistant && public {
                    response = payload
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                timed_emit(kind, public, payload);
            },
            cancelled,
            response_published,
            &context,
            updates,
        );
        timing_marker(&mut timed_emit, "runtime_prompt_finished");
        emit(
            EventKind::Validation,
            false,
            json!({
                "stage": "runtime_timing_summary",
                "runtime_elapsed_ms": timing_started.elapsed().as_secs_f64() * 1000.0,
                "event_publish_ms": emit_time.as_secs_f64() * 1000.0,
                "event_count": event_count,
                "status": format!("{status:?}"),
            }),
        );
        if response_published()
            && let Some(response) = response
        {
            if !self.conversations.contains_key(&key)
                && self.conversations.len() >= CONVERSATIONS_MAX
            {
                if let Some(oldest) = self
                    .conversations
                    .iter()
                    .min_by_key(|(_, context)| context.last_used)
                    .map(|(key, _)| key.clone())
                {
                    self.conversations.remove(&oldest);
                }
            }
            let context = self
                .conversations
                .entry(key)
                .or_insert_with(|| ConversationContext {
                    exchanges: Vec::new(),
                    last_used: Instant::now(),
                });
            context.last_used = Instant::now();
            remember_exchange(&mut context.exchanges, &prompt.text, &response);
        }
        status
    }

    fn close(&mut self, session: &SessionState, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        self.conversations
            .remove(&(session.project_id.clone(), session.session_id.clone()));
        if let Some(runtime) = self.runtime.as_mut()
            && runtime.session_id.as_deref() == Some(&session.session_id)
        {
            let _ = runtime.daemon.set_conversation_context(&[]);
            runtime.session_id = None;
        }
        emit(
            EventKind::Capability,
            false,
            json!({"state": "teardown", "project": session.project_id}),
        );
    }

    fn runtime_status(&mut self) -> Option<Value> {
        let runtime = self.runtime.as_mut()?;
        let alive = runtime
            .process
            .as_mut()
            .is_some_and(|process| matches!(process.try_wait(), Ok(None)));
        if !alive {
            runtime.process = None;
            runtime.daemon.force_disconnect();
            return Some(json!({"phase":"failed","incarnation":self.incarnation,
                "message":"Kore is no longer running. Retry starts the OS again."}));
        }
        // Keep transport/retirement progressing while the UI is idle. Never
        // drop the VM just because the optional supervisor lacks readiness.
        if let Err(error) = runtime.daemon.tick() {
            runtime.recover_supervisor(&mut |_, _, _| {});
            return Some(
                json!({"phase":"failed","message":format!("HostLink recovery pending: {error:?}. Kore remains running.")}),
            );
        }
        if runtime.recovering {
            runtime.recover_supervisor(&mut |_, _, _| {});
        }
        let Some(identity) = runtime.observe_supervisor_ready() else {
            return Some(json!({"phase":"failed","incarnation":self.incarnation,
                "qemu_pid":runtime.process.as_ref().map(QemuProcess::id),
                "message":"Supervisor readiness is unavailable. Kore remains running. Retry recovers the supervisor without replaying any prompt."}));
        };
        if self.supervisor.is_some_and(|previous| previous != identity) {
            self.incarnation = self.incarnation.saturating_add(1);
            self.conversations.clear();
        }
        self.supervisor = Some(identity);
        if !runtime
            .daemon
            .with_idle_backend(|backend| backend.healthy())
            .unwrap_or(false)
        {
            return Some(json!({"phase":"failed","incarnation":self.incarnation,
                "qemu_pid":runtime.process.as_ref().map(QemuProcess::id),
                "message":"The model backend is not ready. Kore remains running while model recovery is checked."}));
        }
        Some(
            json!({"phase":"supervisor_ready","incarnation":self.incarnation,
            "supervisor_generation":identity.generation.get(),
            "qemu_pid":runtime.process.as_ref().map(QemuProcess::id),
            "ai_server_pid":runtime.daemon.core().backend_metrics().process_id}),
        )
    }

    fn backend_incarnation(&self) -> u64 {
        self.incarnation
    }
}

fn timing_marker(emit: &mut dyn FnMut(EventKind, bool, Value), stage: &str) {
    emit(
        EventKind::Validation,
        false,
        json!({"stage": stage, "timing": true}),
    );
}

struct LoopTiming {
    ticks: u64,
    tick: Duration,
    max_tick: Duration,
    sleep: Duration,
    next_report: Instant,
}

impl Default for LoopTiming {
    fn default() -> Self {
        Self {
            ticks: 0,
            tick: Duration::ZERO,
            max_tick: Duration::ZERO,
            sleep: Duration::ZERO,
            next_report: Instant::now() + Duration::from_secs(5),
        }
    }
}

impl LoopTiming {
    fn record_tick(&mut self, duration: Duration) {
        self.ticks += 1;
        self.tick += duration;
        self.max_tick = self.max_tick.max(duration);
    }
    fn report(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value), final_sample: bool) {
        emit(
            EventKind::Validation,
            false,
            json!({
                "stage": if final_sample { "runtime_loop_totals" } else { "runtime_loop_sample" }, "timing": true,
                "ticks": self.ticks, "tick_ms": self.tick.as_secs_f64() * 1000.0,
                "max_tick_ms": self.max_tick.as_secs_f64() * 1000.0,
                "sleep_ms": self.sleep.as_secs_f64() * 1000.0,
            }),
        );
        self.next_report = Instant::now() + Duration::from_secs(5);
    }
}

// These are host observations of guest serial output, not guest CPU timers.
// Poll incrementally, bounded per pass. Buffered guest output and host scheduling
// may delay observation; never use these markers as runtime authority/readiness.
struct BootTiming {
    path: Option<PathBuf>,
    offset: u64,
    pending: String,
    seen: std::collections::BTreeSet<String>,
    next_poll: Instant,
}

impl BootTiming {
    fn new(path: Option<PathBuf>, enabled: bool) -> Self {
        Self {
            path: if enabled { path } else { None },
            offset: 0,
            pending: String::new(),
            seen: Default::default(),
            next_poll: Instant::now(),
        }
    }
    fn poll(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        if Instant::now() < self.next_poll {
            return;
        }
        self.next_poll = Instant::now() + Duration::from_millis(10);
        let Some(path) = &self.path else {
            return;
        };
        let Ok(mut file) = std::fs::File::open(path) else {
            return;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut bytes = [0u8; 16384];
        let Ok(count) = file.read(&mut bytes) else {
            return;
        };
        self.offset += count as u64;
        self.pending
            .push_str(&String::from_utf8_lossy(&bytes[..count]));
        while let Some(newline) = self.pending.find('\n') {
            let line = bounded_progress_text(self.pending[..newline].trim(), 512);
            self.pending.drain(..=newline);
            let marker = if line.contains("kore: _start") {
                Some("kernel_entry")
            } else if line.contains("first-userland-boot: services founded") {
                Some("all_services_ready")
            } else if line.contains("resident agent broker founded") {
                Some("resident_broker_initialized")
            } else if line.contains("first-userland-boot:")
                && (line.contains(" ready (") || line.ends_with(" founded"))
            {
                Some("guest_service")
            } else {
                None
            };
            if let Some(marker) = marker {
                let label = if marker == "guest_service" {
                    line.clone()
                } else {
                    marker.to_owned()
                };
                if self.seen.len() < 128 && self.seen.insert(label) {
                    emit(
                        EventKind::Validation,
                        false,
                        json!({
                            "stage": "guest_boot_observation", "marker": marker,
                            "detail": line, "clock": "host_observed", "poll_interval_ms": 10,
                        }),
                    );
                }
            }
        }
        if self.pending.len() > 16384 {
            self.pending.clear();
        }
    }
}

fn remember_exchange(history: &mut Vec<(bool, String)>, user: &str, assistant: &str) {
    // Retain whole exchanges only. No tool grants, hidden reasoning, or
    // rejected model output survive a prompt-scoped authority teardown.
    history.push((false, user.to_owned()));
    history.push((true, assistant.to_owned()));
    while history.len() > 32
        || history.iter().map(|(_, text)| text.len()).sum::<usize>() > 16 * 1024
    {
        history.drain(..2);
    }
}

fn hostlink_tick_interval() -> Duration {
    std::env::var("VKORE_HOSTLINK_TICK_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|milliseconds| (1..=100).contains(milliseconds))
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TICK)
}

fn bounded_progress_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    format!("{}…", &value[..boundary])
}

/// Response and guest retirement are distinct protocol phases. A response
/// accepted during a slow backend tick must not inherit an expired request timer.
struct PromptDeadline {
    expires: Instant,
    restart_credited: bool,
}

impl PromptDeadline {
    fn new(now: Instant, budget: Duration) -> Self {
        Self {
            expires: now + budget,
            restart_credited: false,
        }
    }
    fn cancelling(&mut self, now: Instant) {
        self.expires = now + Duration::from_secs(10);
    }
    fn settling(&mut self, now: Instant) {
        self.expires = now + Duration::from_secs(5);
    }
    fn backend_restart(&mut self, elapsed: Duration) -> bool {
        if self.restart_credited {
            return false;
        }
        self.restart_credited = true;
        self.expires += elapsed;
        true
    }
    fn expired(&self, now: Instant) -> bool {
        now >= self.expires
    }
}

fn unconfirmed_cleanup_timed_out(
    pending: Option<PromptStatus>,
    published: bool,
    ready: bool,
    response_age: Option<Duration>,
) -> bool {
    pending.is_some()
        && published
        && !ready
        && response_age.is_some_and(|age| age >= Duration::from_secs(5))
}

fn reserve_loopback_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| format!("cannot reserve loopback port: {error}"))
}

fn hex16(id: FrontendIntentId) -> String {
    let mut output = String::with_capacity(32);
    for byte in id.0 {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn without_qemu_resource_arguments(arguments: &[OsString]) -> Result<Vec<OsString>, String> {
    let mut filtered = Vec::with_capacity(arguments.len());
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        if argument == "-m" || argument == "-smp" {
            if index + 1 == arguments.len() {
                return Err(format!("{argument} is missing its value"));
            }
            index += 2;
            continue;
        }
        if argument.starts_with("-m=") || argument.starts_with("-smp=") {
            index += 1;
            continue;
        }
        filtered.push(arguments[index].clone());
        index += 1;
    }
    Ok(filtered)
}

fn qemu_serial_log(arguments: &[OsString]) -> Option<PathBuf> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        let value = if argument == "-serial" {
            index += 1;
            arguments.get(index)?.to_string_lossy()
        } else if let Some(value) = argument.strip_prefix("-serial=") {
            value.into()
        } else {
            index += 1;
            continue;
        };
        return value.strip_prefix("file:").map(PathBuf::from);
    }
    None
}

fn read_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    if length > max_bytes {
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(length - max_bytes))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn runtime_restart_waits_for_owned_process_and_pending_stop_before_touching_artifacts() {
        use super::*;
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let store = ProjectStore::open(root.path().join("projects")).unwrap();
        let config = store
            .create_from_workspace(
                &workspace,
                crate::InferenceConfig {
                    backend: RuntimeBackend::Deterministic,
                    kore_image: None,
                    llama_server: None,
                    model: None,
                    extra_args: vec![],
                    context_tokens: 4096,
                    output_tokens: 256,
                    threads: 1,
                    slots: 2,
                },
            )
            .unwrap()
            .project;
        let backend: Box<dyn InferenceBackend + Send> =
            Box::new(crate::backend::DeterministicBackend::new(1));
        let backend = WorkspaceBackend::open_with_inference(
            &workspace,
            Some(&config.workspace.state),
            true,
            backend,
        )
        .unwrap();
        let (daemon, _) = HostLinkDaemon::bind_dynamic_tcp(backend, ()).unwrap();
        let iso = root.path().join("unused.iso");
        let serial = root.path().join("serial.log");
        std::fs::write(&iso, []).unwrap();
        std::fs::write(&serial, b"previous guest evidence").unwrap();
        // Exercise ProjectRuntime's real spawn/stop paths with an OS child.
        // This fixture does not boot Kore or emulate a successful QMP server.
        let qemu = QemuConfig {
            executable: "/bin/sh".into(),
            kore_iso: iso,
            arguments: vec!["-c".into(), "exec sleep 60".into()],
            hostlink_arguments: vec![],
        };
        let process = Some(qemu.clone().spawn().unwrap());
        let mut runtime = ProjectRuntime {
            config,
            daemon,
            qemu,
            process,
            stop_pending: false,
            session_id: Some("retained-session".into()),
            serial_log: Some(serial.clone()),
            boot_attempt: 2,
            next_boot_log_check: None,
            hostlink_trace_cursor: 0,
            active_model_turns: 0,
            vm_pause_due: None,
            vm_suspended: false,
            resource_identities: BTreeMap::new(),
            task_catalog: Vec::new(),
            focus_attached: false,
            headless_park: false,
            recovering: false,
            retiring_supervisor: None,
            supervisor_stop_sent: false,
        };
        let pid = runtime.process.as_ref().unwrap().id();
        assert!(runtime.spawn_vm().is_err());
        assert_eq!(runtime.process.as_ref().unwrap().id(), pid);
        assert_eq!(runtime.boot_attempt, 2);
        assert_eq!(std::fs::read(&serial).unwrap(), b"previous guest evidence");
        runtime.stop_pending = true;
        runtime.vm_suspended = true;
        assert!(
            runtime
                .resume_vm_for_prompt(&mut |_, _, _| panic!("resumed pending stop"))
                .is_err()
        );
        // Successful termination must not attempt QMP resume: this subprocess
        // has no QMP server, despite the runtime's suspended posture.
        runtime.stop_process().unwrap();
        assert!(runtime.process.is_none());
        assert!(!runtime.stop_pending && !runtime.vm_suspended);
        assert_eq!(runtime.boot_attempt, 2, "boot retry count is retained");
        assert_eq!(runtime.session_id.as_deref(), Some("retained-session"));
        // An unresolved stop also blocks startup when no process is present.
        runtime.stop_pending = true;
        assert!(runtime.spawn_vm().is_err());
        assert_eq!(std::fs::read(&serial).unwrap(), b"previous guest evidence");
        runtime.stop_process().unwrap();
        runtime.spawn_vm().unwrap();
        assert!(runtime.process.is_some());
        assert_eq!(runtime.boot_attempt, 3);
        assert!(std::fs::read(&serial).unwrap().is_empty());
        let surviving_pid = runtime.process.as_ref().unwrap().id();
        runtime.recover_supervisor(&mut |_, _, _| {});
        assert!(runtime.recovering);
        assert_eq!(runtime.process.as_ref().unwrap().id(), surviving_pid);
        assert!(
            runtime
                .process
                .as_mut()
                .unwrap()
                .try_wait()
                .unwrap()
                .is_none()
        );
        assert!(runtime.observe_supervisor_ready().is_none());
        assert!(runtime.stop(&mut |_, _, _| panic!("unexpected stop failure")));
        assert!(runtime.process.is_none());
        assert_eq!(runtime.boot_attempt, 0);
    }

    #[cfg(unix)]
    #[test]
    fn failed_vm_termination_retains_the_exact_process_until_reap() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let iso = directory.path().join("unused.iso");
        std::fs::write(&iso, []).unwrap();
        // A real owned OS child, not a guest VM. The shell execs sleep so
        // QemuProcess owns the only process throughout cleanup and on panic.
        let mut process = Some(
            QemuConfig {
                executable: "/bin/sh".into(),
                kore_iso: iso,
                arguments: vec!["-c".into(), "exec sleep 60".into()],
                hostlink_arguments: Vec::new(),
            }
            .spawn()
            .unwrap(),
        );
        let pid = process.as_ref().unwrap().id();
        for _ in 0..2 {
            assert!(
                retire_process(&mut process, |_| {
                    Err(std::io::Error::other("injected termination failure"))
                })
                .is_err()
            );
            assert_eq!(process.as_ref().unwrap().id(), pid);
            assert!(process.as_mut().unwrap().try_wait().unwrap().is_none());
        }
        let mut reaped = None;
        retire_process(&mut process, |child| {
            reaped = Some(child.terminate()?);
            Ok(())
        })
        .unwrap();
        assert!(reaped.is_some());
        assert!(process.is_none());
        retire_process(&mut process, |_| panic!("already reaped")).unwrap();
    }

    #[test]
    fn runtime_timing_preserves_startup_failure_and_public_event_semantics() {
        use super::*;
        let root = tempfile::tempdir().unwrap();
        let mut runtime = KoreVmRuntime::new(KoreVmRuntimeConfig {
            qemu_executable: root.path().join("unused-qemu"),
            qemu_arguments: vec![],
            project_root: root.path().join("projects"),
            require_real: true,
            headless_park: false,
        })
        .unwrap();
        let session = SessionState {
            api_version: 1,
            project_id: "missing".into(),
            session_id: "timing".into(),
            status: crate::SessionStatus::Open,
            generation: 1,
            backend_incarnation: 0,
            last_event_sequence: 0,
            prompts: BTreeMap::new(),
        };
        let prompt = PromptRequest {
            api_version: 1,
            prompt_id: "p".into(),
            idempotency_key: "p".into(),
            text: "hello".into(),
        };
        let mut events = Vec::new();
        let status = runtime.run_prompt(
            &session,
            &prompt,
            &mut |kind, public, payload| events.push((kind, public, payload)),
            &|| false,
            &|| false,
        );
        assert_eq!(status, PromptStatus::Failed);
        assert!(runtime.runtime.is_none());
        assert!(runtime.conversations.is_empty());
        let public: Vec<_> = events.iter().filter(|(_, public, _)| *public).collect();
        assert_eq!(public.len(), 1);
        assert_eq!(public[0].0, EventKind::Error);
        assert!(!events.iter().any(|(_, _, p)| p["stage"] == "runtime_ready"));
        let mut previous = 0.0;
        for (_, _, payload) in &events {
            let elapsed = payload["runtime_elapsed_ms"].as_f64().unwrap();
            assert!(elapsed >= previous);
            previous = elapsed;
        }
        let (_, public, summary) = events.last().unwrap();
        assert!(!public);
        assert_eq!(summary["stage"], "runtime_timing_summary");
        assert_eq!(summary["status"], "Failed");
        assert_eq!(
            summary["event_count"].as_u64(),
            Some(events.len() as u64 - 1)
        );
    }

    #[test]
    fn boot_timing_bounds_diagnostic_marker_storage() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        for index in 0..200 {
            writeln!(file, "first-userland-boot: service-{index} founded").unwrap();
        }
        let mut timing = super::BootTiming::new(Some(file.path().to_owned()), true);
        let mut count = 0;
        timing.poll(&mut |_, _, _| count += 1);
        assert_eq!(count, 128);
        assert_eq!(timing.seen.len(), 128);
    }

    #[test]
    fn boot_timing_waits_for_complete_lines_and_does_not_repeat_markers() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "kore: _sta").unwrap();
        let mut timing = super::BootTiming::new(Some(file.path().to_owned()), true);
        let mut events = Vec::new();
        timing.poll(&mut |_, _, payload| events.push(payload));
        assert!(events.is_empty());
        writeln!(file, "rt").unwrap();
        writeln!(file, "kore: _start").unwrap();
        writeln!(file, "first-userland-boot: kore-svc-init ready (14/17)").unwrap();
        timing.next_poll = std::time::Instant::now();
        timing.poll(&mut |_, _, payload| events.push(payload));
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["marker"], "kernel_entry");
        assert_eq!(events[1]["marker"], "guest_service");
        timing.next_poll = std::time::Instant::now();
        timing.poll(&mut |_, _, payload| events.push(payload));
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn session_memory_keeps_bounded_whole_exchanges() {
        let mut history = Vec::new();
        for index in 0..20 {
            super::remember_exchange(
                &mut history,
                &format!("user-{index}"),
                &format!("answer-{index}"),
            );
        }
        assert_eq!(history.len(), 32);
        assert_eq!(history[0], (false, "user-4".to_owned()));
        assert_eq!(history[31], (true, "answer-19".to_owned()));
        super::remember_exchange(&mut history, &"x".repeat(16 * 1024), "answer");
        assert!(history.is_empty());
    }
    use super::{qemu_serial_log, read_tail, without_qemu_resource_arguments};
    use crate::PromptStatus;
    use std::ffi::OsString;

    #[test]
    fn cancelled_result_after_slow_tick_has_bounded_retirement_time() {
        let start = std::time::Instant::now();
        let mut deadline = super::PromptDeadline::new(start, std::time::Duration::from_secs(600));
        deadline.cancelling(start);
        let result_at = start + std::time::Duration::from_secs(17);
        assert!(deadline.expired(result_at));
        deadline.settling(result_at);
        assert!(!deadline.expired(result_at + std::time::Duration::from_secs(4)));
        assert!(deadline.expired(result_at + std::time::Duration::from_secs(5)));
    }

    #[test]
    fn backend_restart_does_not_consume_guest_retirement_budget_or_extend_it_twice() {
        let start = std::time::Instant::now();
        let mut deadline = super::PromptDeadline::new(start, std::time::Duration::from_secs(600));
        deadline.cancelling(start);
        deadline.settling(start + std::time::Duration::from_millis(250));
        assert!(deadline.backend_restart(std::time::Duration::from_secs(32)));
        assert!(!deadline.expired(start + std::time::Duration::from_secs(36)));
        assert!(deadline.expired(start + std::time::Duration::from_secs(38)));
        assert!(!deadline.backend_restart(std::time::Duration::from_secs(32)));
        assert!(deadline.expired(start + std::time::Duration::from_secs(38)));
    }

    #[test]
    fn cleanup_timeout_never_precedes_response_publication() {
        use std::time::Duration;
        let check = super::unconfirmed_cleanup_timed_out;
        for status in [
            PromptStatus::Completed,
            PromptStatus::Failed,
            PromptStatus::Cancelled,
        ] {
            assert!(!check(
                Some(status),
                false,
                false,
                Some(Duration::from_secs(60))
            ));
            assert!(!check(
                Some(status),
                true,
                false,
                Some(Duration::from_secs(4))
            ));
            assert!(check(
                Some(status),
                true,
                false,
                Some(Duration::from_secs(5))
            ));
            assert!(!check(
                Some(status),
                true,
                true,
                Some(Duration::from_secs(60))
            ));
        }
        assert!(!check(None, true, false, Some(Duration::from_secs(60))));
        assert!(!check(Some(PromptStatus::Completed), true, false, None));
    }

    #[test]
    fn project_resource_limits_replace_caller_qemu_values() {
        let arguments =
            ["-machine", "q35", "-m", "8G", "-smp=12", "-display", "none"].map(OsString::from);
        assert_eq!(
            without_qemu_resource_arguments(&arguments).unwrap(),
            ["-machine", "q35", "-display", "none"].map(OsString::from)
        );
        assert!(without_qemu_resource_arguments(&[OsString::from("-m")]).is_err());
    }

    #[test]
    fn qemu_serial_file_is_discovered_for_boot_failure_diagnostics() {
        let split =
            ["-display", "none", "-serial", "file:/tmp/kore-serial.log"].map(OsString::from);
        assert_eq!(
            qemu_serial_log(&split).unwrap(),
            std::path::Path::new("/tmp/kore-serial.log")
        );
        let joined = ["-serial=file:/tmp/joined.log"].map(OsString::from);
        assert_eq!(
            qemu_serial_log(&joined).unwrap(),
            std::path::Path::new("/tmp/joined.log")
        );
        assert!(qemu_serial_log(&[OsString::from("-serial"), OsString::from("stdio")]).is_none());
    }

    #[test]
    fn serial_tail_keeps_the_terminal_boot_marker() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("serial.log");
        std::fs::write(
            &path,
            format!("{}\nfirst-shell: FAIL (init rung 110)\n", "x".repeat(4096)),
        )
        .unwrap();
        let tail = read_tail(&path, 512).unwrap();
        assert!(!tail.contains(&"x".repeat(4096)));
        assert!(tail.contains("first-shell: FAIL"));
    }
}

#[cfg(test)]
mod native_supervisor_gate;
