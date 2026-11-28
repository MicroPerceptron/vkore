//! An operator agent for one machine.
//!
//! The agent is a client of the OpenAI chat protocol: any endpoint that serves
//! streamed `/chat/completions` with function tools can drive it, a hosted model
//! now and a model served by Kore later. It acts only through [`Host`], which a
//! front end implements for one machine, and every action passes the [`gate`]:
//! reading is described to the person who enables the agent, and anything that
//! changes the machine is approved by them, recorded by `vkore-policy`.
//!
//! The rules it keeps:
//! - **What a guest or a model wrote is data.** Console text and file contents
//!   go to the model as tool results; nothing in them can approve anything,
//!   because approval is a person's click and never text.
//! - **What was approved is what runs.** Typed input is printable ASCII only, so
//!   the card shows exactly the bytes that are sent.
//! - **Everything is bounded:** streams, lines, replies, tool results, steps per
//!   prompt and the remembered conversation.
//! - **Nothing is replayed.** A failed request is reported, not retried.

pub mod client;
pub mod config;
pub mod gate;
pub mod protocol;
pub mod rule;
pub mod session;
#[cfg(test)]
mod testing;
pub mod tools;

pub use client::{HttpModel, Model, ModelError};
pub use config::{Endpoint, NotConfigured};
pub use gate::{Ask, Choice, Gate, Remember};
pub use rule::{Approval, rule};
pub use session::{Ended, Event, Outcome, Session, Ui};
pub use tools::{Action, FilePath, Host, Terminal};
