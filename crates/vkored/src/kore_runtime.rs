//! Interactive harness runtime backed by a real Kore VM and HostLink.

use crate::{
    BackendLimits, DeterministicBackend, EventKind, HostLinkDaemon, InferenceBackend,
    LlamaServerBackend, LlamaServerConfig, ProjectConfig, ProjectStore, PromptRequest,
    PromptStatus, RuntimeBackend, SessionState, TaskRuntime, WorkspaceAccess, WorkspaceBackend,
    interactive::emit_inference_observation,
};
use kore_hostlink::{FrontendIntentId, FrontendResultKind};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use vkore_vm::{QemuConfig, QemuProcess};

const DEFAULT_TICK: Duration = Duration::from_millis(1);
const VM_PAUSE_DRAIN: Duration = Duration::from_millis(50);

#[derive(Clone, Debug)]
pub struct KoreVmRuntimeConfig {
    pub qemu_executable: PathBuf,
    pub qemu_arguments: Vec<OsString>,
    pub project_root: PathBuf,
}

pub struct KoreVmRuntime {
    config: KoreVmRuntimeConfig,
    runtimes: BTreeMap<String, ProjectRuntime>,
    incarnation: u64,
}

struct ProjectRuntime {
    config: ProjectConfig,
    daemon: HostLinkDaemon<Box<dyn InferenceBackend + Send>>,
    qemu: QemuConfig,
    process: Option<QemuProcess>,
    hostlink_trace_cursor: usize,
    active_model_turns: usize,
    vm_pause_due: Option<Instant>,
    vm_suspended: bool,
}

impl KoreVmRuntime {
    pub fn new(config: KoreVmRuntimeConfig) -> Result<Self, String> {
        ProjectStore::open(&config.project_root).map_err(|error| error.to_string())?;
        Ok(Self {
            config,
            runtimes: BTreeMap::new(),
            incarnation: 1,
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
            RuntimeBackend::Deterministic => Box::new(DeterministicBackend::new(2)),
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
                        trace: false,
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
            hostlink_trace_cursor: 0,
            active_model_turns: 0,
            vm_pause_due: None,
            vm_suspended: false,
        })
    }

    fn stop(&mut self) {
        if self.vm_suspended {
            if let Some(process) = self.process.as_ref() {
                let _ = process.resume();
            }
        }
        self.active_model_turns = 0;
        self.vm_pause_due = None;
        self.vm_suspended = false;
        if let Some(mut process) = self.process.take() {
            let _ = process.terminate();
        }
        self.daemon.force_disconnect();
    }

    fn model_turn_started(&mut self, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        self.active_model_turns = self.active_model_turns.saturating_add(1);
        if self.vm_suspended || self.vm_pause_due.is_some() {
            return;
        }
        // The model worker starts in the daemon tick that accepts the guest's
        // HostLink request. Let that acknowledgement and any paired virtio
        // notifications drain before pausing virtual CPUs; stopping QEMU in
        // the middle of that transaction can strand a freshly opened route.
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
        }
        self.hostlink_trace_cursor = transcript.len();
        while let Some(observation) = self.daemon.take_inference_observation() {
            if observation.kind == crate::ObservationKind::ModelPrompt {
                self.model_turn_started(emit);
            }
            emit_inference_observation(observation, emit);
        }
        while let Some(observation) = self.daemon.take_authority_observation() {
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

impl TaskRuntime for KoreVmRuntime {
    fn run_prompt(
        &mut self,
        session: &SessionState,
        prompt: &PromptRequest,
        emit: &mut dyn FnMut(EventKind, bool, Value),
        cancelled: &dyn Fn() -> bool,
    ) -> PromptStatus {
        let runtime = match self.ensure_project(&session.project_id) {
            Ok(runtime) => runtime,
            Err(error) => {
                emit(EventKind::Error, true, json!({"message": error}));
                return PromptStatus::Failed;
            }
        };
        runtime.stop();
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
        runtime.process = match runtime.qemu.clone().spawn() {
            Ok(process) => Some(process),
            Err(error) => {
                runtime.daemon.cancel_frontend_intent(intent_id);
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("Kore VM startup failed: {error}")}),
                );
                return PromptStatus::Failed;
            }
        };
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
        let tick_interval = hostlink_tick_interval();
        loop {
            if cancelled() {
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                return PromptStatus::Cancelled;
            }
            if Instant::now() >= deadline {
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": "Kore prompt timed out"}),
                );
                return PromptStatus::Failed;
            }
            if let Err(error) = runtime.daemon.tick() {
                runtime.daemon.cancel_frontend_intent(intent_id);
                runtime.stop();
                emit(
                    EventKind::Error,
                    true,
                    json!({"message": format!("HostLink failed: {error:?}")}),
                );
                return PromptStatus::Failed;
            }
            runtime.drain_events(emit);
            runtime.maybe_suspend_vm(emit);
            while let Some(result) = runtime.daemon.take_frontend_result() {
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
                    FrontendResultKind::Cancelled => PromptStatus::Cancelled,
                    FrontendResultKind::Refused | FrontendResultKind::Failed => {
                        emit(EventKind::Error, true, json!({"message": result.text}));
                        PromptStatus::Failed
                    }
                };
                runtime.stop();
                return status;
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

    fn close(&mut self, session: &SessionState, emit: &mut dyn FnMut(EventKind, bool, Value)) {
        if let Some(runtime) = self.runtimes.get_mut(&session.project_id) {
            runtime.stop();
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

fn hostlink_tick_interval() -> Duration {
    std::env::var("VKORE_HOSTLINK_TICK_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|milliseconds| (1..=100).contains(milliseconds))
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_TICK)
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

#[cfg(test)]
mod tests {
    use super::without_qemu_resource_arguments;
    use std::ffi::OsString;

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
}
