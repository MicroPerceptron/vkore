use super::*;
use vkore_agent::client::Reply;
use vkore_agent::protocol::{Message, Tool, ToolCall};
use vkore_agent::{Approval, ModelError};

/// Answers each request with the next scripted reply.
struct Script(VecDeque<Reply>);

impl Model for Script {
    fn complete(
        &mut self,
        _: &[Message],
        _: &[Tool],
        text: &mut dyn FnMut(&str),
        _: &AtomicBool,
    ) -> Result<Reply, ModelError> {
        let reply = self.0.pop_front().unwrap_or_default();
        text(&reply.text);
        Ok(reply)
    }
}

#[derive(Clone, Default)]
struct Stub(Arc<Mutex<usize>>);

impl Host for Stub {
    fn status(&mut self) -> Result<String, String> {
        Ok("ready".into())
    }
    fn console(&mut self) -> Result<Terminal, String> {
        Ok(Terminal::default())
    }
    fn list_files(&mut self, _: &FilePath) -> Result<String, String> {
        Ok(String::new())
    }
    fn read_file(&mut self, _: &FilePath) -> Result<String, String> {
        Ok(String::new())
    }
    fn start(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), String> {
        *lock(&self.0) += 1;
        Ok(())
    }
    fn type_console(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

fn ask_each_change(action: &Action) -> Approval {
    if action.changes_machine() {
        Approval::Ask
    } else {
        Approval::Free
    }
}

fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vkore-tauri-agent-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

struct Rig {
    agents: Agents,
    host: Stub,
    published: Arc<Mutex<Vec<AgentView>>>,
    dir: std::path::PathBuf,
}

impl Rig {
    fn new(tag: &str, replies: Vec<Reply>) -> Self {
        let dir = scratch(tag);
        let endpoint = Endpoint::new("http://127.0.0.1:9/v1", "scripted", None);
        let agents = Agents::new(endpoint, &dir);
        let host = Stub::default();
        let published: Arc<Mutex<Vec<AgentView>>> = Arc::default();
        let seen = published.clone();
        agents
            .enable_with(
                "dev",
                Script(replies.into()),
                host.clone(),
                ask_each_change,
                Arc::new(move |view| lock(&seen).push(view)),
            )
            .unwrap();
        Rig {
            agents,
            host,
            published,
            dir,
        }
    }

    fn until(&self, ready: impl Fn(&AgentView) -> bool) -> AgentView {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let view = self.agents.view("dev").unwrap();
            if ready(&view) {
                return view;
            }
            assert!(Instant::now() < deadline, "never got there: {view:?}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stops(&self) -> usize {
        *lock(&self.host.0)
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.agents.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stop_call(text: &str) -> Reply {
    Reply {
        text: text.into(),
        calls: vec![ToolCall::new("c", "stop_machine", "{}")],
        truncated: false,
    }
}

fn say(text: &str) -> Reply {
    Reply {
        text: text.into(),
        ..Reply::default()
    }
}

#[test]
fn an_action_waits_for_the_answer_that_names_it() {
    let rig = Rig::new(
        "answer",
        vec![stop_call("I will stop it."), say("Stopped.")],
    );
    rig.agents.send("dev", "stop it").unwrap();
    let view = rig.until(|view| view.ask.is_some());
    let ask = view.ask.clone().unwrap();
    assert!(view.busy);
    assert_eq!(ask.title, "Stop the machine");
    assert_eq!(rig.stops(), 0);
    crate::assert_camel_keys(&serde_json::to_value(&view).unwrap());

    // The wrong id, an unknown answer and an unoffered "session" change nothing.
    assert!(rig.agents.decide("dev", "other", "once").is_err());
    assert!(rig.agents.decide("dev", &ask.id, "yes").is_err());
    assert!(rig.agents.decide("dev", &ask.id, "session").is_err());
    assert!(rig.agents.send("dev", "again").is_err());
    assert_eq!(rig.stops(), 0);

    rig.agents.decide("dev", &ask.id, "once").unwrap();
    assert!(rig.agents.decide("dev", &ask.id, "once").is_err());
    let view = rig.until(|view| !view.busy);
    assert_eq!(rig.stops(), 1);
    let states: Vec<&str> = view
        .entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Action { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(states, ["done"]);
    assert!(matches!(view.entries.last(), Some(Entry::Reply { text, .. }) if text == "Stopped."));
    assert!(!lock(&rig.published).is_empty());
}

#[test]
fn stop_and_turning_off_run_nothing() {
    let rig = Rig::new("stop", vec![stop_call(""), stop_call("")]);
    rig.agents.send("dev", "stop it").unwrap();
    rig.until(|view| view.ask.is_some());
    rig.agents.stop("dev").unwrap();
    let view = rig.until(|view| !view.busy);
    assert!(view.ask.is_none());
    assert_eq!(rig.stops(), 0);

    rig.agents.send("dev", "try again").unwrap();
    let ask = rig.until(|view| view.ask.is_some()).ask.unwrap();
    rig.agents.disable("dev").unwrap();
    assert!(rig.agents.decide("dev", &ask.id, "once").is_err());
    let view = rig.agents.view("dev").unwrap();
    assert!(!view.enabled && view.entries.is_empty());
    thread::sleep(Duration::from_millis(150));
    assert_eq!(rig.stops(), 0);
    // Nothing the old session does afterwards reaches the window.
    assert!(!lock(&rig.published).last().unwrap().enabled);
}

#[test]
fn model_text_is_escaped_and_the_transcript_is_bounded() {
    let long = "a\u{202e}b ".repeat(4000);
    let replies = (0..12).map(|_| say(&long)).collect();
    let rig = Rig::new("bounded", replies);
    for turn in 0..12 {
        rig.agents.send("dev", &format!("turn {turn}")).unwrap();
        rig.until(|view| !view.busy);
    }
    let view = rig.agents.view("dev").unwrap();
    let weight: usize = view.entries.iter().map(Entry::weight).sum();
    assert!(weight <= MAX_VIEW_BYTES && view.entries.len() <= MAX_ENTRIES);
    let wire = serde_json::to_string(&view).unwrap();
    assert!(!wire.contains('\u{202e}') && wire.contains("u{202E}"));
}

#[test]
fn without_an_endpoint_the_window_is_told_why_and_nothing_starts() {
    let dir = scratch("unset");
    let agents = Agents::new(Endpoint::from_vars(|_| None), &dir);
    let view = agents.view("dev").unwrap();
    assert!(!view.configured && view.problem.contains("VKORE_AGENT_BASE_URL"));
    let refused = agents.enable("dev", Stub::default(), Arc::new(|_| {}));
    assert!(refused.is_err());
    assert!(agents.view("../x").is_err());
    assert!(!dir.join("agent").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_host_reads_a_stored_machine_and_refuses_what_a_stopped_one_cannot_do() {
    let dir = scratch("host");
    let runtime = Runtime::new(dir.join("state"), None);
    std::fs::write(dir.join("kore.iso"), b"iso").unwrap();
    let config = vkored::MachineConfig::new("dev", dir.join("kore.iso"));
    runtime.store().unwrap().create(&config).unwrap();
    let mut host = MachineHost {
        runtime,
        volumes: Volumes::default(),
        name: "dev".into(),
        start: Box::new(|| Err("no QEMU here".into())),
    };
    let status = host.status().unwrap();
    assert!(
        status.contains("state: idle") && status.contains("ISO: kore.iso"),
        "{status}"
    );
    assert_eq!(host.console().unwrap(), Terminal::default());
    assert!(host.stop().unwrap_err().contains("not running"));
    assert!(
        host.type_console("ls\r")
            .unwrap_err()
            .contains("no console")
    );
    assert_eq!(host.start().unwrap_err(), "no QEMU here");
    std::fs::remove_dir_all(&dir).unwrap();
}
