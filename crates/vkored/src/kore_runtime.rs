//! Interactive harness runtime backed by a real Kore VM and HostLink.

use crate::{
    BackendLimits, EventKind, HostLinkDaemon, InferenceBackend,
    LlamaServerBackend, LlamaServerConfig, ProjectConfig, ProjectStore, PromptRequest,
    PromptStatus, RuntimeBackend, SessionState, TaskRuntime, WorkspaceAccess, WorkspaceBackend,
    interactive::emit_inference_observation,
};
use kore_hostlink::{AuthorityKind, AuthorityState, FrontendIntentId, FrontendResultKind};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Read;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use vkore_vm::{QemuConfig, QemuProcess};

const DEFAULT_TICK: Duration = Duration::from_millis(1);
const VM_PAUSE_DRAIN: Duration = Duration::from_millis(500);
const BOOT_LOG_GRACE: Duration = Duration::from_secs(1);
const BOOT_LOG_POLL: Duration = Duration::from_millis(250);
const PROMPT_PROGRESS_INTERVAL: Duration = Duration::from_secs(5);
const MAX_BOOT_ATTEMPTS: u8 = 3;
const BOOT_LOG_TAIL_BYTES: u64 = 128 * 1024;

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
    runtimes: BTreeMap<String, ProjectRuntime>,
    incarnation: u64,
    conversations: BTreeMap<(String, String), Vec<(bool, String)>>,
}

struct ProjectRuntime {
    config: ProjectConfig,
    daemon: HostLinkDaemon<Box<dyn InferenceBackend + Send>>,
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
}

impl KoreVmRuntime {
    pub fn new(config: KoreVmRuntimeConfig) -> Result<Self, String> {
        ProjectStore::open(&config.project_root).map_err(|error| error.to_string())?;
        Ok(Self {
            config,
            runtimes: BTreeMap::new(),
            incarnation: 1,
            conversations: BTreeMap::new(),
        })
    }

    fn ensure_project(&mut self, project_id: &str) -> Result<&mut ProjectRuntime, String> {
        // The HTTP project API owns a separate store instance. Reload its
        // atomic JSON files here so newly created or edited projects become
        // effective without restarting the harness daemon.
        let project = ProjectStore::open(&self.config.project_root)
            .map_err(|error| error.to_string())?
            .get(project_id)
            .map_err(|error| error.to_string())?;
        let stale = self
            .runtimes
            .get(project_id)
            .is_some_and(|runtime| runtime.config != project);
        if stale {
            self.runtimes.remove(project_id);
            self.incarnation = self.incarnation.saturating_add(1);
        }
        if !self.runtimes.contains_key(project_id) {
            let runtime = ProjectRuntime::start(&self.config, project)?;
            self.runtimes.insert(project_id.to_owned(), runtime);
        }
        self.runtimes
            .get_mut(project_id)
            .ok_or_else(|| "project runtime disappeared".into())
    }
}

