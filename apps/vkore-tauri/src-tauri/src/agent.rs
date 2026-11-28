//! The agent tile's side of `vkore-agent`: one session for each machine a person
//! has turned the agent on for, each on its own thread.
//!
//! The agent's loop, tools and approval rule live in `vkore-agent`. This module
//! is its host (it acts on a machine through the same runtime and volume code the
//! window uses) and its front end (it keeps a bounded transcript to draw, and
//! carries a person's answer back).
//!
//! - **Off until turned on**, machine by machine: until then nothing about a
//!   machine is read or sent anywhere.
//! - **An answer comes only from the window**, naming the request it answers.
//! - **Text from the model or the guest is escaped** before it is kept, and the
//!   transcript is bounded.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use vkore_agent::session::MAX_PROMPT_BYTES;
use vkore_agent::{
    Action, Ask, Choice, Ended, Endpoint, Event, FilePath, Gate, Host, HttpModel, Model,
    NotConfigured, Outcome, Remember, Session, Terminal, Ui,
};

use crate::runtime::{Refusal, Runtime};
use crate::shown::{escape_text, shown};
use crate::views;
use crate::volumes::{ListingView, PreviewView, Volumes, Where};

/// The most machines the agent is on for at once.
pub const MAX_AGENTS: usize = 8;
/// The most transcript entries kept for one machine.
pub const MAX_ENTRIES: usize = 80;
/// The most text the kept entries hold together.
pub const MAX_VIEW_BYTES: usize = 128 << 10;
/// The most of an action's result the window shows.
const MAX_OUTPUT_BYTES: usize = 2 << 10;
/// The fewest time between two updates while a reply streams in.
const FRAME: Duration = Duration::from_millis(60);
/// The newest boot events in a status report.
const STATUS_EVENTS: usize = 20;

/// A request waiting for the person.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AskView {
    pub id: String,
    pub title: &'static str,
    /// Exactly what would be typed or touched.
    pub detail: String,
    /// What "allow for this session" would cover: `exact`, `kind`, or absent.
    pub remember: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Entry {
    Prompt {
        id: u64,
        text: String,
    },
    Reply {
        id: u64,
        text: String,
    },
    Action {
        id: u64,
        title: &'static str,
        detail: String,
        /// `pending`, `done`, `failed`, `denied` or `stopped`.
        state: &'static str,
        output: String,
    },
    Note {
        id: u64,
        text: String,
        bad: bool,
    },
}

impl Entry {
    fn weight(&self) -> usize {
        match self {
            Entry::Prompt { text, .. } | Entry::Reply { text, .. } | Entry::Note { text, .. } => {
                text.len()
            }
            Entry::Action { detail, output, .. } => detail.len() + output.len(),
        }
    }
}

/// Everything the agent tile draws.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentView {
    pub name: String,
    pub configured: bool,
    /// Why there is no endpoint, when there is none.
    pub problem: String,
    pub model: String,
    pub host: String,
    /// The endpoint is on this computer, so nothing leaves it.
    pub local: bool,
    pub enabled: bool,
    pub busy: bool,
    pub entries: Vec<Entry>,
    pub ask: Option<AskView>,
}

/// One machine's transcript and state, shared with its session's thread.
#[derive(Default)]
struct Shared {
    entries: VecDeque<Entry>,
    ask: Option<AskView>,
    busy: bool,
    /// Turned off: the thread's remaining updates go nowhere.
    closed: bool,
    next: u64,
}

impl Shared {
    fn id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    fn push(&mut self, entry: Entry) {
        self.entries.push_back(entry);
        self.bound();
    }

    fn bound(&mut self) {
        let mut weight: usize = self.entries.iter().map(Entry::weight).sum();
        while self.entries.len() > 1
            && (self.entries.len() > MAX_ENTRIES || weight > MAX_VIEW_BYTES)
        {
            if let Some(entry) = self.entries.pop_front() {
                weight -= entry.weight();
            }
        }
    }

