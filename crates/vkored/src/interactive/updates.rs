//! Authenticated per-prompt update custody. HTTP enqueue and runtime delivery
//! share the session owner; a retry cannot become a second guest operation.
use super::*;

const UPDATES_MAX: usize = 32;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorityUpdateRequest {
    pub api_version: u16,
    pub prompt_id: String,
    pub update_id: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityUpdateStatus {
    Queued,
    Dispatched,
    Applied,
    Refused,
    Failed,
    Cancelled,
}

impl AuthorityUpdateStatus {
    pub fn terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Dispatched)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorityUpdateState {
    pub request: AuthorityUpdateRequest,
    pub status: AuthorityUpdateStatus,
    pub revision: Option<u32>,
    pub message: String,
}

impl HarnessHandle {
    pub fn submit_authority_update(
        &self,
        session_id: &str,
        request: AuthorityUpdateRequest,
    ) -> Result<AuthorityUpdateState, HarnessError> {
        if request.api_version != HARNESS_API_VERSION {
            return Err(HarnessError::UnsupportedVersion);
        }
        if !valid_id(&request.prompt_id) || !valid_id(&request.update_id) {
            return Err(HarnessError::InvalidIdentity);
        }
        if request.text.is_empty()
            || request.text.len() > PROMPT_BYTES_MAX
            || request.text.contains('\0')
        {
            return Err(HarnessError::PromptTooLarge);
        }
        let mut shared = self
            .shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?;
        if !shared.authority_updates_supported {
            return Err(HarnessError::Runtime(
                "This runtime does not support authority updates.".into(),
            ));
        }
        let data = shared
            .sessions
            .get(session_id)
            .ok_or(HarnessError::UnknownSession)?;
        if let Some(existing) = data.updates.get(&request.update_id) {
            return if existing.request == request {
                Ok(existing.clone())
            } else {
                Err(HarnessError::IdempotencyConflict)
            };
        }
        if matches!(
            data.state.status,
            SessionStatus::Closing | SessionStatus::Closed
        ) {
            return Err(HarnessError::SessionClosed);
        }
        match data.state.prompts.get(&request.prompt_id) {
            None => return Err(HarnessError::UnknownPrompt),
            Some(PromptStatus::Running) => {}
            _ => return Err(HarnessError::Busy),
        }
        if data.published_prompt.as_deref() == Some(request.prompt_id.as_str())
            || data.cancellations.contains(&request.prompt_id)
            || data.updates.values().any(|update| {
                update.request.prompt_id == request.prompt_id && !update.status.terminal()
            })
        {
            return Err(HarnessError::Busy);
        }
        if data.updates.len() == UPDATES_MAX {
            return Err(HarnessError::Backpressure);
        }
        let state = AuthorityUpdateState {
            request,
            status: AuthorityUpdateStatus::Queued,
            revision: None,
            message: "Authority update queued.".into(),
        };
        shared.emit(
            session_id,
            Some(&state.request.prompt_id),
            EventKind::AuthorityUpdate,
            true,
            json!({"update_id": state.request.update_id, "status": state.status,
                "text": state.request.text, "message": state.message,
                "provenance": "frontend_authority_update"}),
        )?;
        shared
            .sessions
            .get_mut(session_id)
            .unwrap()
            .updates
            .insert(state.request.update_id.clone(), state.clone());
        Ok(state)
    }

    pub fn authority_update(
        &self,
        session_id: &str,
        prompt_id: &str,
        update_id: &str,
    ) -> Result<AuthorityUpdateState, HarnessError> {
        let shared = self
            .shared
            .lock()
            .map_err(|_| HarnessError::Runtime("state poisoned".into()))?;
        shared
            .sessions
            .get(session_id)
            .ok_or(HarnessError::UnknownSession)?
            .updates
            .get(update_id)
            .filter(|update| update.request.prompt_id == prompt_id)
            .cloned()
            .ok_or(HarnessError::UnknownPrompt)
    }
}

impl Shared {
    pub(super) fn take_authority_update(
        &mut self,
        session_id: &str,
        prompt_id: &str,
    ) -> Option<AuthorityUpdateRequest> {
        let data = self.sessions.get_mut(session_id)?;
        if data.cancellations.contains(prompt_id)
            || data.state.status != SessionStatus::Busy
            || data.state.prompts.get(prompt_id) != Some(&PromptStatus::Running)
        {
            return None;
        }
        let update = data.updates.values_mut().find(|update| {
            update.request.prompt_id == prompt_id && update.status == AuthorityUpdateStatus::Queued
        })?;
        update.status = AuthorityUpdateStatus::Dispatched;
        Some(update.request.clone())
    }

    pub(super) fn finish_authority_update(
        &mut self,
        session_id: &str,
        prompt_id: &str,
        payload: Value,
    ) -> Result<(), HarnessError> {
        let id = payload["update_id"]
            .as_str()
            .ok_or(HarnessError::InvalidIdentity)?
            .to_owned();
        let status: AuthorityUpdateStatus = serde_json::from_value(payload["status"].clone())
            .map_err(|_| HarnessError::InvalidIdentity)?;
        let current = self
            .sessions
            .get(session_id)
            .and_then(|data| data.updates.get(&id))
            .filter(|update| update.request.prompt_id == prompt_id)
            .ok_or(HarnessError::UnknownPrompt)?;
        if !status.terminal()
            || current.status.terminal()
            || (current.status == AuthorityUpdateStatus::Queued
                && status != AuthorityUpdateStatus::Cancelled)
        {
            return Err(HarnessError::InvalidIdentity);
        }
        let message = payload["message"].as_str().unwrap_or_default().to_owned();
        let revision = payload["revision"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok());
        self.emit(
            session_id,
            Some(prompt_id),
            EventKind::AuthorityUpdate,
            true,
            payload,
        )?;
        let update = self
            .sessions
            .get_mut(session_id)
            .unwrap()
            .updates
            .get_mut(&id)
            .unwrap();
        update.status = status;
        update.message = message;
        update.revision = revision;
        Ok(())
    }

    pub(super) fn finish_pending_updates(&mut self, session_id: &str, prompt_id: &str) {
        let pending: Vec<_> = self
            .sessions
            .get(session_id)
            .into_iter()
            .flat_map(|data| data.updates.values())
            .filter(|update| update.request.prompt_id == prompt_id && !update.status.terminal())
            .map(|update| (update.request.update_id.clone(), update.status))
            .collect();
        for (id, status) in pending {
            let (status, message) = if status == AuthorityUpdateStatus::Queued {
                (
                    AuthorityUpdateStatus::Cancelled,
                    "The prompt ended before this update was dispatched.",
                )
            } else {
                (
                    AuthorityUpdateStatus::Failed,
                    "The prompt ended without a confirmed authority update outcome.",
                )
            };
            let _ = self.finish_authority_update(
                session_id,
                prompt_id,
                json!({"update_id": id, "status": status, "message": message}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, HarnessHandle) {
        let temp = tempfile::tempdir().unwrap();
        let (commands, _) = mpsc::sync_channel(1);
        let handle = HarnessHandle {
            commands,
            shared: Arc::new(Mutex::new(Shared {
                authority_updates_supported: true,
                sessions: BTreeMap::new(),
                next_sequence: 1,
                queued: 0,
                transcript_root: temp.path().to_owned(),
            })),
        };
        handle
            .create_session(CreateSession {
                api_version: 1,
                project_id: "project".into(),
                session_id: "session".into(),
            })
            .unwrap();
        {
            let mut shared = handle.shared.lock().unwrap();
            let data = shared.sessions.get_mut("session").unwrap();
            data.state.status = SessionStatus::Busy;
            data.state
                .prompts
                .insert("prompt".into(), PromptStatus::Running);
        }
        (temp, handle)
    }

    fn request(id: &str) -> AuthorityUpdateRequest {
        AuthorityUpdateRequest {
            api_version: 1,
            prompt_id: "prompt".into(),
            update_id: id.into(),
            text: "Revoke input.".into(),
        }
    }

    #[test]
    fn authority_updates_are_bounded_idempotent_and_bound_to_the_active_prompt() {
        let (_temp, handle) = fixture();
        assert_eq!(
            handle.submit_authority_update("missing", request("u")),
            Err(HarnessError::UnknownSession)
        );
        assert_eq!(
            handle.submit_authority_update(
                "session",
                AuthorityUpdateRequest {
                    prompt_id: "foreign".into(),
                    ..request("u")
                }
            ),
            Err(HarnessError::UnknownPrompt)
        );
        for text in [
            String::new(),
            "bad\0text".into(),
            "x".repeat(PROMPT_BYTES_MAX + 1),
            "x".repeat(kore_hostlink::FRONTEND_TEXT_MAX + 1),
        ] {
            assert_eq!(
                handle.submit_authority_update(
                    "session",
                    AuthorityUpdateRequest {
                        text,
                        ..request("u")
                    }
                ),
                Err(HarnessError::PromptTooLarge)
            );
        }
        for index in 0..UPDATES_MAX {
            let mut request = request(&format!("u{index}"));
            if index == 0 {
                request.text = "x".repeat(PROMPT_BYTES_MAX);
            }
            let state = handle
                .submit_authority_update("session", request.clone())
                .unwrap();
            assert_eq!(
                handle
                    .submit_authority_update("session", request.clone())
                    .unwrap(),
                state
            );
            assert_eq!(
                handle.submit_authority_update(
                    "session",
                    AuthorityUpdateRequest {
                        text: "Grant input.".into(),
                        ..request.clone()
                    }
                ),
                Err(HarnessError::IdempotencyConflict)
            );
            assert_eq!(
                handle.submit_authority_update(
                    "session",
                    AuthorityUpdateRequest {
                        update_id: "another".into(),
                        ..request.clone()
                    }
                ),
                Err(HarnessError::Busy)
            );
            let mut shared = handle.shared.lock().unwrap();
            assert!(shared.take_authority_update("session", "foreign").is_none());
            assert_eq!(
                shared.take_authority_update("session", "prompt"),
                Some(request.clone())
            );
            assert!(shared.take_authority_update("session", "prompt").is_none());
            shared.finish_authority_update("session", "prompt", json!({
                "update_id": request.update_id, "status": "refused", "message": "no authority"
            })).unwrap();
        }
        assert_eq!(
            handle.submit_authority_update("session", request("overflow")),
            Err(HarnessError::Backpressure)
        );
        let events = handle.events("session", 0).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::AuthorityUpdate)
                .count(),
            UPDATES_MAX * 2
        );
        assert!(!events.iter().any(|event| matches!(
            event.kind,
            EventKind::Assistant | EventKind::Error | EventKind::PromptTerminal
        )));
    }

    #[test]
    fn authority_updates_end_truthfully_on_cancellation_or_unknown_dispatch() {
        for dispatched in [false, true] {
            let (_temp, handle) = fixture();
            handle
                .submit_authority_update("session", request("u"))
                .unwrap();
            {
                let mut shared = handle.shared.lock().unwrap();
                if dispatched {
                    assert!(shared.take_authority_update("session", "prompt").is_some());
                }
                shared
                    .sessions
                    .get_mut("session")
                    .unwrap()
                    .cancellations
                    .insert("prompt".into());
                assert!(shared.take_authority_update("session", "prompt").is_none());
                shared.finish_pending_updates("session", "prompt");
                shared.finish_pending_updates("session", "prompt");
            }
            let result = handle.authority_update("session", "prompt", "u").unwrap();
            assert_eq!(
                result.status,
                if dispatched {
                    AuthorityUpdateStatus::Failed
                } else {
                    AuthorityUpdateStatus::Cancelled
                }
            );
            assert_eq!(
                handle
                    .submit_authority_update("session", request("u"))
                    .unwrap(),
                result
            );
            assert_eq!(
                handle.submit_authority_update("session", request("new")),
                Err(HarnessError::Busy)
            );
            assert!(handle.authority_update("session", "foreign", "u").is_err());
            assert_eq!(
                handle
                    .events("session", 0)
                    .unwrap()
                    .iter()
                    .filter(|event| event.kind == EventKind::AuthorityUpdate)
                    .count(),
                2
            );
        }
        let (_temp, handle) = fixture();
        handle
            .shared
            .lock()
            .unwrap()
            .sessions
            .get_mut("session")
            .unwrap()
            .published_prompt = Some("prompt".into());
        assert_eq!(
            handle.submit_authority_update("session", request("late")),
            Err(HarnessError::Busy)
        );
    }
}
