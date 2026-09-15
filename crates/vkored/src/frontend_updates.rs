//! Bounded host custody of authenticated updates, separate from prompt answers.
use crate::{backend::SequenceIdentity, daemon::FrontendQueueError};
use kore_hostlink::{
    FRONTEND_TEXT_MAX, FrontendIntentId, FrontendUpdate, FrontendUpdateOutcome,
    FrontendUpdateResult, RequestId,
};
use std::collections::VecDeque;

const MAX: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontendUpdateResultOwned {
    pub intent_id: FrontendIntentId,
    pub revision: u32,
    pub outcome: FrontendUpdateOutcome,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::BackendRole;
    use kore_hostlink::{RouteId, TaskGeneration};

    fn broker() -> SequenceIdentity {
        SequenceIdentity {
            route: RouteId([1; 16]),
            generation: TaskGeneration::new(3).unwrap(),
            role: BackendRole::Broker,
        }
    }

    #[test]
    fn frontend_updates_reject_foreign_stale_and_undelivered_results() {
        let mut updates = FrontendUpdates::default();
        let id = FrontendIntentId([2; 16]);
        let broker = broker();
        let revision = updates
            .queue(id, broker, "Revoke input.".to_owned())
            .unwrap();
        let result = FrontendUpdateResult {
            intent_id: id,
            revision,
            outcome: FrontendUpdateOutcome::Applied,
            text: "done",
        };
        assert!(!updates.accepts(broker, result));
        updates.delivered(id, revision);
        assert!(!updates.accepts(
            SequenceIdentity {
                generation: TaskGeneration::new(4).unwrap(),
                ..broker
            },
            result
        ));
        assert!(!updates.accepts(
            broker,
            FrontendUpdateResult {
                revision: revision + 1,
                ..result
            }
        ));
        assert!(!updates.accepts(
            broker,
            FrontendUpdateResult {
                intent_id: FrontendIntentId([3; 16]),
                ..result
            }
        ));
        assert!(updates.accepts(broker, result));
        updates.close_broker(SequenceIdentity {
            role: BackendRole::Agent,
            ..broker
        });
        updates.close_broker(SequenceIdentity {
            generation: TaskGeneration::new(4).unwrap(),
            ..broker
        });
        assert!(updates.pop().is_none());
        assert!(updates.requires_progress());
        updates.close_broker(broker);
        let result = updates.pop().unwrap();
        assert_eq!(result.outcome, FrontendUpdateOutcome::Failed);
        assert!(result.text.contains("unknown"));
        assert!(!updates.requires_progress());
    }

    #[test]
    fn frontend_updates_bound_capacity_and_cancel_only_undelivered_requests() {
        let mut updates = FrontendUpdates::default();
        for n in 1..=MAX {
            updates
                .queue(
                    FrontendIntentId::from_sequence(n as u128).unwrap(),
                    broker(),
                    "update".to_owned(),
                )
                .unwrap();
        }
        assert_eq!(
            updates.queue(FrontendIntentId([99; 16]), broker(), "update".to_owned()),
            Err(FrontendQueueError::Full)
        );
        let id = FrontendIntentId::from_sequence(1).unwrap();
        let other = FrontendIntentId::from_sequence(2).unwrap();
        let revision = updates.pending(broker(), other).unwrap().revision;
        updates.delivered(other, revision);
        updates.cancel_undelivered(id);
        updates.cancel_undelivered(other);
        assert_eq!(
            updates.pop().unwrap().outcome,
            FrontendUpdateOutcome::Cancelled
        );
        assert!(
            updates.pop().is_none(),
            "delivered effects cannot be called cancelled without guest testimony"
        );
        assert!(updates.requires_progress());
        assert!(
            updates
                .queue(FrontendIntentId([99; 16]), broker(), "update".to_owned())
                .is_ok()
        );
        updates.disconnect();
        assert!(!updates.requires_progress());
        let mut completed = 0;
        while updates.pop().is_some() {
            completed += 1;
        }
        assert_eq!(completed, MAX);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Receipt {
    pub identity: SequenceIdentity,
    pub request: RequestId,
    pub turn: u64,
    pub submitted_tick: u64,
}

struct Entry {
    intent: FrontendIntentId,
    revision: u32,
    broker: SequenceIdentity,
    text: String,
    delivered: bool,
    result: Option<FrontendUpdateResultOwned>,
    receipt: Option<Receipt>,
    acknowledged: bool,
}

#[derive(Default)]
pub(crate) struct FrontendUpdates {
    entries: VecDeque<Entry>,
    revision: u32,
}

impl FrontendUpdates {
    pub fn queue(
        &mut self,
        intent: FrontendIntentId,
        broker: SequenceIdentity,
        text: String,
    ) -> Result<u32, FrontendQueueError> {
        if intent.is_zero()
            || text.is_empty()
            || text.len() > FRONTEND_TEXT_MAX
            || text.as_bytes().contains(&0)
        {
            return Err(FrontendQueueError::InvalidIntent);
        }
        if self.entries.iter().any(|entry| entry.intent == intent) {
            return Err(FrontendQueueError::Duplicate);
        }
        if self.entries.len() == MAX {
            return Err(FrontendQueueError::Full);
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or(FrontendQueueError::Full)?;
        self.entries.push_back(Entry {
            intent,
            revision: self.revision,
            broker,
            text,
            delivered: false,
            result: None,
            receipt: None,
            acknowledged: false,
        });
        Ok(self.revision)
    }

    pub fn pending(
        &self,
        broker: SequenceIdentity,
        intent: FrontendIntentId,
    ) -> Option<FrontendUpdate<'_>> {
        let entry = self.entries.iter().find(|entry| {
            entry.broker == broker
                && entry.intent == intent
                && !entry.delivered
                && entry.result.is_none()
        })?;
        Some(FrontendUpdate {
            intent_id: intent,
            revision: entry.revision,
            text: &entry.text,
        })
    }

    pub fn delivered(&mut self, intent: FrontendIntentId, revision: u32) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.intent == intent && entry.revision == revision)
        {
            entry.delivered = true;
        }
    }

    pub fn accepts(&self, broker: SequenceIdentity, result: FrontendUpdateResult<'_>) -> bool {
        result.text.len() <= FRONTEND_TEXT_MAX
            && !result.text.as_bytes().contains(&0)
            && self.entries.iter().any(|entry| {
                entry.intent == result.intent_id
                    && entry.revision == result.revision
                    && entry.broker == broker
                    && entry.delivered
                    && entry.result.is_none()
            })
    }

    pub fn accept(&mut self, receipt: Receipt, result: FrontendUpdateResult<'_>) -> bool {
        if !self.accepts(receipt.identity, result) {
            return false;
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.intent == result.intent_id && entry.revision == result.revision)
            .unwrap();
        entry.result = Some(FrontendUpdateResultOwned {
            intent_id: result.intent_id,
            revision: result.revision,
            outcome: result.outcome,
            text: result.text.to_owned(),
        });
        entry.receipt = Some(receipt);
        true
    }

    pub fn receipt(&self) -> Option<(u32, Receipt)> {
        self.entries
            .iter()
            .find_map(|entry| Some((entry.revision, entry.receipt?)))
    }

    pub fn acknowledged(&mut self, revision: u32) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.revision == revision && entry.receipt.is_some())
        {
            entry.receipt = None;
            entry.acknowledged = true;
        }
    }

    pub fn pop(&mut self) -> Option<FrontendUpdateResultOwned> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.result.is_some() && entry.acknowledged)?;
        self.entries.remove(index)?.result
    }

    pub fn pop_for(
        &mut self,
        intent: FrontendIntentId,
        revision: u32,
    ) -> Option<FrontendUpdateResultOwned> {
        let index = self.entries.iter().position(|entry| {
            entry.intent == intent
                && entry.revision == revision
                && entry.result.is_some()
                && entry.acknowledged
        })?;
        self.entries.remove(index)?.result
    }

    pub fn requires_progress(&self) -> bool {
        self.entries.iter().any(|entry| !entry.acknowledged)
    }

    pub fn cancel_undelivered(&mut self, intent: FrontendIntentId) {
        for entry in &mut self.entries {
            if entry.intent == intent && !entry.delivered && entry.result.is_none() {
                entry.result = Some(FrontendUpdateResultOwned {
                    intent_id: intent,
                    revision: entry.revision,
                    outcome: FrontendUpdateOutcome::Cancelled,
                    text: "Update cancelled before delivery.".to_owned(),
                });
                entry.acknowledged = true;
            }
        }
    }

    pub fn disconnect(&mut self) {
        self.close(None);
    }

    pub fn close_broker(&mut self, broker: SequenceIdentity) {
        self.close(Some(broker));
    }

    fn close(&mut self, broker: Option<SequenceIdentity>) {
        for entry in &mut self.entries {
            if broker.is_some_and(|broker| broker != entry.broker) {
                continue;
            }
            if entry.result.is_none() {
                entry.result = Some(FrontendUpdateResultOwned {
                    intent_id: entry.intent,
                    revision: entry.revision,
                    outcome: FrontendUpdateOutcome::Failed,
                    text: if entry.delivered {
                        "Connection lost; the update outcome is unknown."
                    } else {
                        "Connection lost before update delivery."
                    }
                    .to_owned(),
                });
            }
            entry.receipt = None;
            // Closure ends receipt delivery for this exact generation. A
            // missing guest result remains explicitly unknown, never Applied.
            entry.acknowledged = true;
        }
    }
}
