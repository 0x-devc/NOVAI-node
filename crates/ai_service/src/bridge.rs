//! Bridge between the synchronous copilot thread and the async AI service.
//!
//! PURPOSE: The copilot observer runs in a background std::thread (Thread 2),
//! detecting anomalies every 500ms. When it detects something, it calls
//! `AnomalyCallback::on_anomaly`. This module provides `AiTriggerCallback`,
//! which sends a lightweight trigger message through a bounded mpsc channel
//! to the async AI service thread (Thread 3) for deeper analysis.
//!
//! INVARIANTS:
//! - Never blocks the copilot thread (uses try_send, not send)
//! - Drops triggers silently if the channel is full (backpressure)
//! - AnomalyTrigger is a lightweight struct (no large allocations)
//!
//! FAILURE MODES:
//! - Channel full → trigger dropped, warning logged
//! - Receiver dropped → triggers silently fail (AI service stopped)

use novai_ai_entities::{AiSignalType, AiSignalV1, SignalPayload};
use novai_copilot::observer::AnomalyCallback;
use tokio::sync::mpsc;

/// Lightweight trigger sent from the copilot thread to the AI service.
#[derive(Debug, Clone)]
pub struct AnomalyTrigger {
    /// Type of signal detected by the copilot.
    pub signal_type: AiSignalType,

    /// Confidence level from the anomaly detector (0–255).
    pub confidence: u8,

    /// Human-readable description of the anomaly.
    pub details: String,

    /// Block height when the anomaly was detected.
    pub height: u64,
}

/// Callback that bridges copilot anomalies to the AI service via a channel.
///
/// Implements `AnomalyCallback` from the copilot crate. When `on_anomaly`
/// is called, it converts the signal into a lightweight `AnomalyTrigger`
/// and sends it through a bounded mpsc channel.
pub struct AiTriggerCallback {
    tx: mpsc::Sender<AnomalyTrigger>,
}

impl AiTriggerCallback {
    /// Create a new trigger callback with the given channel sender.
    #[must_use]
    pub fn new(tx: mpsc::Sender<AnomalyTrigger>) -> Self {
        Self { tx }
    }
}

impl AnomalyCallback for AiTriggerCallback {
    fn on_anomaly(&self, _payload: SignalPayload, signal: AiSignalV1) {
        let trigger = AnomalyTrigger {
            signal_type: signal.signal_type,
            confidence: signal.confidence,
            details: format!(
                "{:?} at height {} (confidence {})",
                signal.signal_type, signal.height, signal.confidence
            ),
            height: signal.height,
        };

        // Non-blocking send — never stall the copilot thread
        if let Err(e) = self.tx.try_send(trigger) {
            tracing::warn!(
                %e,
                "AI trigger channel full or closed — dropping anomaly trigger"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use novai_ai_entities::AiSignalType;

    fn make_test_signal(signal_type: AiSignalType, height: u64, confidence: u8) -> AiSignalV1 {
        AiSignalV1 {
            signal_type,
            height,
            issuer: [0x42; 32],
            confidence,
            payload_hash: [0; 32],
            zk_proof: None,
            signature: [0; 64],
        }
    }

    fn make_test_payload() -> SignalPayload {
        SignalPayload::new(
            "test".to_string(),
            "1.0".to_string(),
            "input".to_string(),
            vec![],
            "explanation".to_string(),
        )
    }

    #[test]
    fn ai_trigger_callback_sends() {
        let (tx, mut rx) = mpsc::channel(32);
        let callback = AiTriggerCallback::new(tx);

        let signal = make_test_signal(AiSignalType::Anomaly, 100, 200);
        let payload = make_test_payload();

        callback.on_anomaly(payload, signal);

        let trigger = rx.try_recv().expect("should receive trigger");
        assert_eq!(trigger.signal_type, AiSignalType::Anomaly);
        assert_eq!(trigger.confidence, 200);
        assert_eq!(trigger.height, 100);
        assert!(trigger.details.contains("Anomaly"));
        assert!(trigger.details.contains("100"));
    }

    #[test]
    fn ai_trigger_callback_full_channel() {
        // Channel capacity 1
        let (tx, _rx) = mpsc::channel(1);
        let callback = AiTriggerCallback::new(tx);

        let signal1 = make_test_signal(AiSignalType::Anomaly, 1, 100);
        let signal2 = make_test_signal(AiSignalType::Anomaly, 2, 150);
        let signal3 = make_test_signal(AiSignalType::Anomaly, 3, 200);

        // First send succeeds
        callback.on_anomaly(make_test_payload(), signal1);

        // Second and third should drop without panic
        callback.on_anomaly(make_test_payload(), signal2);
        callback.on_anomaly(make_test_payload(), signal3);
    }

    #[test]
    fn ai_trigger_callback_closed_channel() {
        let (tx, rx) = mpsc::channel(32);
        let callback = AiTriggerCallback::new(tx);

        // Drop receiver
        drop(rx);

        let signal = make_test_signal(AiSignalType::Anomaly, 100, 200);
        // Should not panic
        callback.on_anomaly(make_test_payload(), signal);
    }

    #[test]
    fn anomaly_trigger_fields() {
        let trigger = AnomalyTrigger {
            signal_type: AiSignalType::CongestionForecast,
            confidence: 180,
            details: "test details".into(),
            height: 500,
        };

        assert_eq!(trigger.signal_type, AiSignalType::CongestionForecast);
        assert_eq!(trigger.confidence, 180);
        assert_eq!(trigger.height, 500);
        assert_eq!(trigger.details, "test details");
    }
}