    fn note(&mut self, text: impl Into<String>, bad: bool) {
        let id = self.id();
        let text = text.into();
        self.push(Entry::Note { id, text, bad });
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// How a session's updates reach the window.
pub type Publish = Arc<dyn Fn(AgentView) + Send + Sync>;

#[derive(Clone)]
struct Live {
    prompts: SyncSender<String>,
    answers: SyncSender<(String, Choice)>,
    cancel: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    publish: Publish,
}

struct Inner {
    endpoint: Result<Endpoint, NotConfigured>,
    audit: std::path::PathBuf,
    live: Mutex<HashMap<String, Live>>,
}

/// The agents this window runs.
#[derive(Clone)]
pub struct Agents(Arc<Inner>);

fn view(inner: &Inner, name: &str, shared: Option<&Shared>) -> AgentView {
    let endpoint = inner.endpoint.as_ref().ok();
    AgentView {
        name: name.to_owned(),
        configured: endpoint.is_some(),
        problem: inner
            .endpoint
            .as_ref()
            .err()
            .map_or_else(String::new, |why| shown(&why.to_string())),
        model: endpoint.map_or_else(String::new, |e| shown(e.model())),
        host: endpoint.map_or_else(String::new, |e| shown(e.host())),
        local: endpoint.is_some_and(Endpoint::is_local),
        enabled: shared.is_some(),
        busy: shared.is_some_and(|s| s.busy),
        entries: shared.map_or_else(Vec::new, |s| s.entries.iter().cloned().collect()),
        ask: shared.and_then(|s| s.ask.clone()),
    }
}

/// The session's front end, on the session's thread.
struct Window {
    inner: Arc<Inner>,
    name: String,
    shared: Arc<Mutex<Shared>>,
    publish: Publish,
    answers: Receiver<(String, Choice)>,
    cancel: Arc<AtomicBool>,
    /// The reply entry being streamed, and the entry of the call being handled.
    reply: Option<u64>,
    action: Option<u64>,
    published: Instant,
}

impl Window {
    fn send(&mut self) {
        let snapshot = {
            let shared = lock(&self.shared);
            if shared.closed {
                return;
            }
            view(&self.inner, &self.name, Some(&shared))
        };
        self.published = Instant::now();
        (self.publish)(snapshot);
    }

    fn finished(&mut self, ended: Ended) {
        {
            let mut shared = lock(&self.shared);
            shared.busy = false;
            shared.ask = None;
            match ended {
                Ended::Finished => {}
                Ended::Stopped => shared.note("Stopped.", false),
                Ended::StepLimit => shared.note(
                    "Stopped after the most steps one prompt may take. Ask again to go on.",
                    false,
                ),
                Ended::Failed(why) => shared.note(shown(&why), true),
            }
        }
        self.reply = None;
        self.action = None;
        self.send();
    }
}

impl Ui for Window {
    fn event(&mut self, event: Event<'_>) {
        let mut now = true;
        {
            let mut shared = lock(&self.shared);
            match event {
                Event::Text(piece) => {
                    if piece.is_empty() {
                        return;
                    }
                    let piece = escape_text(piece);
                    let current = self.reply;
                    let entry = shared
                        .entries
                        .iter_mut()
                        .rev()
                        .find_map(|entry| match entry {
                            Entry::Reply { id, text } if Some(*id) == current => Some(text),
                            _ => None,
                        });
                    match entry {
                        Some(text) => {
                            text.push_str(&piece);
                            shared.bound();
                        }
                        None => {
                            let id = shared.id();
                            self.reply = Some(id);
                            shared.push(Entry::Reply { id, text: piece });
                        }
                    }
                    now = self.published.elapsed() >= FRAME;
                }
                Event::ReplyEnd { truncated } => {
                    self.reply = None;
                    if truncated {
                        shared.note("The model's reply was cut at its length limit.", false);
                    }
                }
                Event::Proposed { action, .. } => {
                    let id = shared.id();
                    self.action = Some(id);
                    shared.push(Entry::Action {
                        id,
                        title: action.title(),
                        detail: escape_text(&action.detail()),
                        state: "pending",
                        output: String::new(),
                    });
                }
                Event::Invalid { name, why } => shared.note(
                    shown(&format!(
                        "The model asked for \"{name}\", which was not run: {why}."
                    )),
                    true,
                ),
                Event::Finished {
                    outcome, output, ..
                } => {
                    let current = self.action.take();
                    for entry in &mut shared.entries {
                        if let Entry::Action {
                            id,
                            state,
                            output: shown_output,
                            ..
                        } = entry
                            && Some(*id) == current
                        {
                            *state = match outcome {
                                Outcome::Done => "done",
                                Outcome::Failed => "failed",
                                Outcome::Denied => "denied",
                                Outcome::Stopped => "stopped",
                            };
                            let mut end = output.len().saturating_sub(MAX_OUTPUT_BYTES);
                            while !output.is_char_boundary(end) {
                                end += 1;
                            }
                            let cut = if end > 0 { "…\n" } else { "" };
                            *shown_output = format!("{cut}{}", escape_text(&output[end..]));
                        }
                    }
                    shared.bound();
                }
            }
        }
        if now {
            self.send();
        }
    }

    fn approve(&mut self, ask: &Ask, action: &Action) -> Option<Choice> {
        // Anything still queued answers an earlier request.
        while self.answers.try_recv().is_ok() {}
        lock(&self.shared).ask = Some(AskView {
            id: ask.id.clone(),
            title: action.title(),
            detail: escape_text(&action.detail()),
            remember: ask.remember.map(|remember| match remember {
                Remember::Exact => "exact",
                Remember::Kind => "kind",
            }),
        });
        self.send();
        let answer = loop {
            if self.cancel.load(Ordering::Relaxed) {
                break None;
            }
            match self.answers.recv_timeout(Duration::from_millis(50)) {
                Ok((id, choice)) if id == ask.id => break Some(choice),
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break None,
            }
        };
        lock(&self.shared).ask = None;
        self.send();
        // A Stop that arrived with the answer wins.
        answer.filter(|_| !self.cancel.load(Ordering::Relaxed))
    }
}

impl Agents {
    /// Agents talking to `endpoint`, auditing under the state directory `root`.
    pub fn new(endpoint: Result<Endpoint, NotConfigured>, root: &std::path::Path) -> Self {
        Self(Arc::new(Inner {
            endpoint,
            audit: root.join("agent"),
            live: Mutex::default(),
        }))
    }

    fn live(&self, name: &str) -> Result<Live, Refusal> {
        lock(&self.0.live)
            .get(name)
            .cloned()
            .ok_or_else(|| Refusal::State("the agent is off for this machine".into()))
    }

    pub fn view(&self, name: &str) -> Result<AgentView, Refusal> {
        if !vkored::config::valid_name(name) {
            return Err(Refusal::BadName);
        }
        let live = lock(&self.0.live).get(name).cloned();
        Ok(match live {
            Some(live) => view(&self.0, name, Some(&lock(&live.shared))),
            None => view(&self.0, name, None),
        })
    }

    /// Turn the agent on for machine `name`, acting through `host`.
    pub fn enable(
        &self,
        name: &str,
        host: impl Host + Send + 'static,
        publish: Publish,
    ) -> Result<(), Refusal> {
        let endpoint = self
            .0
            .endpoint
            .clone()
            .map_err(|why| Refusal::State(why.to_string()))?;
        let model = HttpModel::new(endpoint).map_err(|why| Refusal::State(why.to_string()))?;
        self.enable_with(name, model, host, vkore_agent::rule, publish)
    }

    /// [`Agents::enable`] with the model and rule given, as tests need.
    pub fn enable_with(
        &self,
        name: &str,
        model: impl Model + Send + 'static,
        host: impl Host + Send + 'static,
        rule: vkore_agent::gate::Rule,
        publish: Publish,
    ) -> Result<(), Refusal> {
        if !vkored::config::valid_name(name) {
            return Err(Refusal::BadName);
        }
        let mut live = lock(&self.0.live);
        if live.contains_key(name) {
            return Ok(());
        }
        if live.len() >= MAX_AGENTS {
            return Err(Refusal::State(format!(
                "the agent is on for {MAX_AGENTS} machines already; turn one off first"
            )));
        }
        let gate = Gate::open(&self.0.audit, name, rule).map_err(Refusal::State)?;
        let mut session = Session::new(model, host, gate, name);
        let (prompts, queued) = mpsc::sync_channel::<String>(1);
        let (answers, answered) = mpsc::sync_channel(4);
        let cancel = Arc::new(AtomicBool::new(false));
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let mut window = Window {
            inner: self.0.clone(),
            name: name.to_owned(),
            shared: shared.clone(),
            publish: publish.clone(),
            answers: answered,
            cancel: cancel.clone(),
            reply: None,
            action: None,
            published: Instant::now(),
        };
        thread::Builder::new()
            .name(format!("vkore-agent-{name}"))
            .spawn(move || {
                while let Ok(prompt) = queued.recv() {
                    let cancel = window.cancel.clone();
                    let ended = catch_unwind(AssertUnwindSafe(|| {
                        session.prompt(&prompt, &mut window, &cancel)
                    }));
                    match ended {
                        Ok(ended) => window.finished(ended),
                        // The session's state can no longer be trusted: say so and end.
                        Err(_) => {
                            window.finished(Ended::Failed(
                                "the agent stopped unexpectedly; turn it off and on again".into(),
                            ));
                            return;
                        }
                    }
                }
            })
            .map_err(|error| Refusal::State(error.to_string()))?;
        live.insert(
            name.to_owned(),
            Live {
                prompts,
                answers,
                cancel,
                shared,
                publish,
            },
        );
        drop(live);
        self.announce(name);
        Ok(())
    }

    /// Send the machine's current view to the window.
    fn announce(&self, name: &str) {
        let live = lock(&self.0.live).get(name).cloned();
        if let Some(live) = live {
            let snapshot = view(&self.0, name, Some(&lock(&live.shared)));
            (live.publish)(snapshot);
        }
    }

    /// Turn the agent off for machine `name`: stop it, and forget the
    /// conversation and every remembered approval.
    pub fn disable(&self, name: &str) -> Result<(), Refusal> {
        if !vkored::config::valid_name(name) {
            return Err(Refusal::BadName);
        }
        let Some(live) = lock(&self.0.live).remove(name) else {
            return Ok(());
        };
        lock(&live.shared).closed = true;
        live.cancel.store(true, Ordering::Relaxed);
        (live.publish)(view(&self.0, name, None));
        Ok(())
    }

    /// Turn every agent off, as the window closes.
    pub fn shutdown(&self) {
        let names: Vec<String> = lock(&self.0.live).keys().cloned().collect();
        for name in names {
            let _ = self.disable(&name);
        }
    }

    pub fn send(&self, name: &str, text: &str) -> Result<(), Refusal> {
        let text = text.trim();
        if text.is_empty() || text.len() > MAX_PROMPT_BYTES {
            return Err(Refusal::Input("a message holds between 1 byte and 8 KiB"));
        }
        let live = self.live(name)?;
        {
            let mut shared = lock(&live.shared);
            if shared.busy {
                return Err(Refusal::Input("the agent is still working"));
            }
            live.cancel.store(false, Ordering::Relaxed);
            live.prompts
                .try_send(text.to_owned())
                .map_err(|_| Refusal::State("turn the agent off and on again".into()))?;
            shared.busy = true;
            let id = shared.id();
            shared.push(Entry::Prompt {
                id,
                text: escape_text(text),
            });
        }
        self.announce(name);
        Ok(())
    }

    /// A person's answer to the request `ask`: `once`, `session` or `deny`.
    pub fn decide(&self, name: &str, ask: &str, choice: &str) -> Result<(), Refusal> {
        let live = self.live(name)?;
        let choice = match choice {
            "once" => Choice::Once,
            "session" => Choice::Session,
            "deny" => Choice::Deny,
            _ => return Err(Refusal::Input("answer once, session or deny")),
        };
        {
            let mut shared = lock(&live.shared);
            let waiting = shared
                .ask
                .as_ref()
                .filter(|waiting| waiting.id == ask)
                .ok_or(Refusal::Input("that request is no longer waiting"))?;
            if choice == Choice::Session && waiting.remember.is_none() {
                return Err(Refusal::Input("this action can only be allowed once"));
            }
            live.answers
                .try_send((ask.to_owned(), choice))
                .map_err(|_| Refusal::Input("that request is no longer waiting"))?;
            // Answered: a second click finds nothing to answer.
            shared.ask = None;
        }
        self.announce(name);
        Ok(())
    }

    pub fn stop(&self, name: &str) -> Result<(), Refusal> {
        let live = self.live(name)?;
        if lock(&live.shared).busy {
            live.cancel.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// One machine, as the agent acts on it: the window's own runtime and volumes.
pub struct MachineHost {
    pub runtime: Runtime,
    pub volumes: Volumes,
    pub name: String,
    /// Start the machine exactly as the window's Start button does.
    pub start: Box<dyn Fn() -> Result<(), String> + Send>,
}

fn words(error: Refusal) -> String {
    error.to_string()
}

impl MachineHost {
    /// Run `read` on the machine's volume, releasing the disk afterwards unless the
    /// window already had it open.
    fn on_volume<T>(
        &self,
        read: impl FnOnce(&mut crate::volumes::VolumeGuard<'_>) -> Result<T, Refusal>,
    ) -> Result<T, String> {
        let mut volumes = self.volumes.lock();
        let held = volumes.holds(&self.name);
        let result = read(&mut volumes);
        if !held {
            volumes.close_for(&self.name);
        }
        result.map_err(words)
    }

    fn at(path: &FilePath) -> Where {
        Where {
            place: if path.terminal.is_some() {
                "home"
            } else {
                "root"
            }
            .into(),
            face: path.terminal.unwrap_or(0),
            names: path.names.clone(),
        }
    }
}

impl Host for MachineHost {
    fn status(&mut self) -> Result<String, String> {
        let detail = views::machine(&self.runtime, &self.name).map_err(words)?;
        let machine = detail.machine;
        let phase = serde_json::to_value(machine.run.phase)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        let mut out = format!(
            "state: {phase}\nboot stage reached: {}\nCPUs: {}\nmemory: {} MiB\ndisk: {} MiB\nISO: {}\n",
            machine.run.stage.unwrap_or("none"),
            machine.cpus,
            machine.memory_mib,
            machine.disk_mib,
            machine.iso,
        );
        if !machine.run.message.is_empty() {
            let _ = writeln!(out, "last run ended: {}", machine.run.message);
        }
        let skip = detail.lines.len().saturating_sub(STATUS_EVENTS);
        if !detail.lines.is_empty() {
            out.push_str("recent boot events (ms since start):\n");
        }
        for line in detail.lines.into_iter().skip(skip) {
            let _ = writeln!(out, "{} {}", line.at_ms, line.text);
        }
        Ok(out)
    }

    fn console(&mut self) -> Result<Terminal, String> {
        Ok(self
            .runtime
            .screen(&self.name)
            .map_or_else(Terminal::default, |screen| Terminal {
                text: screen.spans.iter().map(|span| span.text.as_str()).collect(),
                connected: screen.connected,
            }))
    }

    fn list_files(&mut self, at: &FilePath) -> Result<String, String> {
        let listing =
            self.on_volume(|volume| volume.list(&self.runtime, &self.name, &Self::at(at)))?;
        Ok(match listing {
            ListingView::EmptyHome => {
                "This terminal has no home yet: the machine creates it when first used.".into()
            }
            ListingView::File { len } => format!("That is a file of {len} bytes."),
            ListingView::Directory { entries, truncated } => {
                if entries.is_empty() {
                    return Ok("The folder is empty.".into());
                }
                let mut out = String::new();
                for entry in entries {
                    if entry.directory {
                        let _ = writeln!(out, "{}/", entry.label);
                    } else {
                        let _ = writeln!(out, "{}  {} bytes", entry.label, entry.len);
                    }
                }
                if truncated {
                    out.push_str("(only the first entries are listed)\n");
                }
                out
            }
        })
    }

    fn read_file(&mut self, at: &FilePath) -> Result<String, String> {
        let preview =
            self.on_volume(|volume| volume.preview(&self.runtime, &self.name, &Self::at(at)))?;
        Ok(match preview {
            PreviewView::Text {
                text,
                truncated,
                len,
                ..
            } => {
                let more = if truncated {
                    ", only its beginning shown"
                } else {
                    ""
                };
                format!("A text file of {len} bytes{more}:\n{text}")
            }
            PreviewView::Binary { len, .. } => {
                format!("A binary file of {len} bytes; its contents are not sent.")
            }
            PreviewView::Image {
                width, height, len, ..
            } => format!("A PNG picture, {width} by {height}, {len} bytes; pictures are not sent."),
        })
    }

    fn start(&mut self) -> Result<(), String> {
        (self.start)()
    }

    fn stop(&mut self) -> Result<(), String> {
        self.runtime.stop(&self.name).map_err(words)
    }

    fn type_console(&mut self, input: &str) -> Result<(), String> {
        self.runtime.console_input(&self.name, input).map_err(words)
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
