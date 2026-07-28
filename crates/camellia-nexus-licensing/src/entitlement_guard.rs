use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, RwLock},
};

use crate::{
    Capability, ClientBuildIdentity, ClientVersionDisposition, DeviceState, EntitlementState,
    LicenseInactiveReason, LicenseStanding, LicensingError, NumericLimit, ProtectedOperation,
    RestrictedOperation, Result, SafetyOperation, VerifiedEntitlement, evaluate_client_version,
};

#[derive(Debug)]
struct GuardInner {
    state: RwLock<EntitlementState>,
    pending_limits: Mutex<BTreeMap<NumericLimit, u64>>,
    client_build: ClientBuildIdentity,
}

#[derive(Debug, Clone)]
pub struct EntitlementGuard {
    inner: Arc<GuardInner>,
}

impl EntitlementGuard {
    pub fn new(state: EntitlementState, client_build: ClientBuildIdentity) -> Self {
        Self {
            inner: Arc::new(GuardInner {
                state: RwLock::new(state),
                pending_limits: Mutex::new(BTreeMap::new()),
                client_build,
            }),
        }
    }

    pub fn current_entitlement_state(&self) -> EntitlementState {
        self.inner
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn replace_state(&self, state: EntitlementState) {
        *self
            .inner
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    pub fn apply_time_policy(&self, now: i64) {
        let current = self.current_entitlement_state();
        let next = match current {
            EntitlementState::Active { entitlement } => {
                state_at(entitlement, now, &self.inner.client_build)
            }
            EntitlementState::RestrictedOffline { entitlement, .. }
            | EntitlementState::Expired { entitlement } => {
                state_at(entitlement, now, &self.inner.client_build)
            }
            state => state,
        };
        self.replace_state(next);
    }

    pub fn require_capability(&self, capability: Capability) -> Result<()> {
        let state = self.current_entitlement_state();
        let EntitlementState::Active { entitlement } = state else {
            return Err(state_denial(&state));
        };
        if entitlement.claims.capabilities.contains(&capability) {
            Ok(())
        } else {
            Err(LicensingError::CapabilityDenied)
        }
    }

    pub fn authorize(&self, operation: ProtectedOperation) -> Result<()> {
        let state = self.current_entitlement_state();
        let EntitlementState::Active { entitlement } = state else {
            return Err(state_denial(&state));
        };
        match operation.capability() {
            Some(capability) if !entitlement.claims.capabilities.contains(&capability) => {
                Err(LicensingError::CapabilityDenied)
            }
            _ => Ok(()),
        }
    }

    pub fn authorize_operation(&self, operation: RestrictedOperation) -> Result<()> {
        match operation {
            RestrictedOperation::Safety(operation) => self.authorize_safety_operation(operation),
            RestrictedOperation::Protected(operation) => self.authorize(operation),
        }
    }

    pub const fn authorize_safety_operation(&self, _operation: SafetyOperation) -> Result<()> {
        Ok(())
    }

    pub fn reserve_limit(
        &self,
        limit: NumericLimit,
        current_count: u64,
        requested_count: u64,
    ) -> Result<LimitReservation> {
        let state = self.current_entitlement_state();
        let EntitlementState::Active { entitlement } = state else {
            return Err(state_denial(&state));
        };
        let maximum = entitlement
            .claims
            .limits
            .get(&limit)
            .copied()
            .ok_or(LicensingError::LimitExceeded)?;
        let mut pending = self
            .inner
            .pending_limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let reserved = pending.get(&limit).copied().unwrap_or(0);
        if current_count
            .checked_add(reserved)
            .and_then(|value| value.checked_add(requested_count))
            .is_none_or(|value| value > maximum)
        {
            return Err(LicensingError::LimitExceeded);
        }
        pending.insert(limit, reserved + requested_count);
        Ok(LimitReservation {
            inner: self.inner.clone(),
            limit,
            count: requested_count,
            released: false,
        })
    }

    pub fn require_limit(&self, limit: NumericLimit, requested_count: u64) -> Result<()> {
        let state = self.current_entitlement_state();
        let EntitlementState::Active { entitlement } = state else {
            return Err(state_denial(&state));
        };
        let maximum = entitlement
            .claims
            .limits
            .get(&limit)
            .copied()
            .ok_or(LicensingError::LimitExceeded)?;
        if requested_count > maximum {
            return Err(LicensingError::LimitExceeded);
        }
        Ok(())
    }
}

fn offline_state(entitlement: VerifiedEntitlement, now: i64) -> EntitlementState {
    let mut safety_window_ends_at = entitlement.claims.offline_access_ends_at;
    if let Some(license_expires_at) = entitlement.claims.license_expires_at {
        safety_window_ends_at = safety_window_ends_at.min(license_expires_at);
    }
    if safety_window_ends_at > now {
        EntitlementState::RestrictedOffline {
            entitlement,
            safety_window_ends_at,
        }
    } else {
        EntitlementState::Expired { entitlement }
    }
}

pub(crate) fn state_at(
    entitlement: VerifiedEntitlement,
    now: i64,
    client_build: &ClientBuildIdentity,
) -> EntitlementState {
    match evaluate_client_version(client_build, &entitlement.claims.client_version_policy, now) {
        Ok(ClientVersionDisposition::UpgradeRequired) => {
            return EntitlementState::ClientUpgradeRequired {
                policy: entitlement.claims.client_version_policy.clone(),
                entitlement: Some(entitlement),
            };
        }
        Err(_) => {
            return EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::InvalidServerProof,
            };
        }
        _ => {}
    }
    if let Some(reason) = commercial_inactive_reason(&entitlement, now) {
        return EntitlementState::LicenseInactive {
            reason,
            entitlement: Some(entitlement),
        };
    }
    if entitlement.claims.expires_at > now {
        EntitlementState::Active { entitlement }
    } else {
        offline_state(entitlement, now)
    }
}

fn commercial_inactive_reason(
    entitlement: &VerifiedEntitlement,
    now: i64,
) -> Option<LicenseInactiveReason> {
    let term_ended = entitlement
        .claims
        .license_expires_at
        .is_some_and(|expires_at| expires_at <= now);
    match entitlement.claims.license_status {
        LicenseStanding::Active if term_ended => Some(LicenseInactiveReason::LicenseExpired),
        LicenseStanding::PastDue
            if term_ended || entitlement.claims.license_expires_at.is_none() =>
        {
            Some(LicenseInactiveReason::LicensePastDue)
        }
        LicenseStanding::Canceled
            if term_ended || entitlement.claims.license_expires_at.is_none() =>
        {
            Some(LicenseInactiveReason::LicenseCanceled)
        }
        _ => None,
    }
}

fn state_denial(state: &EntitlementState) -> LicensingError {
    match state {
        EntitlementState::DeviceDenied {
            state: DeviceState::Removed,
        } => LicensingError::DeviceRemoved,
        EntitlementState::DeviceDenied {
            state: DeviceState::Revoked,
        } => LicensingError::DeviceRevoked,
        EntitlementState::DeviceDenied {
            state: DeviceState::Suspicious,
        } => LicensingError::DeviceSuspicious,
        EntitlementState::LicenseInactive { reason, .. } => match reason {
            crate::LicenseInactiveReason::AccountSuspended => LicensingError::AccountSuspended,
            crate::LicenseInactiveReason::AccountDenylisted => LicensingError::AccountDenylisted,
            crate::LicenseInactiveReason::LicensePastDue => LicensingError::LicensePastDue,
            crate::LicenseInactiveReason::LicenseCanceled => LicensingError::LicenseCanceled,
            crate::LicenseInactiveReason::LicenseExpired => LicensingError::LicenseExpired,
            crate::LicenseInactiveReason::LicenseUnavailable => LicensingError::LicenseUnavailable,
        },
        EntitlementState::ClientUpgradeRequired { policy, .. } => {
            LicensingError::ClientUpgradeRequired {
                policy: policy.clone(),
            }
        }
        _ => LicensingError::AuthorizationRequired,
    }
}

pub struct LimitReservation {
    inner: Arc<GuardInner>,
    limit: NumericLimit,
    count: u64,
    released: bool,
}

impl LimitReservation {
    pub fn release(mut self) {
        self.do_release();
    }

