#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    LoginSucceeded,
    LoginFailed,
    DeviceRegistered,
    DeviceRemoved,
    EntitlementIssued,
    EntitlementRefreshed,
    EntitlementDenied,
    ClientUpgradeRequired,
    DeviceRevoked,
    SuspiciousClockRollback,
    RefreshTokenReuseDetected,
    SessionRecovered,
    DeviceIdentityReset,
    ActivationLimitDenied,
    SigningKeyRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Succeeded,
    Denied,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub event_type: AuditEventType,
    pub outcome: AuditOutcome,
    pub occurred_at: i64,
    pub device_id: Option<String>,
    pub license_id: Option<String>,
    pub reason_code: Option<String>,
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

#[derive(Debug, Default)]
pub struct TracingAuditSink;

impl AuditSink for TracingAuditSink {
    fn record(&self, event: AuditEvent) {
        tracing::info!(
            audit_event = ?event.event_type,
            outcome = ?event.outcome,
            occurred_at = event.occurred_at,
            device_id = event.device_id.as_deref().unwrap_or(""),
            license_id = event.license_id.as_deref().unwrap_or(""),
            reason_code = event.reason_code.as_deref().unwrap_or(""),
            "licensing audit event"
        );
    }
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Default)]
pub struct InMemoryAuditSink {
    events: Mutex<Vec<AuditEvent>>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryAuditSink {
    pub fn events(&self) -> Vec<AuditEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AuditSink for InMemoryAuditSink {
    fn record(&self, event: AuditEvent) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}