impl ProjectRuntime {
    fn start(runtime: &KoreVmRuntimeConfig, config: ProjectConfig) -> Result<Self, String> {
        let inference: Box<dyn InferenceBackend + Send> = match config.inference.backend {
            RuntimeBackend::Deterministic => return Err(
                "real-only runtime requires llama.cpp; deterministic inference is forbidden".into(),
            ),
            RuntimeBackend::LlamaCpp => {
                let executable = config
                    .inference
                    .llama_server
                    .clone()
                    .ok_or("project has no llama.cpp server")?;
                let model = config
                    .inference
                    .model
                    .clone()
                    .ok_or("project has no model")?;
                let slots = usize::from(config.inference.slots).max(2);
                let limits = BackendLimits {
                    sequences: slots,
                    brokers: 1,
                    agents: slots.saturating_sub(1),
                    max_output_tokens: config.inference.output_tokens,
                    ..BackendLimits::default()
                };
                Box::new(
                    LlamaServerBackend::start(LlamaServerConfig {
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
                    })
                    .map_err(|error| format!("llama.cpp startup failed: {error:?}"))?,
                )
            }
        };
        let allow_writeback = config.workspace.access == WorkspaceAccess::AutomaticWriteback;
        let backend = WorkspaceBackend::open_with_inference(
            &config.workspace.root,
            Some(&config.workspace.state),
            allow_writeback,
            inference,
        )
        .map_err(|error| format!("workspace backend failed: {error:?}"))?;
        let backend: Box<dyn InferenceBackend + Send> = Box::new(backend);
        let (daemon, hostlink) = HostLinkDaemon::bind_dynamic_tcp(backend, ())
            .map_err(|error| format!("HostLink bind failed: {error:?}"))?;
        let kore_iso = config
            .inference
            .kore_image
            .clone()
            .ok_or("project has no Kore image")?;
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
        self.session_id = None;
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
        self.active_model_turns = self.active_model_turns.saturating_add(1);
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
        self.active_model_turns = self.active_model_turns.saturating_sub(1);
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
            if message.starts_with("completed role=Broker ")
                || message.starts_with("completed role=Agent ")
                || message.starts_with("backend failed ")
                || message == "cancelled"
            {
                self.model_turn_finished(emit);
            }
            if let Some(failure) = message.strip_prefix("backend failed ") {
                self.backend_failure = Some(failure.to_owned());
            }
        }
        self.hostlink_trace_cursor = transcript.len();
        while let Some(observation) = self.daemon.take_inference_observation() {
            if observation.kind == crate::ObservationKind::ModelPrompt {
                self.model_turn_started(emit);
            }
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
            emit(
                EventKind::Capability,
                false,
                json!({
                    "state": format!("{:?}", observation.state),
                    "kind": format!("{:?}", observation.kind),
                    "agent_slot": observation.agent_slot,
                    "generation": observation.generation,
                    "rights": observation.rights,
                    "label": observation.label,
                }),
            );
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
        let runtime = match self.ensure_project(&session.project_id) {
            Ok(runtime) => runtime,
            Err(error) => {
                emit(EventKind::Error, true, json!({"message": error}));
                return PromptStatus::Failed;
            }
        };
        if runtime.session_id.as_deref() != Some(&session.session_id) {
            runtime.stop();
            runtime.session_id = Some(session.session_id.clone());
        }
        if let Err(error) = runtime.daemon.set_conversation_context(context) {
            emit(EventKind::Error, true, json!({"message": format!("conversation context refused: {error:?}")}));
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
        if runtime.process.is_none() {
            runtime.boot_attempt = 0;
            if let Err(error) = runtime.spawn_vm() {
                runtime.daemon.cancel_frontend_intent(intent_id);
                emit(EventKind::Error, true, json!({"message": error}));
                return PromptStatus::Failed;
            }
        } else if let Err(error) = runtime.resume_vm_for_prompt(emit) {
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
        let mut reclaimed_at = None;
        let mut response_at = None;
        loop {
            if cancelled() {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": "Prompt cancelled", "stage": "agent_failure"}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                return PromptStatus::Cancelled;
            }
            if Instant::now() >= deadline {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": "Kore prompt timed out", "stage": "agent_failure"}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                return PromptStatus::Failed;
            }
            let tick_result = if runtime.vm_suspended {
                runtime.daemon.tick_frozen()
            } else {
                runtime.daemon.tick()
            };
            if let Err(error) = tick_result {
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("HostLink failed: {error:?}")}),
                );
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
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
                return PromptStatus::Failed;
            }
            if let Some(marker) = runtime.boot_failure() {
                if runtime.boot_attempt < MAX_BOOT_ATTEMPTS {
                    let failed_attempt = runtime.boot_attempt;
                    runtime.stop_process();
                    emit(
                        EventKind::Validation,
                        false,
                        json!({
                            "stage": "kore_boot_retry",
                            "attempt": failed_attempt,
                            "reason": marker,
                        }),
                    );
                    if let Err(error) = runtime.spawn_vm() {
                        runtime.daemon.cancel_frontend_intent(intent_id);
                        emit(EventKind::Error, true, json!({"message": error}));
                        return PromptStatus::Failed;
                    }
                    continue;
                }
                let attempts = runtime.boot_attempt;
                let serial_log = runtime.serial_log.clone();
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop_process();
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
                return PromptStatus::Failed;
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
                pending_terminal = Some(status);
            }
            if pending_terminal.is_some() && response_published() {
                response_at.get_or_insert_with(Instant::now);
                // The result is durable and available to the client event
                // stream. Only now may the guest release the prompt's worker.
                runtime.daemon.acknowledge_frontend_result(intent_id);
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
                    return PromptStatus::Failed;
                }
                return status;
            }
            if let Some(status) = pending_terminal
                && response_published()
                && runtime.completed_frontend_teardowns >= teardown_target
            {
                // Single-prompt guests (including PR909) never issue another
                // BrokerNext poll. Do not manufacture an idle barrier or hang
                // a completed prompt: retire the real VM after confirmed
                // worker reclamation and boot a fresh one for the next prompt.
                let reclaimed = reclaimed_at.get_or_insert_with(Instant::now);
                if reclaimed.elapsed() >= Duration::from_secs(2) {
                    emit(
                        EventKind::Validation,
                        false,
                        json!({"stage": "guest_retired_after_reclaim", "reason": "guest did not return to frontend idle"}),
                    );
                    runtime.stop();
                    return status;
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
                return PromptStatus::Failed;
            }
            std::thread::sleep(tick_interval);
        }
    }

}

