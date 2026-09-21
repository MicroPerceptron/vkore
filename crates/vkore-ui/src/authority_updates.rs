//! A permission update is an explicit action on the current prompt. Retain
//! the exact request across uncertain HTTP delivery instead of inventing a
//! new operation on retry or interpreting the composer as a new chat turn.
use super::*;

const UPDATE_TEXT_MAX: usize = 128; // Current authenticated broker intent bound.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UpdateState {
    Sending,
    Awaiting,
    Uncertain,
    Applied,
    Refused,
    Failed,
    Cancelled,
}

impl UpdateState {
    fn terminal(self) -> bool {
        matches!(
            self,
            Self::Applied | Self::Refused | Self::Failed | Self::Cancelled
        )
    }

    fn decode(value: &Value) -> Option<Self> {
        Some(match value.as_str()? {
            "queued" | "dispatched" => Self::Awaiting,
            "applied" => Self::Applied,
            "refused" => Self::Refused,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    fn from_record(value: &Value) -> Option<Self> {
        let state = Self::decode(&value["status"])?;
        if state == Self::Applied
            && value["revision"]
                .as_u64()
                .is_none_or(|revision| revision == 0 || revision > u32::MAX as u64)
        {
            return None;
        }
        Some(state)
    }
}

#[derive(Clone)]
pub(super) struct Submission {
    token: u64,
    session: String,
    prompt: String,
    id: String,
    text: String,
    draft: String,
    state: UpdateState,
    message: String,
}

impl Submission {
    fn body(&self) -> Value {
        json!({"api_version": 1, "prompt_id": self.prompt, "update_id": self.id, "text": self.text})
    }

    fn accept(&mut self, value: &Value) -> bool {
        let Some(state) = UpdateState::from_record(value) else {
            return false;
        };
        // The event stream may deliver final testimony before the POST reply.
        if !self.state.terminal() {
            self.state = state;
            self.message = value["message"]
                .as_str()
                .filter(|text| !text.is_empty())
                .unwrap_or(match state {
                    UpdateState::Applied => "Permissions updated.",
                    UpdateState::Refused => "The permission update was refused.",
                    UpdateState::Failed => "The permission update could not be confirmed.",
                    UpdateState::Cancelled => "Permission update cancelled.",
                    _ => "Permission update queued. Waiting for the broker.",
                })
                .to_owned();
        }
        true
    }
}

impl Model {
    fn current_permission_update(&self) -> Option<&Submission> {
        self.permission_update.as_ref().filter(|update| {
            self.session.as_deref() == Some(update.session.as_str())
                && self.prompt_id.as_deref() == Some(update.prompt.as_str())
        })
    }

    pub(super) fn permission_update_message(&self) -> Option<&str> {
        self.current_permission_update()
            .map(|update| update.message.as_str())
    }

    pub(super) fn permission_update_label(&self) -> &'static str {
        if self.current_permission_update().is_some_and(|update| {
            self.prompt_id.as_deref() == Some(update.prompt.as_str())
                && update.state == UpdateState::Uncertain
        }) {
            "Retry update"
        } else {
            "Update permissions"
        }
    }

    pub(super) fn can_update_permissions(&self) -> bool {
        let (Some(session), Some(prompt)) = (&self.session, &self.prompt_id) else {
            return false;
        };
        if self.events.iter().any(|event| {
            event["session_id"].as_str() == Some(session)
                && event["prompt_id"].as_str() == Some(prompt)
                && (event["kind"] == "prompt_terminal"
                    || (event["public"] == true
                        && matches!(event["kind"].as_str(), Some("assistant" | "error"))))
        }) {
            return false;
        }
        // Another authenticated client can update the same prompt. Derive
        // pending state from the newest event for each update ID.
        let mut seen = BTreeSet::new();
        for event in self.events.iter().rev().filter(|event| {
            event["kind"] == "authority_update"
                && event["session_id"].as_str() == Some(session)
                && event["prompt_id"].as_str() == Some(prompt)
        }) {
            let payload = &event["payload"];
            if let Some(id) = payload["update_id"].as_str()
                && seen.insert(id)
                && UpdateState::from_record(payload) == Some(UpdateState::Awaiting)
            {
                return false;
            }
        }
        if let Some(update) = self
            .current_permission_update()
            .filter(|update| update.prompt == *prompt)
        {
            if !update.state.terminal() {
                return update.state == UpdateState::Uncertain;
            }
        }
        !self.prompt.text().trim().is_empty()
    }

    pub(super) fn send_permission_update(&mut self) {
        if !self.can_update_permissions() {
            return;
        }
        let session = self.session.clone().unwrap();
        let prompt = self.prompt_id.clone().unwrap();
        let retry = self.permission_update.as_ref().is_some_and(|update| {
            update.session == session
                && update.prompt == prompt
                && update.state == UpdateState::Uncertain
        });
        if !retry {
            let draft = self.prompt.text();
            let text = draft.trim();
            if text.len() > UPDATE_TEXT_MAX || text.contains('\0') {
                self.status =
                    "Keep permission updates within 128 UTF-8 bytes and omit NUL characters."
                        .into();
                return;
            }
            let Some(token) = self.next_permission_update.checked_add(1) else {
                self.status = "Permission update identifiers exhausted. Restart the UI.".into();
                return;
            };
            self.next_permission_update = token;
            self.permission_update = Some(Submission {
                token,
                session,
                prompt,
                id: format!("permission-{}-{token}", now_id()),
                text: text.to_owned(),
                draft,
                state: UpdateState::Sending,
                message: "Submitting permission update…".into(),
            });
        }
        let update = self.permission_update.as_mut().unwrap();
        update.state = UpdateState::Sending;
        update.message = "Submitting permission update…".into();
        self.network.request(Request {
            kind: RequestKind::PermissionUpdate(update.token),
            method: "POST",
            path: format!(
                "/api/v1/sessions/{}/prompts/{}/updates",
                update.session, update.prompt
            ),
            body: Some(update.body()),
        });
    }

    pub(super) fn permission_update_response(
        &mut self,
        token: u64,
        response: Result<Value, String>,
    ) {
        // A permission POST can finish after the user switches projects.
        // Keep its exact request/retry state with that chat, never the new one.
        if !self
            .permission_update
            .as_ref()
            .is_some_and(|update| update.token == token)
        {
            for chat in self.project_chats.values_mut() {
                let Some(update) = chat
                    .permission_update
                    .as_mut()
                    .filter(|update| update.token == token)
                else {
                    continue;
                };
                let accepted = match &response {
                    Ok(value) if value["request"] == update.body() => update.accept(value),
                    _ => false,
                };
                if !accepted && !update.state.terminal() {
                    update.state = UpdateState::Uncertain;
                    update.message =
                        "Could not confirm the update. Retry sends the same permission request."
                            .into();
                }
                if accepted
                    && matches!(update.state, UpdateState::Awaiting | UpdateState::Applied)
                    && chat.prompt_id.as_deref() == Some(update.prompt.as_str())
                    && chat.draft == update.draft
                {
                    chat.draft.clear();
                }
                return;
            }
            return;
        }
        let update = self.permission_update.as_mut().unwrap();
        let accepted = match &response {
            Ok(value) if value["request"] == update.body() => update.accept(value),
            _ => false,
        };
        if !accepted && !update.state.terminal() {
            update.state = UpdateState::Uncertain;
            update.message =
                "Could not confirm the update. Retry sends the same permission request.".into();
        }
        if accepted
            && matches!(update.state, UpdateState::Awaiting | UpdateState::Applied)
            && self.session.as_deref() == Some(update.session.as_str())
            && self.prompt_id.as_deref() == Some(update.prompt.as_str())
            && self.prompt.text() == update.draft
        {
            self.prompt = text_editor::Content::new();
        }
    }

    pub(super) fn permission_update_event(&mut self, event: &Value) {
        let Some(update) = self.permission_update.as_mut() else {
            return;
        };
        if event["kind"] == "authority_update"
            && event["session_id"].as_str() == Some(update.session.as_str())
            && event["prompt_id"].as_str() == Some(update.prompt.as_str())
            && event["payload"]["update_id"].as_str() == Some(update.id.as_str())
        {
            update.accept(&event["payload"]);
        }
    }
}

pub(super) fn work_row(event: &Value) -> Option<WorkRow> {
    let payload = &event["payload"];
    let state = UpdateState::from_record(payload)?;
    let body = if state == UpdateState::Awaiting {
        payload["text"]
            .as_str()
            .or_else(|| payload["message"].as_str())?
    } else {
        payload["message"].as_str()?
    };
    Some(WorkRow {
        speaker: "Permissions".into(),
        context: Some(
            match state {
                UpdateState::Applied => "Update applied",
                UpdateState::Refused => "Update refused",
                UpdateState::Failed => "Update unconfirmed",
                UpdateState::Cancelled => "Update cancelled",
                _ => "Update queued",
            }
            .into(),
        ),
        body: body.to_owned(),
        kind: if matches!(state, UpdateState::Refused | UpdateState::Failed) {
            "error"
        } else {
            "decision"
        }
        .into(),
        output: None,
    })
}