    fn do_release(&mut self) {
        if self.released {
            return;
        }
        let mut pending = self
            .inner
            .pending_limits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remaining = pending
            .get(&self.limit)
            .copied()
            .unwrap_or(0)
            .saturating_sub(self.count);
        if remaining == 0 {
            pending.remove(&self.limit);
        } else {
            pending.insert(self.limit, remaining);
        }
        self.released = true;
    }
}

impl Drop for LimitReservation {
    fn drop(&mut self) {
        self.do_release();
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Barrier, thread};

    use super::*;
    use crate::{EntitlementClaims, Plan};

    fn test_build() -> ClientBuildIdentity {
        ClientBuildIdentity::parse("1.0.0").unwrap()
    }

    fn entitlement(expires_at: i64) -> VerifiedEntitlement {
        VerifiedEntitlement {
            key_id: "key".into(),
            claims: EntitlementClaims {
                schema_version: 3,
                iss: "issuer".into(),
                aud: "audience".into(),
                sub: "account".into(),
                license_id: "license".into(),
                device_id: "device".into(),
                device_key_thumbprint: "thumbprint".into(),
                plan: Plan::Pro,
                plan_revision: 2,
                policy_hash: "0".repeat(64),
                license_status: crate::LicenseStanding::Active,
                capabilities: vec![Capability::ManagedConfigSources],
                workspace_permissions: Vec::new(),
                limits: BTreeMap::from([
                    (NumericLimit::MaxPrograms, 2),
                    (NumericLimit::MaxConfigSourcesPerProgram, 20),
                    (NumericLimit::MaxTeamMembers, 1),
                    (NumericLimit::MaxRemoteMonitors, 3),
                    (NumericLimit::MaxSharedPrograms, 0),
                    (NumericLimit::MaxWebhookEndpoints, 0),
                    (NumericLimit::MaxWorkspaceStorageBytes, 0),
                    (NumericLimit::MaxAlertRules, 0),
                    (NumericLimit::MaxAuditExportEvents, 0),
                ]),
                client_version_policy: crate::ClientVersionPolicy {
                    minimum_version: "1.0.0".into(),
                    recommended_version: "1.0.0".into(),
                    enforce_after: 10_000,
                },
                license_expires_at: None,
                license_epoch: 1,
                device_limit: 3,
                member_limit: 1,
                issued_at: 100,
                refresh_after: 200,
                expires_at,
                offline_access_ends_at: expires_at.saturating_add(24 * 60 * 60),
                token_id: "token".into(),
                key_id: "key".into(),
            },
        }
    }

