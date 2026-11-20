#![forbid(unsafe_code)]
//! Mid-run steering inbox and orphan-safe abort synthesis.
//!
//! Signals are recorded on `SessionTree` immediately upon enqueue so
//! interruptions and redirects remain crash-safe and deterministically replayable.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::effect::StepResult;

/// Timing mode of a [`SteeringSignal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringKind {
    /// Applied between Steps once the active effect batch finishes.
    Steer,
    /// Cancels in-flight step I/O preemptively and injects steering immediately.
    UrgentSteer,
    /// Queued until the active Turn completes, then starts Turn `N + 1`.
    FollowUp,
    /// Cancels in-flight step I/O and aborts the active Turn cleanly.
    Abort,
}

/// Originator of a [`SteeringSignal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteeringSource {
    /// Interactive operator or CLI/ACP client.
    User,
    /// Parent agent or external supervisor.
    Supervisor,
    /// Automated policy or safety gate.
    Gate,
}

/// A durable steering or interruption signal stored in the `SessionTree` inbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SteeringSignal {
    /// Deterministic signal identifier.
    pub id: String,
    /// Timing and interruption mode.
    pub kind: SteeringKind,
    /// Arbitrary JSON instruction or cancellation reason.
    pub content: Value,
    /// Entity that issued the signal.
    pub source: SteeringSource,
    /// Deterministic timestamp in milliseconds since Unix epoch.
    pub timestamp_ms: u64,
}

/// Kernel-managed steering queue enforcing intra-turn, preemptive, and
/// between-turn delivery guarantees.
#[derive(Debug, Clone, Default)]
pub struct SteeringInbox {
    intra_turn: VecDeque<SteeringSignal>,
    follow_ups: VecDeque<SteeringSignal>,
    pending_abort: Option<SteeringSignal>,
    pending_urgent_interrupt: bool,
}

impl SteeringInbox {
    /// Create an empty [`SteeringInbox`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a [`SteeringSignal`] into the appropriate delivery lane.
    ///
    /// * `Steer` -> intra-turn FIFO queue (carries across `Effect::Switch`).
    /// * `UrgentSteer` -> intra-turn FIFO queue + flags preemptive step interrupt.
    /// * `FollowUp` -> next-turn FIFO queue.
    /// * `Abort` -> sets pending Turn abort signal.
    pub fn enqueue(&mut self, signal: SteeringSignal) {
        match signal.kind {
            SteeringKind::Steer => self.intra_turn.push_back(signal),
            SteeringKind::UrgentSteer => {
                self.pending_urgent_interrupt = true;
                self.intra_turn.push_back(signal);
            }
            SteeringKind::FollowUp => self.follow_ups.push_back(signal),
            SteeringKind::Abort => self.pending_abort = Some(signal),
        }
    }

    /// Drain all pending intra-turn (`Steer` / `UrgentSteer`) signals in FIFO order.
    pub fn drain_steering(&mut self) -> Vec<SteeringSignal> {
        self.pending_urgent_interrupt = false;
        self.intra_turn.drain(..).collect()
    }

    /// Return `true` if a preemptive `Abort` is pending.
    #[must_use]
    pub fn has_abort(&self) -> bool {
        self.pending_abort.is_some()
    }

    /// Return `true` if an `UrgentSteer` requires cancelling the active in-flight step.
    #[must_use]
    pub fn has_urgent_interrupt(&self) -> bool {
        self.pending_urgent_interrupt
    }

    /// Clear and return the pending `Abort` signal, if any.
    pub fn take_abort(&mut self) -> Option<SteeringSignal> {
        self.pending_abort.take()
    }

    /// Clear the urgent-interrupt flag once in-flight step I/O has been aborted,
    /// keeping the `UrgentSteer` message queued for the next `drain_steering()`.
    pub fn acknowledge_urgent_interrupt(&mut self) {
        self.pending_urgent_interrupt = false;
    }

    /// Promote any un-drained intra-turn `Steer` / `UrgentSteer` signals into
    /// `FollowUp` signals when a Turn finishes so user input is never dropped.
    ///
    /// # Returns
    ///
    /// The promoted [`SteeringSignal`]s in FIFO order.
    pub fn promote_undrained_to_follow_up(&mut self) -> Vec<SteeringSignal> {
        self.pending_urgent_interrupt = false;
        let mut promoted = Vec::with_capacity(self.intra_turn.len());
        while let Some(mut sig) = self.intra_turn.pop_front() {
            sig.kind = SteeringKind::FollowUp;
            self.follow_ups.push_back(sig.clone());
            promoted.push(sig);
        }
        promoted
    }

    /// Pop the next queued `FollowUp` signal to start Turn `N + 1`.
    pub fn pop_next_follow_up(&mut self) -> Option<SteeringSignal> {
        self.follow_ups.pop_front()
    }

    /// Return the number of un-drained intra-turn signals.
    #[must_use]
    pub fn pending_steering_count(&self) -> usize {
        self.intra_turn.len()
    }

    /// Return the number of queued `FollowUp` signals.
    #[must_use]
    pub fn pending_follow_up_count(&self) -> usize {
        self.follow_ups.len()
    }
}

/// Synthesize [`StepResult::Aborted`] entries for every unpaired `effect_id`
/// in deterministic order so the `SessionTree` never contains orphan tool calls.
///
/// # Arguments
///
/// * `unpaired_effect_ids` - Slice of in-flight effect IDs without a terminal result
/// * `reason` - Human-readable cancellation reason
///
/// # Returns
///
/// Vector of [`StepResult::Aborted`] in the same order as `unpaired_effect_ids`.
#[must_use]
pub fn synthesize_aborted_results(unpaired_effect_ids: &[String], reason: &str) -> Vec<StepResult> {
    unpaired_effect_ids
        .iter()
        .map(|effect_id| StepResult::Aborted {
            effect_id: effect_id.clone(),
            reason: reason.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sig(id: &str, kind: SteeringKind) -> SteeringSignal {
        SteeringSignal {
            id: id.to_string(),
            kind,
            content: json!({"msg": id}),
            source: SteeringSource::User,
            timestamp_ms: 100,
        }
    }

    #[test]
    fn undrained_steer_promotes_to_follow_up_at_turn_end() {
        let mut inbox = SteeringInbox::new();
        inbox.enqueue(sig("s1", SteeringKind::Steer));
        inbox.enqueue(sig("s2", SteeringKind::UrgentSteer));
        assert!(inbox.has_urgent_interrupt());

        let promoted = inbox.promote_undrained_to_follow_up();
        assert_eq!(promoted.len(), 2);
        assert!(!inbox.has_urgent_interrupt());
        assert_eq!(inbox.pending_steering_count(), 0);
        assert_eq!(inbox.pending_follow_up_count(), 2);

        let first = inbox.pop_next_follow_up().expect("first follow-up");
        assert_eq!(first.id, "s1");
        assert_eq!(first.kind, SteeringKind::FollowUp);
    }

    #[test]
    fn synthesize_aborted_preserves_effect_order() {
        let ids = vec!["eff_2".to_string(), "eff_3".to_string()];
        let aborted = synthesize_aborted_results(&ids, "urgent steer");
        assert_eq!(aborted.len(), 2);
        assert_eq!(aborted[0].effect_id(), "eff_2");
        assert_eq!(aborted[1].effect_id(), "eff_3");
    }
}
