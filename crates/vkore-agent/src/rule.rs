//! Which actions a person must approve, and what they may let the agent repeat.
//!
//! This is the policy of the agent, kept in one small function. The [`gate`]
//! holds one floor under it whatever it says: an action that changes the machine
//! is never run without a person having approved it at least once.
//!
//! [`gate`]: crate::gate

use crate::tools::Action;

/// What a person must do before an action runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    /// Run without asking. For an action that changes the machine this is
    /// raised to [`Approval::Ask`].
    Free,
    /// Ask every time.
    Ask,
    /// Ask, and offer "allow for this session": the very same action (for typing,
    /// the same text and the same Enter) then runs without asking.
    AskRememberExact,
    /// Ask, and offer "allow for this session": every later action of this kind
    /// then runs without asking, whatever its arguments.
    AskRememberKind,
}

/// What the person must do before the agent performs `action`.
///
/// A remembered approval lasts until the agent is turned off for the machine,
/// and applies to that machine only.
pub fn rule(action: &Action) -> Approval {
    // TODO(you): decide which of the actions that change a machine may be
    // remembered, and how widely. Until then each one asks every time.
    //
    //   Action::StartMachine             boots the machine; its disk changes
    //   Action::StopMachine              ends whatever the guest was doing
    //   Action::TypeConsole { text, .. } runs a command in the guest's shell
    //
    // The reading actions below change nothing, but what they read is sent to
    // the model's endpoint.
    match action {
        Action::MachineStatus
        | Action::ReadConsole
        | Action::ListFiles(_)
        | Action::ReadFile(_) => Approval::Free,
        Action::StartMachine | Action::StopMachine | Action::TypeConsole { .. } => Approval::Ask,
    }
}
