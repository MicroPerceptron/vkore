//! Interactive harness runtime backed by a real Kore VM and HostLink.

use crate::{
    BackendLimits, EventKind, HostLinkDaemon, InferenceBackend, LlamaServerBackend,
    LlamaServerConfig, ProjectConfig, ProjectStore, PromptRequest, PromptStatus, RuntimeBackend,
    SessionState, TaskRuntime, WorkspaceAccess, WorkspaceBackend,
    interactive::emit_inference_observation,
};
use kore_hostlink::{AuthorityKind, AuthorityState, FrontendIntentId, FrontendResultKind};
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

#[derive(Clone, Debug)]
pub struct KoreVmRuntimeConfig {
    pub qemu_executable: PathBuf,
    pub qemu_arguments: Vec<OsString>,
    pub project_root: PathBuf,
    /// Compatibility flag; live inference is mandatory even when false.
    pub require_real: bool,
}

pub struct KoreVmRuntime {
    config: KoreVmRuntimeConfig,
    runtime: Option<ProjectRuntime>,
    incarnation: u64,
    conversations: BTreeMap<(String, String), ConversationContext>,
}

struct ProjectRuntime {
    config: ProjectConfig,
    daemon: HostLinkDaemon<WorkspaceBackend<Box<dyn InferenceBackend + Send>>>,
    qemu: QemuConfig,
    process: Option<QemuProcess>,
    session_id: Option<String>,
    serial_log: Option<PathBuf>,
    boot_attempt: u8,
    next_boot_log_check: Option<Instant>,
    hostlink_trace_cursor: usize,
    active_model_turns: usize,
    vm_pause_due: Option<Instant>,
    vm_suspended: bool,
    backend_failure: Option<String>,
    completed_frontend_broker_decisions: u64,
    completed_frontend_teardowns: u64,
    completed_frontend_idle_barriers: u64,
    resource_identities: BTreeMap<String, PathBuf>,
    task_catalog: Vec<(String, bool)>,
    focus_attached: bool,
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
        let discovery = crate::personal::search(query, &settings.authorized_locations)?;
        emit(
            EventKind::Validation,
            false,
            json!({"stage":"personal_discovery_completed",
            "elapsed_ms":discovery_started.elapsed().as_millis(), "entries_scanned":discovery.scanned,
            "candidate_count":discovery.files.len(), "bounded":discovery.bounded}),
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
        runtime.focus_attached = discovery.files.is_empty() && !project.workspace.virtual_only;
        runtime.resource_identities = discovery.identities.into_iter().collect();
        runtime
            .daemon
            .with_idle_backend(|backend| {
                if discovery.files.is_empty() {
                    backend.attach_task_root(
                        &project.workspace.root,
                        &project.workspace.state,
                        project.workspace.access == WorkspaceAccess::AutomaticWriteback,
                    )
                } else {
                    backend.attach_selected(&state.join("runtime/empty"), discovery.files)
                }
            })
            .map_err(|error| format!("Task attachment requires reclaimed broker idle: {error:?}"))?
            .map_err(|error| format!("Task filesystem attachment failed: {error:?}"))?;
        runtime.task_catalog = runtime
            .daemon
            .with_idle_backend(|backend| backend.task_catalog())
            .map_err(|error| format!("Task catalog attachment failed: {error:?}"))?;
        if runtime.focus_attached {
            runtime
                .resource_identities
                .insert("root".into(), project.workspace.root.clone());
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
            serial_log,
            boot_attempt: 0,
            next_boot_log_check: None,
            hostlink_trace_cursor: 0,
            active_model_turns: 0,
            vm_pause_due: None,
            vm_suspended: false,
            backend_failure: None,
            completed_frontend_broker_decisions: 0,
            completed_frontend_teardowns: 0,
            completed_frontend_idle_barriers: 0,
            resource_identities: BTreeMap::new(),
            task_catalog: Vec::new(),
            focus_attached: false,
        })
    }

    fn stop_process(&mut self) {
        if self.vm_suspended
            && let Some(process) = self.process.as_ref()
        {
            let _ = process.resume();
        }
        self.active_model_turns = 0;
        self.vm_pause_due = None;
        self.vm_suspended = false;
        self.next_boot_log_check = None;
        if let Some(mut process) = self.process.take() {
            let _ = process.terminate();
        }
        self.daemon.force_disconnect();
    }

    fn stop(&mut self) {
        self.stop_process();
        // Retiring the VM is not closing the logical conversation. Retain
        // ownership so an explicit session close can clear its model context.
        self.boot_attempt = 0;
    }

    fn spawn_vm(&mut self) -> Result<(), String> {
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
        if self.vm_suspended {
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
        if self.vm_suspended || self.vm_pause_due.is_some() {
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
                self.completed_frontend_idle_barriers =
                    self.completed_frontend_idle_barriers.saturating_add(1);
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
                self.backend_failure = Some(failure.to_owned());
            }
        }
        self.hostlink_trace_cursor = transcript.len();
        while let Some(observation) = self.daemon.take_inference_observation() {
            if observation.role == crate::BackendRole::Broker
                && observation.kind == crate::ObservationKind::BrokerProposal
            {
                self.completed_frontend_broker_decisions =
                    self.completed_frontend_broker_decisions.saturating_add(1);
            }
            emit_inference_observation(observation, emit);
        }
        while let Some(observation) = self.daemon.take_authority_observation() {
            if observation.state == AuthorityState::Teardown
                && observation.kind == AuthorityKind::Agent
                && observation.label == "agent/frontend"
            {
                self.completed_frontend_teardowns =
                    self.completed_frontend_teardowns.saturating_add(1);
            }
            let alias = observation.label.strip_prefix("workspace/");
            let mut directory_contents = Vec::new();
            if observation.kind == AuthorityKind::View
                && let Some(alias) = alias
            {
                for (path, directory) in &self.task_catalog {
                    if alias == "root"
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
        if self.active_model_turns == 0 {
            self.model_turn_finished(emit);
        } else {
            self.model_turn_started(emit);
        }
    }
}

impl Drop for ProjectRuntime {
    fn drop(&mut self) {
        self.stop();
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
        runtime.backend_failure = None;
        let mut boot_timing =
            BootTiming::new(runtime.serial_log.clone(), runtime.process.is_none());
        if runtime.process.is_none() {
            runtime.daemon.cancel_frontend_intent(intent_id);
            emit(
                EventKind::Error,
                true,
                json!({"message":"Shared VM is unavailable; use Retry to restart the assistant"}),
            );
            return PromptStatus::Failed;
        }
        if let Err(error) = runtime.resume_vm_for_prompt(emit) {
            runtime.daemon.cancel_frontend_intent(intent_id);
            runtime.stop();
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
        let deadline = Instant::now()
            + Duration::from_secs(u64::from(runtime.config.resources.prompt_seconds.max(1)));
        let started = Instant::now();
        let mut next_progress = started;
        let tick_interval = hostlink_tick_interval();
        let broker_decision_target = runtime
            .completed_frontend_broker_decisions
            .saturating_add(1);
        let teardown_target = runtime.completed_frontend_teardowns.saturating_add(1);
        let idle_barrier_target = runtime.completed_frontend_idle_barriers.saturating_add(1);
        let mut pending_terminal = None;
        let mut cancellation_started = None;
        let mut cancellation_requested = false;
        let mut broker_reported_failure = false;
        let mut reclaimed_at = None;
        let mut response_at = None;
        let mut loop_timing = LoopTiming::default();
        loop {
            boot_timing.poll(emit);
            if cancelled() && pending_terminal.is_none() {
                cancellation_requested = true;
                cancellation_started.get_or_insert_with(Instant::now);
                runtime.daemon.cancel_frontend_work(intent_id);
                if runtime.vm_suspended {
                    let _ = runtime.resume_vm_for_prompt(emit);
                }
            }
            if cancellation_started
                .is_some_and(|started: Instant| started.elapsed() > Duration::from_secs(30))
            {
                runtime.stop();
                emit(
                    EventKind::Error,
                    true,
                    json!({"message":"Guest cancellation did not reclaim its resources; the shared runtime requires recovery", "stage":"runtime_failed"}),
                );
                return PromptStatus::Failed;
            }
            if Instant::now() >= deadline {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": "Kore prompt timed out", "stage": "agent_failure"}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                loop_timing.report(emit, true);
                return PromptStatus::Failed;
            }
            let tick_started = Instant::now();
            let tick_result = if runtime.vm_suspended {
                runtime.daemon.tick_frozen()
            } else {
                runtime.daemon.tick()
            };
            loop_timing.record_tick(tick_started.elapsed());
            if let Err(error) = tick_result {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("HostLink failed: {error:?}")}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                loop_timing.report(emit, true);
                return PromptStatus::Failed;
            }
            runtime.drain_events(emit);
            runtime.maybe_suspend_vm(emit);
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
            if let Some(failure) = runtime.backend_failure.take() {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("host inference failed: {failure}")}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                loop_timing.report(emit, true);
                return PromptStatus::Failed;
            }
            if let Some(marker) = runtime.boot_failure() {
                let attempts = runtime.boot_attempt;
                let serial_log = runtime.serial_log.clone();
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop_process();
                emit(
                    EventKind::Error,
                    true,
                    json!({
                        "message": format!(
                            "Shared Kore runtime failed ({marker}, boot {attempts}); use Retry{}",
                            serial_log.as_deref().map_or_else(String::new, |path| format!(
                                "; serial log: {}",
                                path.display()
                            ))
                        )
                    }),
                );
                loop_timing.report(emit, true);
                return PromptStatus::Failed;
            }
            while pending_terminal.is_none()
                && let Some(result) = runtime.daemon.take_frontend_result()
            {
                if result.intent_id != intent_id {
                    continue;
                }
                broker_reported_failure = result.source_role == crate::backend::BackendRole::Broker;
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
                pending_terminal = Some(if cancellation_requested {
                    PromptStatus::Cancelled
                } else {
                    status
                });
            }
            if pending_terminal.is_some() && response_published() {
                response_at.get_or_insert_with(Instant::now);
                // The result is durable and available to the client event
                // stream. Only now may the guest release the prompt's worker.
                runtime.daemon.acknowledge_frontend_result(intent_id);
            }
            if broker_reported_failure
                && pending_terminal.is_some()
                && response_published()
                && runtime.completed_frontend_idle_barriers >= idle_barrier_target
            {
                // The retained broker returned to its trusted idle poll after
                // refusing construction. No worker teardown is invented.
                if let Err(error) = runtime.suspend_vm_between_prompts(emit) {
                    runtime.stop();
                    emit(EventKind::Error, true, json!({"message": error}));
                }
                loop_timing.report(emit, true);
                return if cancellation_requested {
                    PromptStatus::Cancelled
                } else {
                    PromptStatus::Failed
                };
            }
            if response_published()
                && let Some(status) = terminal_after_reclaim_and_idle(
                    pending_terminal,
                    runtime.completed_frontend_broker_decisions,
                    broker_decision_target,
                    runtime.completed_frontend_teardowns,
                    teardown_target,
                    runtime.completed_frontend_idle_barriers,
                    idle_barrier_target,
                )
            {
                if let Err(error) = runtime.suspend_vm_between_prompts(emit) {
                    runtime.stop();
                    emit(EventKind::Error, true, json!({"message": error}));
                    loop_timing.report(emit, true);
                    return PromptStatus::Failed;
                }
                loop_timing.report(emit, true);
                return status;
            }
            if pending_terminal.is_some()
                && response_published()
                && runtime.completed_frontend_teardowns >= teardown_target
            {
                // A reusable guest must return to its authenticated idle
                // barrier. A missing barrier is a shared-runtime failure.
                let reclaimed = reclaimed_at.get_or_insert_with(Instant::now);
                if reclaimed.elapsed() >= Duration::from_secs(2) {
                    emit(
                        EventKind::Validation,
                        false,
                        json!({"stage": "runtime_failed", "reason": "guest did not return to frontend idle; explicit recovery required"}),
                    );
                    runtime.stop();
                    loop_timing.report(emit, true);
                    return PromptStatus::Failed;
                }
            }
            if unconfirmed_cleanup_timed_out(
                pending_terminal,
                response_published(),
                runtime.completed_frontend_teardowns,
                teardown_target,
                response_at.map(|at: Instant| at.elapsed()),
            ) {
                // Reclamation can fail after any response, not only an
                // explicit worker failure. Never strand a published final,
                // or pretend the guest confirmed cleanup: report a failed
                // cleanup and terminate the actual VM after publication.
                emit(
                    EventKind::Error,
                    false,
                    json!({"message": "Agent cleanup was not confirmed after its response; the Kore VM was stopped.", "stage": "agent_cleanup_failed"}),
                );
                emit(
                    EventKind::Validation,
                    false,
                    json!({"stage": "failed_guest_shutdown", "reason": "no guest reclamation confirmation after published response"}),
                );
                runtime.stop();
                loop_timing.report(emit, true);
                return PromptStatus::Failed;
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
                loop_timing.report(emit, true);
                return PromptStatus::Failed;
            }
            let sleep_started = Instant::now();
            std::thread::sleep(tick_interval);
            loop_timing.sleep += sleep_started.elapsed();
            if Instant::now() >= loop_timing.next_report {
                loop_timing.report(emit, false);
            }
        }
    }
}

impl TaskRuntime for KoreVmRuntime {
    fn prepare(
        &mut self,
        status: &mut dyn FnMut(Value),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), String> {
        let profile = self.application_profile()?;
        let _ = self.runtime_status();
        if let Some(runtime) = self.runtime.as_ref()
            && runtime.config.inference == profile.inference
            && runtime.daemon.broker_idle()
        {
            status(
                json!({"phase":"broker_ready","incarnation":self.incarnation,
                "qemu_pid":runtime.process.as_ref().map(QemuProcess::id),
                "ai_server_pid":runtime.daemon.core().backend_metrics().process_id}),
            );
            return Ok(());
        }
        if self.runtime.take().is_some() {
            self.incarnation = self
                .incarnation
                .checked_add(1)
                .ok_or("Runtime generation exhausted")?;
            self.conversations.clear();
            status(json!({"phase":"preparing","restart":true,"incarnation":self.incarnation}));
        }
        let started = Instant::now();
        let mut runtime = ProjectRuntime::start(
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
        )?;
        status(json!({"phase":"vm_booting","elapsed_ms":started.elapsed().as_millis()}));
        runtime.spawn_vm()?;
        let mut boot_timing = BootTiming::new(runtime.serial_log.clone(), true);
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut connected = false;
        while !runtime.daemon.broker_idle() {
            if cancelled() {
                return Err("Assistant startup cancelled".into());
            }
            runtime
                .daemon
                .tick()
                .map_err(|e| format!("HostLink preparation failed: {e:?}"))?;
            boot_timing.poll(&mut |_, _, detail| {
                status(json!({
                    "phase": if connected { "hostlink_connected" } else { "vm_booting" },
                    "detail": detail,
                    "elapsed_ms": started.elapsed().as_millis(),
                }));
            });
            if !connected && runtime.daemon.core().epoch().is_some() {
                connected = true;
                status(
                    json!({"phase":"hostlink_connected","elapsed_ms":started.elapsed().as_millis()}),
                );
            }
            if runtime
                .process
                .as_mut()
                .and_then(|p| p.try_wait().ok().flatten())
                .is_some()
            {
                return Err("Kore exited during application startup".into());
            }
            if Instant::now() >= deadline {
                return Err(
                    "Kore did not reach resident broker readiness; verify the headless guest image"
                        .into(),
                );
            }
            std::thread::sleep(DEFAULT_TICK);
        }
        status(json!({"phase":"bootstrap_ready","elapsed_ms":started.elapsed().as_millis()}));
        runtime.drain_events(&mut |_, _, _| {});
        let pid = runtime.process.as_ref().map(QemuProcess::id);
        let ai_server_pid = runtime.daemon.core().backend_metrics().process_id;
        runtime.suspend_vm_between_prompts(&mut |_, _, _| {})?;
        self.runtime = Some(runtime);
        status(
            json!({"phase":"broker_ready","incarnation":self.incarnation,"qemu_pid":pid,"ai_server_pid":ai_server_pid,
            "elapsed_ms":started.elapsed().as_millis()}),
        );
        Ok(())
    }

    fn run_prompt(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
        response_published: &dyn Fn() -> bool,
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
        let healthy = alive
            && runtime
                .daemon
                .with_idle_backend(|backend| backend.healthy())
                .unwrap_or(false);
        if !healthy {
            self.runtime.take();
            self.incarnation = self.incarnation.saturating_add(1);
            self.conversations.clear();
            return Some(json!({"phase":"failed", "incarnation":self.incarnation,
                "message":"Shared assistant runtime lost readiness. Retry restarts it and clears affected task contexts."}));
        }
        Some(
            json!({"phase":"broker_ready", "incarnation":self.incarnation,
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
            let line = self.pending[..newline].trim().to_owned();
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
                if self.seen.insert(label) {
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

fn unconfirmed_cleanup_timed_out(
    pending: Option<PromptStatus>,
    published: bool,
    observed_teardowns: u64,
    teardown_target: u64,
    response_age: Option<Duration>,
) -> bool {
    pending.is_some()
        && published
        && observed_teardowns < teardown_target
        && response_age.is_some_and(|age| age >= Duration::from_secs(5))
}

fn terminal_after_reclaim_and_idle(
    pending: Option<PromptStatus>,
    observed_broker_decisions: u64,
    broker_decision_target: u64,
    observed_teardowns: u64,
    teardown_target: u64,
    observed_idle_barriers: u64,
    idle_barrier_target: u64,
) -> Option<PromptStatus> {
    (observed_broker_decisions >= broker_decision_target
        && observed_teardowns >= teardown_target
        && observed_idle_barriers >= idle_barrier_target)
        .then_some(pending)
        .flatten()
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
    use super::{
        qemu_serial_log, read_tail, terminal_after_reclaim_and_idle,
        without_qemu_resource_arguments,
    };
    use crate::PromptStatus;
    use std::ffi::OsString;

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
                0,
                1,
                Some(Duration::from_secs(60))
            ));
            assert!(!check(
                Some(status),
                true,
                0,
                1,
                Some(Duration::from_secs(4))
            ));
            assert!(check(
                Some(status),
                true,
                0,
                1,
                Some(Duration::from_secs(5))
            ));
            assert!(!check(
                Some(status),
                true,
                1,
                1,
                Some(Duration::from_secs(60))
            ));
        }
        assert!(!check(None, true, 0, 1, Some(Duration::from_secs(60))));
        assert!(!check(Some(PromptStatus::Completed), true, 0, 1, None));
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

    #[test]
    fn frontend_result_is_not_terminal_until_kore_is_ready_for_the_next_chat() {
        assert_eq!(
            terminal_after_reclaim_and_idle(None, 2, 3, 4, 5, 7, 8),
            None
        );
        assert_eq!(
            terminal_after_reclaim_and_idle(Some(PromptStatus::Completed), 2, 3, 4, 5, 7, 8),
            None
        );
        assert_eq!(
            terminal_after_reclaim_and_idle(Some(PromptStatus::Completed), 3, 3, 4, 5, 7, 8),
            None
        );
        assert_eq!(
            terminal_after_reclaim_and_idle(Some(PromptStatus::Completed), 3, 3, 5, 5, 7, 8),
            None
        );
        assert_eq!(
            terminal_after_reclaim_and_idle(Some(PromptStatus::Completed), 3, 3, 5, 5, 8, 8),
            Some(PromptStatus::Completed)
        );
    }
}