    #[test]
    fn offline_policy_preserves_safety_but_denies_protected_writes() {
        let guard = EntitlementGuard::new(
            EntitlementState::Active {
                entitlement: entitlement(1_000),
            },
            test_build(),
        );
        guard.apply_time_policy(1_001);
        assert!(matches!(
            guard.current_entitlement_state(),
            EntitlementState::RestrictedOffline { .. }
        ));
        guard
            .authorize_safety_operation(SafetyOperation::Stop)
            .expect("stop remains available");
        assert!(matches!(
            guard.authorize(ProtectedOperation::UseManagedConfigSources),
            Err(LicensingError::AuthorizationRequired)
        ));
        guard.apply_time_policy(1_000 + 24 * 60 * 60 + 1);
        assert!(matches!(
            guard.current_entitlement_state(),
            EntitlementState::Expired { .. }
        ));
    }

    #[test]
    fn minimum_client_version_becomes_hard_inactive_at_the_exact_boundary() {
        let mut old_client_lease = entitlement(500);
        old_client_lease.claims.refresh_after = 400;
        old_client_lease.claims.client_version_policy = crate::ClientVersionPolicy {
            minimum_version: "2.0.0".into(),
            recommended_version: "2.1.0".into(),
            enforce_after: 500,
        };
        let guard = EntitlementGuard::new(
            EntitlementState::Active {
                entitlement: old_client_lease,
            },
            test_build(),
        );
        guard.apply_time_policy(499);
        assert!(matches!(
            guard.current_entitlement_state(),
            EntitlementState::Active { .. }
        ));
        guard.apply_time_policy(500);
        assert!(matches!(
            guard.current_entitlement_state(),
            EntitlementState::ClientUpgradeRequired { .. }
        ));
        assert!(matches!(
            guard.authorize(ProtectedOperation::UseManagedConfigSources),
            Err(LicensingError::ClientUpgradeRequired { .. })
        ));

        let mut supported_lease = entitlement(2_000);
        supported_lease.claims.client_version_policy = crate::ClientVersionPolicy {
            minimum_version: "2.0.0".into(),
            recommended_version: "2.1.0".into(),
            enforce_after: 500,
        };
        let supported = EntitlementGuard::new(
            EntitlementState::Active {
                entitlement: supported_lease,
            },
            ClientBuildIdentity::parse("2.0.0").unwrap(),
        );
        supported.apply_time_policy(500);
        assert!(matches!(
            supported.current_entitlement_state(),
            EntitlementState::Active { .. }
        ));
    }