impl TaskRuntime for KoreVmRuntime {
    fn run_prompt(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
        response_published: &dyn Fn() -> bool,
    ) -> PromptStatus {
        let key = (session.project_id.clone(), session.session_id.clone());
        let context = self.conversations.get(&key).cloned().unwrap_or_default();
        let mut response = None;
        let status = self.run_prompt_inner(session, prompt, &mut |kind, public, payload| {
            if kind == EventKind::Assistant && public {
                response = payload.get("text").and_then(Value::as_str).map(str::to_owned);
            }
            emit(kind, public, payload);
        }, cancelled, response_published, &context);
        if response_published() && let Some(response) = response {
            remember_exchange(self.conversations.entry(key).or_default(), &prompt.text, &response);
        }
        status
    }

    fn close(&mut self, session: &SessionState, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        self.conversations.remove(&(session.project_id.clone(), session.session_id.clone()));
        if let Some(runtime) = self.runtimes.get_mut(&session.project_id)
            && runtime.session_id.as_deref() == Some(&session.session_id)
        {
            runtime.stop();
            let _ = runtime.daemon.set_conversation_context(&[]);
        }
        emit(
            EventKind::Capability,
            false,
            json!({"state": "teardown", "project": session.project_id}),
        );
    }

    fn backend_incarnation(&self) -> u64 {
        self.incarnation
    }
}

fn remember_exchange(history: &mut Vec<(bool, String)>, user: &str, assistant: &str) {
    // Retain whole exchanges only. No tool grants, hidden reasoning, or
    // rejected model output survive a prompt-scoped authority teardown.
    history.push((false, user.to_owned()));
    history.push((true, assistant.to_owned()));
    while history.len() > 32 || history.iter().map(|(_, text)| text.len()).sum::<usize>() > 16 * 1024 {
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
    fn session_memory_keeps_bounded_whole_exchanges() {
        let mut history = Vec::new();
        for index in 0..20 {
            super::remember_exchange(&mut history, &format!("user-{index}"), &format!("answer-{index}"));
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
        for status in [PromptStatus::Completed, PromptStatus::Failed, PromptStatus::Cancelled] {
            assert!(!check(Some(status), false, 0, 1, Some(Duration::from_secs(60))));
            assert!(!check(Some(status), true, 0, 1, Some(Duration::from_secs(4))));
            assert!(check(Some(status), true, 0, 1, Some(Duration::from_secs(5))));
            assert!(!check(Some(status), true, 1, 1, Some(Duration::from_secs(60))));
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
