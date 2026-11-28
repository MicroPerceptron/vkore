//! Opt-in: a real model behind a real endpoint, operating a pretend machine.
//!
//! Set `VKORE_AGENT_LIVE=1` and the `VKORE_AGENT_*` endpoint variables, then run
//! `cargo test -p vkore-agent --test live -- --nocapture`. Nothing real is
//! touched: the machine is a few canned strings.

use std::sync::atomic::AtomicBool;

use vkore_agent::{
    Action, Approval, Ask, Choice, Ended, Endpoint, Event, FilePath, Gate, Host, HttpModel,
    Outcome, Session, Terminal, Ui,
};

#[derive(Default)]
struct Pretend {
    console: String,
    typed: Vec<String>,
}

impl Host for Pretend {
    fn status(&mut self) -> Result<String, String> {
        Ok("state: ready, uptime 12 s, 4 CPUs, 4096 MiB".into())
    }
    fn console(&mut self) -> Result<Terminal, String> {
        Ok(Terminal {
            text: self.console.clone(),
            connected: true,
        })
    }
    fn list_files(&mut self, _: &FilePath) -> Result<String, String> {
        Err("a running machine owns its disk".into())
    }
    fn read_file(&mut self, _: &FilePath) -> Result<String, String> {
        Err("a running machine owns its disk".into())
    }
    fn start(&mut self) -> Result<(), String> {
        Err("that machine is already running".into())
    }
    fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn type_console(&mut self, input: &str) -> Result<(), String> {
        self.typed.push(input.into());
        let command = input.trim();
        self.console.push_str(&format!("{command}\n"));
        self.console.push_str(match command {
            "help" => "builtins: help, source, version\n",
            "version" => "kore 0.40.0 (pretend)\n",
            _ => "unknown command; try help\n",
        });
        self.console.push_str("terminal:0> ");
        Ok(())
    }
}

#[derive(Default)]
struct Approver {
    proposed: Vec<String>,
    outcomes: Vec<Outcome>,
}

impl Ui for Approver {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Text(piece) => print!("{piece}"),
            Event::ReplyEnd { .. } => println!(),
            Event::Proposed { action, .. } => {
                println!("[proposes {} {}]", action.operation(), action.detail());
                self.proposed.push(action.operation().into());
            }
            Event::Invalid { name, why } => println!("[invalid {name}: {why}]"),
            Event::Finished {
                outcome, output, ..
            } => {
                println!("[{outcome:?}]\n{output}");
                self.outcomes.push(outcome);
            }
        }
    }
    fn approve(&mut self, _: &Ask, _: &Action) -> Option<Choice> {
        Some(Choice::Once)
    }
}

#[test]
fn a_real_model_operates_a_pretend_machine() {
    if std::env::var("VKORE_AGENT_LIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set VKORE_AGENT_LIVE=1 to run against a real endpoint");
        return;
    }
    let endpoint = Endpoint::from_env().expect("the endpoint variables");
    println!("model {} at {}", endpoint.model(), endpoint.host());
    let directory = tempfile::tempdir().unwrap();
    let gate = Gate::open(directory.path(), "pretend", |action| {
        if action.changes_machine() {
            Approval::Ask
        } else {
            Approval::Free
        }
    })
    .unwrap();
    let mut session = Session::new(
        HttpModel::new(endpoint).unwrap(),
        Pretend {
            console: "terminal:0> ".into(),
            typed: Vec::new(),
        },
        gate,
        "pretend",
    );
    let mut ui = Approver::default();
    let ended = session.prompt(
        "Run the command `version` in the console and tell me which version it prints.",
        &mut ui,
        &AtomicBool::new(false),
    );
    assert_eq!(ended, Ended::Finished);
    assert_eq!(session.host().typed, ["version\r"]);
    // A second prompt proves the server accepts the history with its tool turns.
    let ended = session.prompt(
        "Which version was that again? Answer without tools.",
        &mut ui,
        &AtomicBool::new(false),
    );
    assert_eq!(ended, Ended::Finished);
    let answer = &session.history().last().unwrap().content;
    assert!(answer.contains("0.40.0"), "{answer}");
}