    #[test]
    fn offline_safety_never_extends_the_commercial_license_term() {
        let mut fixed_term = entitlement(1_100);
        fixed_term.claims.license_expires_at = Some(1_100);
        let guard = EntitlementGuard::new(
            EntitlementState::Active {
                entitlement: fixed_term,
            },
            test_build(),
        );
        guard.apply_time_policy(1_100);
        assert!(matches!(
            guard.current_entitlement_state(),
            EntitlementState::LicenseInactive {
                reason: LicenseInactiveReason::LicenseExpired,
                ..
            }
        ));

        let mut canceled_grace = entitlement(1_100);
        canceled_grace.claims.license_status = LicenseStanding::Canceled;
        canceled_grace.claims.license_expires_at = Some(1_200);
        let guard = EntitlementGuard::new(
            EntitlementState::Active {
                entitlement: canceled_grace,
            },
            test_build(),
        );
        guard.apply_time_policy(1_101);
        assert!(matches!(
            guard.current_entitlement_state(),
            EntitlementState::RestrictedOffline {
                safety_window_ends_at: 1_200,
                ..
            }
        ));
        guard.apply_time_policy(1_200);
        assert!(matches!(
            guard.current_entitlement_state(),
            EntitlementState::LicenseInactive {
                reason: LicenseInactiveReason::LicenseCanceled,
                ..
            }
        ));
    }

    #[test]
    fn enforces_capabilities_and_limits() {
        let guard = EntitlementGuard::new(
            EntitlementState::Active {
                entitlement: entitlement(2_000),
            },
            test_build(),
        );
        guard
            .require_capability(Capability::ManagedConfigSources)
            .expect("capability");
        assert!(matches!(
            guard.require_capability(Capability::CloudSync),
            Err(LicensingError::CapabilityDenied)
        ));
        let first = guard
            .reserve_limit(NumericLimit::MaxPrograms, 0, 1)
            .expect("first reservation");
        let second = guard
            .reserve_limit(NumericLimit::MaxPrograms, 0, 1)
            .expect("second reservation");
        assert!(matches!(
            guard.reserve_limit(NumericLimit::MaxPrograms, 0, 1),
            Err(LicensingError::LimitExceeded)
        ));
        guard
            .require_limit(NumericLimit::MaxPrograms, 2)
            .expect("limit boundary is allowed");
        assert!(matches!(
            guard.require_limit(NumericLimit::MaxPrograms, 3),
            Err(LicensingError::LimitExceeded)
        ));
        drop((first, second));
    }

    #[test]
    fn concurrent_limit_reservations_cannot_exceed_the_limit() {
        let guard = EntitlementGuard::new(
            EntitlementState::Active {
                entitlement: entitlement(2_000),
            },
            test_build(),
        );
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let guard = guard.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                let reservation = guard.reserve_limit(NumericLimit::MaxPrograms, 1, 1);
                barrier.wait();
                reservation.is_ok()
            }));
        }
        barrier.wait();
        barrier.wait();
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .filter(|succeeded| *succeeded)
            .count();
        assert_eq!(successes, 1);
    }
}
