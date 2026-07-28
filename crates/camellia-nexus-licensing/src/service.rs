use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    ActivationProofVerifier, AuditEvent, AuditEventType, AuditOutcome, AuditSink,
    DeviceIdentityProvider, DeviceState, DynSecureStore, EntitlementGuard, EntitlementState,
    EntitlementVerifier, EntitlementVerifierConfig, LicenseApi, LicensingError, Result, SecretKey,
    SecureStoreMode, TracingAuditSink, TrustedEntitlementKeys, TrustedTime,
    VerifiedActivationProof, VerifiedEntitlement, get_json, put_json,
};

#[derive(Debug, Clone)]
pub struct LicensingAuthority {
    pub issuer: String,
    pub audience: String,
    pub minimum_license_epoch: u64,
    pub keys: TrustedEntitlementKeys,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshMetadata {
    pub refreshed_at: i64,
    pub token_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LicenseDenialMarker {
    reason: crate::LicenseInactiveReason,
    observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevalidationMarker {
    reason: crate::RevalidationReason,
    observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientUpgradeMarker {
    blocked_build_version: String,
    policy: crate::ClientVersionPolicy,
    observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "reason",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum AuthorizationBlockMarker {
    LocalDeauthorization {
        observed_at: i64,
    },
    DeviceDenied {
        state: DeviceState,
        observed_at: i64,
    },
}

#[derive(Debug, Clone)]
pub struct DeviceRegistrationFlow {
    pub authorization_code: crate::SecretValue,
    pub pkce_verifier: crate::SecretValue,
    pub redirect_uri: String,
    pub platform: String,
    pub display_name: Option<String>,
    pub local_unix: i64,
}

#[derive(Debug, Clone)]
pub struct ActivationProofVerification {
    pub proof: VerifiedActivationProof,
    pub rotated_refresh_session: crate::RefreshSession,
    pub device_state: DeviceState,
}

pub struct AuthorizationService {
    store: DynSecureStore,
    guard: EntitlementGuard,
    authority: LicensingAuthority,
    verifier: RwLock<Option<EntitlementVerifier>>,
    trusted_time: TrustedTime,
    audit: Arc<dyn AuditSink>,
    credential_operations: LocalCredentialOperations,
    client_build: crate::ClientBuildIdentity,
}

const LOCAL_CREDENTIAL_TIMEOUT: Duration = Duration::from_secs(12);
const DEVICE_LIST_SCOPE: &str = "device:list";
const BILLING_SUMMARY_SCOPE: &str = "billing:summary";
const BILLING_PAYMENT_SCOPE: &str = "billing:payment:submit";
const TEAM_PROFILE_SCOPE: &str = "team:profile";
const TEAM_MEMBERS_SCOPE: &str = "team:members";
const TEAM_INVITATION_CREATE_SCOPE: &str = "team:invitation:create";
const TEAM_INVITATION_ACCEPT_SCOPE: &str = "team:invitation:accept";
const TEAM_DEVICE_ENROLLMENT_CREATE_SCOPE: &str = "team:device-enrollment:create";
const TEAM_DEVICE_ENROLLMENT_ACCEPT_SCOPE: &str = "team:device-enrollment:accept";
const TEAM_LEAVE_SCOPE: &str = "team:leave";
const TEAM_OWNERSHIP_TRANSFER_SCOPE: &str = "team:ownership:transfer";
const WORKSPACE_SHARED_READ_SCOPE: &str = "workspace:shared:read";
const WORKSPACE_SHARED_WRITE_SCOPE: &str = "workspace:shared:write";
const WORKSPACE_SHARED_PUBLISH_SCOPE: &str = "workspace:shared:publish";
const WORKSPACE_SHARED_PURGE_SCOPE: &str = "workspace:shared:purge";
const WORKSPACE_SYNC_READ_SCOPE: &str = "workspace:sync:read";
const WORKSPACE_SYNC_WRITE_SCOPE: &str = "workspace:sync:write";
const WORKSPACE_ALERTS_READ_SCOPE: &str = "workspace:alerts:read";
const WORKSPACE_ALERTS_MANAGE_SCOPE: &str = "workspace:alerts:manage";
const WORKSPACE_ALERTS_ACK_SCOPE: &str = "workspace:alerts:ack";
const WORKSPACE_AUDIT_READ_SCOPE: &str = "workspace:audit:read";
const WORKSPACE_AUDIT_EXPORT_SCOPE: &str = "workspace:audit:export";
const WORKSPACE_WEBHOOK_LIST_SCOPE: &str = "workspace:webhooks:list";
const WORKSPACE_WEBHOOK_CREATE_SCOPE: &str = "workspace:webhooks:create";
const WORKSPACE_WEBHOOK_DELIVERIES_SCOPE: &str = "workspace:webhooks:deliveries";

#[derive(Clone)]
struct LocalCredentialOperations {
    gate: Arc<tokio::sync::Mutex<()>>,
    timeout: Duration,
}

impl LocalCredentialOperations {
    fn new() -> Self {
        Self {
            gate: Arc::new(tokio::sync::Mutex::new(())),
            timeout: LOCAL_CREDENTIAL_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_timeout(timeout: Duration) -> Self {
        Self {
            gate: Arc::new(tokio::sync::Mutex::new(())),
            timeout,
        }
    }

    async fn run<T, F>(&self, stage: &'static str, operation: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T> + Send + 'static,
    {
        let deadline = tokio::time::Instant::now() + self.timeout;
        tracing::debug!(%stage, "local credential operation queued");
        let permit = tokio::time::timeout_at(deadline, self.gate.clone().lock_owned())
            .await
            .map_err(|_| {
                tracing::warn!(%stage, "local credential operation queue timed out");
                LicensingError::SecureStoreTimeout
            })?;
        tracing::debug!(%stage, "local credential operation started");

        let blocking_task = tokio::task::spawn_blocking(operation);
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = match blocking_task.await {
                Ok(result) => result,
                Err(error) => {
                    tracing::warn!(%stage, %error, "local credential operation failed");
                    Err(LicensingError::SecureStoreBackend)
                }
            };
            tracing::debug!(%stage, "local credential operation settled");
            let _ = result_sender.send(result);
            drop(permit);
        });

        match tokio::time::timeout_at(deadline, result_receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(LicensingError::SecureStoreBackend),
            Err(_) => {
                // The worker owns the gate until the blocking operation really
                // finishes. A timeout or caller cancellation therefore cannot
                // let a later credential write overtake this operation.
                tracing::warn!(%stage, "local credential operation timed out");
                Err(LicensingError::SecureStoreTimeout)
            }
        }
    }
}

fn store_refresh_session(store: DynSecureStore, new_session: &[u8]) -> Result<()> {
    if new_session.len() < 24 || new_session.len() > 16 * 1024 {
        return Err(LicensingError::InvalidServerResponse);
    }
    store.put_secret(SecretKey::RefreshSession, new_session)
}

fn load_refresh_session(store: DynSecureStore) -> Result<crate::RefreshSession> {
    let bytes = store
        .get_secret(SecretKey::RefreshSession)?
        .ok_or(LicensingError::AuthorizationRequired)?;
    if bytes.len() < 24 || bytes.len() > 16 * 1024 {
        return Err(LicensingError::SecureStoreCorrupt);
    }
    let value = String::from_utf8(bytes).map_err(|_| LicensingError::SecureStoreCorrupt)?;
    Ok(crate::RefreshSession(value))
}

fn clear_persisted_session(store: &dyn crate::SecureStore) -> Result<()> {
    let mut first_error = None;
    for key in [
        SecretKey::RefreshSession,
        SecretKey::EntitlementLease,
        SecretKey::RefreshMetadata,
        SecretKey::TrustedTime,
        SecretKey::LicenseDenial,
        SecretKey::RevalidationMarker,
        SecretKey::ClientUpgradeMarker,
    ] {
        if let Err(error) = store.delete_secret(key)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn clear_cached_authorization_material(store: &dyn crate::SecureStore) -> Result<()> {
    let mut first_error = None;
    for key in [SecretKey::RefreshSession, SecretKey::EntitlementLease] {
        if let Err(error) = store.delete_secret(key)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

impl std::fmt::Debug for AuthorizationService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizationService")
            .field("state", &self.guard.current_entitlement_state())
            .field("secure_store_mode", &self.store.mode())
            .finish_non_exhaustive()
    }
}

impl AuthorizationService {
    pub fn initialize(
        store: DynSecureStore,
        authority: LicensingAuthority,
        client_build: crate::ClientBuildIdentity,
        local_unix: i64,
    ) -> Self {
        Self::initialize_with_audit(
            store,
            authority,
            client_build,
            local_unix,
            Arc::new(TracingAuditSink),
        )
    }

    pub fn initialize_with_audit(
        store: DynSecureStore,
        authority: LicensingAuthority,
        client_build: crate::ClientBuildIdentity,
        local_unix: i64,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        let trusted_time = TrustedTime::new(store.clone());
        let fallback = if store.mode() == SecureStoreMode::SessionOnly {
            EntitlementState::SessionOnly
        } else {
            EntitlementState::Unauthenticated
        };
        let service = Self {
            store: store.clone(),
            guard: EntitlementGuard::new(fallback, client_build.clone()),
            authority: authority.clone(),
            verifier: RwLock::new(None),
            trusted_time,
            audit,
            credential_operations: LocalCredentialOperations::new(),
            client_build,
        };
        if store.mode() == SecureStoreMode::Persistent {
            match get_json::<AuthorizationBlockMarker>(
                store.as_ref(),
                SecretKey::AuthorizationBlock,
            ) {
                Ok(Some(marker)) => {
                    let _ = clear_cached_authorization_material(store.as_ref());
                    if let AuthorizationBlockMarker::DeviceDenied { state, .. } = marker {
                        service
                            .guard
                            .replace_state(EntitlementState::DeviceDenied { state });
                    }
                    return service;
                }
                Ok(None) => {}
                Err(_) => {
                    service
                        .guard
                        .replace_state(EntitlementState::RevalidationRequired {
                            reason: crate::RevalidationReason::CorruptSecureStore,
                        });
                    return service;
                }
            }
        }
        let metadata = match get_json::<crate::DeviceRegistrationMetadata>(
            store.as_ref(),
            SecretKey::DeviceRegistration,
        ) {
            Ok(metadata) => metadata,
            Err(_) => {
                service
                    .guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::CorruptSecureStore,
                    });
                return service;
            }
        };
        let Some(metadata) = metadata else {
            return service;
        };
        let verifier = service.verifier_for(&metadata);
        let trusted = service.trusted_time.observe(local_unix);
        let trusted_now = match trusted {
            Ok(Some(observation)) if observation.rollback_detected => {
                service
                    .guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::ClockRollback,
                    });
                service.audit.record(AuditEvent {
                    event_type: AuditEventType::SuspiciousClockRollback,
                    outcome: AuditOutcome::Denied,
                    occurred_at: local_unix,
                    device_id: None,
                    license_id: None,
                    reason_code: Some("clock_rollback".into()),
                });
                *service
                    .verifier
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verifier);
                return service;
            }
            Ok(Some(observation)) => Some(observation.unix),
            Ok(None) => None,
            Err(_) => {
                service
                    .guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::CorruptSecureStore,
                    });
                *service
                    .verifier
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verifier);
                return service;
            }
        };
        if store.mode() == SecureStoreMode::Persistent {
            match store.get_secret(SecretKey::EntitlementLease) {
                Ok(Some(token)) => {
                    let token = Zeroizing::new(token);
                    let restored = trusted_now
                        .ok_or(LicensingError::SecureStoreCorrupt)
                        .and_then(|trusted_now| {
                            std::str::from_utf8(&token)
                                .map_err(|_| LicensingError::SecureStoreCorrupt)
                                .and_then(|token| verifier.verify_cached(token, trusted_now))
                                .map(|entitlement| (entitlement, trusted_now))
                        });
                    match restored {
                        Ok((entitlement, trusted_now)) => {
                            service
                                .guard
                                .replace_state(crate::entitlement_guard::state_at(
                                    entitlement,
                                    trusted_now,
                                    &service.client_build,
                                ))
                        }
                        Err(error) => service
                            .guard
                            .replace_state(cached_entitlement_error_state(error)),
                    }
                }
                Ok(None) => {}
                Err(_) => service
                    .guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::CorruptSecureStore,
                    }),
            }
            match get_json::<LicenseDenialMarker>(store.as_ref(), SecretKey::LicenseDenial) {
                Ok(Some(marker)) => {
                    let entitlement = service.state().entitlement().cloned();
                    service
                        .guard
                        .replace_state(EntitlementState::LicenseInactive {
                            reason: marker.reason,
                            entitlement,
                        });
                }
                Ok(None) => {}
                Err(_) => service
                    .guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::CorruptSecureStore,
                    }),
            }
            match get_json::<RevalidationMarker>(store.as_ref(), SecretKey::RevalidationMarker) {
                Ok(Some(marker)) => {
                    service
                        .guard
                        .replace_state(EntitlementState::RevalidationRequired {
                            reason: marker.reason,
                        })
                }
                Ok(None) => {}
                Err(_) => service
                    .guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::CorruptSecureStore,
                    }),
            }
            match get_json::<ClientUpgradeMarker>(store.as_ref(), SecretKey::ClientUpgradeMarker) {
                Ok(Some(marker)) => {
                    if crate::ClientBuildIdentity::parse(&marker.blocked_build_version).is_err() {
                        service
                            .guard
                            .replace_state(EntitlementState::RevalidationRequired {
                                reason: crate::RevalidationReason::CorruptSecureStore,
                            });
                        *service
                            .verifier
                            .write()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verifier);
                        return service;
                    }
                    let evaluated_at = trusted_now.unwrap_or(local_unix).max(marker.observed_at);
                    match crate::evaluate_client_version(
                        &service.client_build,
                        &marker.policy,
                        evaluated_at,
                    ) {
                        Ok(crate::ClientVersionDisposition::UpgradeRequired)
                            if !matches!(
                                service.state(),
                                EntitlementState::DeviceDenied { .. }
                                    | EntitlementState::LicenseInactive { .. }
                                    | EntitlementState::RevalidationRequired { .. }
                            ) =>
                        {
                            let entitlement = service.state().entitlement().cloned();
                            service
                                .guard
                                .replace_state(EntitlementState::ClientUpgradeRequired {
                                    policy: marker.policy,
                                    entitlement,
                                });
                        }
                        Ok(crate::ClientVersionDisposition::UpgradeRequired) => {}
                        Ok(_) => {
                            let checkpoint =
                                service.trusted_time.checkpoint_authenticated_lower_bound(
                                    marker.observed_at,
                                    marker.policy.enforce_after,
                                    local_unix,
                                );
                            let checkpoint_failed = match checkpoint {
                                Ok(trusted_floor) => {
                                    service.guard.apply_time_policy(trusted_floor);
                                    false
                                }
                                Err(_) => true,
                            };
                            if checkpoint_failed
                                || store.delete_secret(SecretKey::ClientUpgradeMarker).is_err()
                            {
                                service.guard.replace_state(
                                    EntitlementState::RevalidationRequired {
                                        reason: crate::RevalidationReason::CorruptSecureStore,
                                    },
                                );
                            }
                        }
                        Err(_) => {
                            service
                                .guard
                                .replace_state(EntitlementState::RevalidationRequired {
                                    reason: crate::RevalidationReason::CorruptSecureStore,
                                })
                        }
                    }
                }
                Ok(None) => {}
                Err(_) => service
                    .guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::CorruptSecureStore,
                    }),
            }
            if matches!(service.state(), EntitlementState::Unauthenticated) {
                match load_refresh_session(store.clone()) {
                    Ok(_) => service
                        .guard
                        .replace_state(EntitlementState::ActivationPending),
                    Err(LicensingError::AuthorizationRequired) => {}
                    Err(_) => service
                        .guard
                        .replace_state(EntitlementState::RevalidationRequired {
                            reason: crate::RevalidationReason::CorruptSecureStore,
                        }),
                }
            }
        }
        *service
            .verifier
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verifier);
        service
    }

    pub fn guard(&self) -> &EntitlementGuard {
        &self.guard
    }

    pub fn state(&self) -> EntitlementState {
        self.guard.current_entitlement_state()
    }

    /// Loads the locally registered device without exposing its private key.
    /// Secure-store access shares the credential operation gate so UI reads
    /// cannot observe or overtake a concurrent registration/reset transaction.
    pub async fn device_registration_metadata(
        &self,
    ) -> Result<Option<crate::DeviceRegistrationMetadata>> {
        let identity_provider = self.identity_provider();
        self.credential_operations
            .run("device_registration_load", move || {
                identity_provider
                    .load()
                    .map(|identity| identity.map(|identity| identity.metadata))
            })
            .await
    }

    pub fn state_at(&self, local_unix: i64) -> EntitlementState {
        if let Err(error) = self.refresh_time_policy(local_unix)
            && !matches!(error, LicensingError::ClockRollback)
        {
            tracing::warn!(%error, "license state evaluation failed closed");
            self.guard
                .replace_state(EntitlementState::RevalidationRequired {
                    reason: crate::RevalidationReason::CorruptSecureStore,
                });
        }
        self.state()
    }

    /// Returns the anti-rollback time used for signed license scheduling.
    /// Before the first authenticated online observation there is no cached
    /// entitlement to protect, so the local clock is the only available hint.
    pub fn trusted_now(&self, local_unix: i64) -> Result<i64> {
        match self.trusted_time.observe(local_unix)? {
            Some(observation) if observation.rollback_detected => {
                Err(LicensingError::ClockRollback)
            }
            Some(observation) => Ok(observation.unix),
            None => Ok(local_unix),
        }
    }

    pub fn authorize(&self, operation: crate::RestrictedOperation, local_unix: i64) -> Result<()> {
        self.refresh_time_policy(local_unix)?;
        self.guard.authorize_operation(operation)
    }

    pub fn reserve_limit(
        &self,
        limit: crate::NumericLimit,
        current_count: u64,
        requested_count: u64,
        local_unix: i64,
    ) -> Result<crate::LimitReservation> {
        self.refresh_time_policy(local_unix)?;
        self.guard
            .reserve_limit(limit, current_count, requested_count)
    }

    pub async fn install_entitlement(
        &self,
        compact_jws: &str,
        server_unix: i64,
        local_unix: i64,
    ) -> Result<VerifiedEntitlement> {
        self.install_entitlement_after_online_refresh(
            Zeroizing::new(compact_jws.to_owned()),
            server_unix,
            local_unix,
        )
        .await
    }

    async fn verify_entitlement_after_online_refresh(
        &self,
        compact_jws: &str,
        server_unix: i64,
        local_unix: i64,
    ) -> Result<VerifiedEntitlement> {
        if self
            .verifier
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_none()
        {
            let store = self.store.clone();
            let metadata = self
                .credential_operations
                .run(
                    "device_registration_load_for_entitlement_verifier",
                    move || {
                        get_json::<crate::DeviceRegistrationMetadata>(
                            store.as_ref(),
                            SecretKey::DeviceRegistration,
                        )?
                        .ok_or(LicensingError::DeviceIdentityUnavailable)
                    },
                )
                .await?;
            self.ensure_verifier_for_metadata(&metadata);
        }

        let trusted_time = self.trusted_time.clone();
        let trusted = self
            .credential_operations
            .run(
                "trusted_time_observe_before_entitlement_install",
                move || trusted_time.observe(local_unix),
            )
            .await?;
        let trusted_now = match trusted {
            Some(observation) if observation.rollback_detected => {
                self.guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::ClockRollback,
                    });
                return Err(LicensingError::ClockRollback);
            }
            Some(observation) => observation.unix,
            None => local_unix,
        };

        let entitlement = self
            .verifier
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .ok_or(LicensingError::DeviceIdentityUnavailable)?
            .verify(compact_jws, trusted_now)
            .map_err(|error| {
                tracing::warn!(
                    %error,
                    trusted_now,
                    "license entitlement verification failed"
                );
                error
            })?;
        if server_unix.abs_diff(entitlement.claims.issued_at) > 5 * 60 {
            tracing::warn!(
                server_unix,
                issued_at = entitlement.claims.issued_at,
                "license entitlement server time check failed"
            );
            return Err(LicensingError::InvalidServerResponse);
        }
        tracing::info!(
            license_id = %entitlement.claims.license_id,
            device_id = %entitlement.claims.device_id,
            key_id = %entitlement.key_id,
            "license entitlement verified"
        );
        Ok(entitlement)
    }

    async fn verify_activation_proof_after_online_refresh(
        &self,
        metadata: &crate::DeviceRegistrationMetadata,
        compact_jws: &str,
        server_unix: i64,
        local_unix: i64,
    ) -> Result<VerifiedActivationProof> {
        let trusted_time = self.trusted_time.clone();
        let trusted = self
            .credential_operations
            .run("trusted_time_observe_before_activation_verify", move || {
                trusted_time.observe(local_unix)
            })
            .await?;
        let trusted_now = match trusted {
            Some(observation) if observation.rollback_detected => {
                self.guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::ClockRollback,
                    });
                return Err(LicensingError::ClockRollback);
            }
            Some(observation) => observation.unix,
            None => local_unix,
        };
        let verifier = self.activation_verifier_for(metadata);
        let proof = verifier.verify(compact_jws, trusted_now).map_err(|error| {
            tracing::warn!(
                %error,
                trusted_now,
                "license activation proof verification failed"
            );
            error
        })?;
        if server_unix.abs_diff(proof.claims.issued_at) > 5 * 60 {
            tracing::warn!(
                server_unix,
                issued_at = proof.claims.issued_at,
                "license activation proof server time check failed"
            );
            return Err(LicensingError::InvalidServerResponse);
        }
        tracing::info!(
            license_id = %proof.claims.license_id,
            device_id = %proof.claims.device_id,
            key_id = %proof.key_id,
            "license activation proof verified"
        );
        Ok(proof)
    }

    async fn install_verified_entitlement_after_online_refresh(
        &self,
        compact_jws: Zeroizing<String>,
        entitlement: VerifiedEntitlement,
        server_unix: i64,
        local_unix: i64,
    ) -> Result<VerifiedEntitlement> {
        let store = self.store.clone();
        let token_id = entitlement.claims.token_id.clone();
        let entitlement_lease = compact_jws.as_bytes().to_vec();
        let issued_at = entitlement.claims.issued_at;
        let trusted_time = self.trusted_time.clone();
        self.credential_operations
            .run("entitlement_store_after_refresh", move || {
                let entitlement_lease = Zeroizing::new(entitlement_lease);
                trusted_time.record_server_time(issued_at, local_unix)?;
                store.put_secret(SecretKey::EntitlementLease, entitlement_lease.as_slice())?;
                put_json(
                    store.as_ref(),
                    SecretKey::RefreshMetadata,
                    &RefreshMetadata {
                        refreshed_at: server_unix,
                        token_id,
                    },
                )?;
                store
                    .delete_secret(SecretKey::LicenseDenial)
                    .and_then(|_| store.delete_secret(SecretKey::RevalidationMarker))
                    .and_then(|_| store.delete_secret(SecretKey::ClientUpgradeMarker))
                    .and_then(|_| store.delete_secret(SecretKey::AuthorizationBlock))
            })
            .await?;

        self.guard.replace_state(EntitlementState::Active {
            entitlement: entitlement.clone(),
        });
        self.audit.record(AuditEvent {
            event_type: AuditEventType::EntitlementRefreshed,
            outcome: AuditOutcome::Succeeded,
            occurred_at: server_unix,
            device_id: Some(entitlement.claims.device_id.clone()),
            license_id: Some(entitlement.claims.license_id.clone()),
            reason_code: None,
        });
        Ok(entitlement)
    }

    async fn install_entitlement_after_online_refresh(
        &self,
        compact_jws: Zeroizing<String>,
        server_unix: i64,
        local_unix: i64,
    ) -> Result<VerifiedEntitlement> {
        let entitlement = self
            .verify_entitlement_after_online_refresh(&compact_jws, server_unix, local_unix)
            .await?;
        self.install_verified_entitlement_after_online_refresh(
            compact_jws,
            entitlement,
            server_unix,
            local_unix,
        )
        .await
    }

    pub async fn register_device_with_api(
        &self,
        api: &dyn LicenseApi,
        request: DeviceRegistrationFlow,
    ) -> Result<crate::DeviceRegistrationMetadata> {
        if self.store.mode() != SecureStoreMode::Persistent {
            return Err(LicensingError::SecureStoreUnavailable);
        }
        let DeviceRegistrationFlow {
            authorization_code,
            pkce_verifier,
            redirect_uri,
            platform,
            display_name,
            local_unix: _,
        } = request;
        tracing::info!(%platform, "license device registration flow started");
        let identity_provider = self.identity_provider();
        let app_version = self.client_build.wire_version().to_owned();
        let identity = self
            .credential_operations
            .run("device_identity_load_or_create", move || {
                identity_provider.load_or_create(platform, app_version, display_name)
            })
            .await?;
        tracing::info!(
            device_id = %identity.metadata.device_id,
            "license device identity ready"
        );
        self.ensure_verifier_for_metadata(&identity.metadata);
        let response = api
            .register_device(crate::DeviceRegistrationRequest {
                device: identity.metadata.clone(),
                authorization_code,
                pkce_verifier,
                redirect_uri,
            })
            .await?;
        tracing::info!(device_state = ?response.device_state, "license device registration response accepted");
        let server_unix = response.server_unix;
        let device_state = response.device_state;
        let refresh_session = response.refresh_session.expose().as_bytes().to_vec();
        let store = self.store.clone();
        self.credential_operations
            .run("refresh_session_store_after_registration", move || {
                let refresh_session = Zeroizing::new(refresh_session);
                store_refresh_session(store.clone(), refresh_session.as_slice())?;
                store.delete_secret(SecretKey::AuthorizationBlock)
            })
            .await?;
        match device_state {
            DeviceState::Active | DeviceState::PendingActivation => self
                .guard
                .replace_state(EntitlementState::ActivationPending),
            denied => {
                self.apply_device_state_persisted(denied, server_unix)
                    .await?;
                return Err(device_state_error(denied));
            }
        }
        tracing::debug!("license refresh session stored after registration");
        tracing::info!("license device registration flow completed");
        Ok(identity.metadata)
    }

    pub async fn verify_activation_proof_with_api(
        &self,
        api: &dyn LicenseApi,
        requested_scope: impl Into<String>,
        local_unix: i64,
    ) -> Result<ActivationProofVerification> {
        tracing::info!("license activation proof verification flow started");
        let identity_provider = self.identity_provider();
        let identity = self
            .credential_operations
            .run("device_identity_load", move || identity_provider.load())
            .await?
            .ok_or(LicensingError::DeviceIdentityUnavailable)?;
        let store = self.store.clone();
        let session = self
            .credential_operations
            .run("refresh_session_load", move || load_refresh_session(store))
            .await?;
        let challenge = api
            .issue_challenge(
                &session,
                crate::ChallengeRequest {
                    device_id: identity.metadata.device_id.clone(),
                    requested_scope: requested_scope.into(),
                },
            )
            .await?;
        tracing::debug!("license activation challenge received");
        let challenge_now = self.challenge_validation_time(local_unix).await?;
        let proof =
            identity.sign_challenge(&challenge, challenge_now, self.client_build.wire_version())?;
        tracing::debug!("license activation challenge signed");
        let response = api
            .verify_activation(&session, crate::ActivationVerificationRequest { proof })
            .await?;
        tracing::info!(device_state = ?response.device_state, "license activation verification response accepted");
        if !matches!(
            response.device_state,
            DeviceState::Active | DeviceState::PendingActivation
        ) {
            self.apply_device_state_persisted(response.device_state, response.server_unix)
                .await?;
            return Err(device_state_error(response.device_state));
        }
        let activation = Zeroizing::new(response.activation.expose().to_owned());
        let verified = self
            .verify_activation_proof_after_online_refresh(
                &identity.metadata,
                &activation,
                response.server_unix,
                local_unix,
            )
            .await?;
        let trusted_time = self.trusted_time.clone();
        let signed_issued_at = verified.claims.issued_at;
        self.credential_operations
            .run("trusted_time_record_after_activation_proof", move || {
                trusted_time.record_server_time(signed_issued_at, local_unix)
            })
            .await?;
        Ok(ActivationProofVerification {
            proof: verified,
            rotated_refresh_session: response.rotated_refresh_session,
            device_state: response.device_state,
        })
    }

    pub async fn confirm_activation_with_api(
        &self,
        api: &dyn LicenseApi,
        session: crate::RefreshSession,
        _local_unix: i64,
    ) -> Result<()> {
        let response = api.confirm_activation(&session).await?;
        tracing::info!(device_state = ?response.device_state, "license activation confirmation response accepted");
        let refresh_session = session.expose().as_bytes().to_vec();
        if response.device_state != DeviceState::Active {
            self.apply_device_state_persisted(response.device_state, response.server_unix)
                .await?;
            return Err(device_state_error(response.device_state));
        }
        let store = self.store.clone();
        self.credential_operations
            .run(
                "refresh_session_store_after_activation_confirmation",
                move || {
                    let refresh_session = Zeroizing::new(refresh_session);
                    store_refresh_session(store, refresh_session.as_slice())
                },
            )
            .await?;
        Ok(())
    }

    /// Resumes every post-registration activation stage from persisted device credentials.
    /// This makes the browser authorization code a one-way handoff: once registration succeeds,
    /// crashes and ambiguous network failures no longer require the code to be redeemed again.
    pub async fn complete_pending_activation_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<VerifiedEntitlement> {
        let verification = self
            .verify_activation_proof_with_api(api, "activation:verify", local_unix)
            .await?;
        let confirmation_session = verification.rotated_refresh_session;
        self.rotate_refresh_session(confirmation_session.expose().as_bytes())
            .await?;
        self.confirm_activation_with_api(api, confirmation_session, local_unix)
            .await?;
        self.refresh_entitlement_with_api(api, "entitlement:refresh", local_unix)
            .await
    }

    pub async fn refresh_entitlement_with_api(
        &self,
        api: &dyn LicenseApi,
        requested_scope: impl Into<String>,
        local_unix: i64,
    ) -> Result<VerifiedEntitlement> {
        tracing::info!("license entitlement refresh flow started");
        let identity_provider = self.identity_provider();
        let identity = self
            .credential_operations
            .run("device_identity_load", move || identity_provider.load())
            .await?
            .ok_or(LicensingError::DeviceIdentityUnavailable)?;
        let store = self.store.clone();
        let session = self
            .credential_operations
            .run("refresh_session_load", move || load_refresh_session(store))
            .await?;
        let challenge = api
            .issue_challenge(
                &session,
                crate::ChallengeRequest {
                    device_id: identity.metadata.device_id.clone(),
                    requested_scope: requested_scope.into(),
                },
            )
            .await?;
        tracing::debug!("license entitlement challenge received");
        let challenge_now = self.challenge_validation_time(local_unix).await?;
        let proof =
            identity.sign_challenge(&challenge, challenge_now, self.client_build.wire_version())?;
        tracing::debug!("license entitlement challenge signed");
        let response = api
            .refresh_entitlement(&session, crate::EntitlementRefreshRequest { proof })
            .await?;
        tracing::info!(device_state = ?response.device_state, "license entitlement refresh response accepted");
        self.apply_device_state_persisted(response.device_state, response.server_unix)
            .await?;
        if response.device_state != DeviceState::Active {
            return Err(device_state_error(response.device_state));
        }
        let rotated_refresh_session = response
            .rotated_refresh_session
            .expose()
            .as_bytes()
            .to_vec();
        let store = self.store.clone();
        self.credential_operations
            .run("refresh_session_store_after_refresh", move || {
                let rotated_refresh_session = Zeroizing::new(rotated_refresh_session);
                store_refresh_session(store, rotated_refresh_session.as_slice())
            })
            .await?;
        tracing::debug!("rotated license refresh session stored");
        let entitlement = Zeroizing::new(response.entitlement.expose().to_owned());
        let installed = self
            .install_entitlement_after_online_refresh(entitlement, response.server_unix, local_unix)
            .await?;
        tracing::info!("license entitlement refresh flow completed");
        Ok(installed)
    }

    pub async fn entitlement_status_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<crate::EntitlementStatusResponse> {
        self.refresh_time_policy(local_unix)?;
        let identity_provider = self.identity_provider();
        let identity = self
            .credential_operations
            .run("device_identity_load", move || identity_provider.load())
            .await?
            .ok_or(LicensingError::DeviceIdentityUnavailable)?;
        let session = self.refresh_session().await?;
        let challenge = api
            .issue_challenge(
                &session,
                crate::ChallengeRequest {
                    device_id: identity.metadata.device_id.clone(),
                    requested_scope: "entitlement:status".to_owned(),
                },
            )
            .await?;
        let challenge_now = self.challenge_validation_time(local_unix).await?;
        let proof =
            identity.sign_challenge(&challenge, challenge_now, self.client_build.wire_version())?;
        let response = api
            .entitlement_status(&session, crate::EntitlementStatusRequest { proof })
            .await?;
        self.apply_device_state_persisted(response.device_state, challenge_now)
            .await?;
        self.apply_minimum_license_epoch(response.license_epoch, challenge_now)
            .await?;
        Ok(response)
    }

    pub async fn recover_session_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<()> {
        tracing::info!("license session recovery flow started");
        let identity_provider = self.identity_provider();
        let identity = self
            .credential_operations
            .run("device_identity_load", move || identity_provider.load())
            .await?
            .ok_or(LicensingError::DeviceIdentityUnavailable)?;
        let challenge = api
            .issue_session_recovery_challenge(crate::SessionRecoveryChallengeRequest {
                device_id: identity.metadata.device_id.clone(),
            })
            .await?;
        let challenge_now = self.challenge_validation_time(local_unix).await?;
        let proof =
            identity.sign_challenge(&challenge, challenge_now, self.client_build.wire_version())?;
        let response = api
            .recover_session(crate::SessionRecoveryRequest { proof })
            .await?;
        if !matches!(
            response.device_state,
            DeviceState::Active | DeviceState::PendingActivation
        ) {
            self.apply_device_state_persisted(response.device_state, response.server_unix)
                .await?;
            return Err(device_state_error(response.device_state));
        }
        self.apply_device_state_persisted(response.device_state, response.server_unix)
            .await?;
        let refresh_session = response.refresh_session.expose().as_bytes().to_vec();
        let server_unix = response.server_unix;
        let store = self.store.clone();
        self.credential_operations
            .run("refresh_session_store_after_recovery", move || {
                let refresh_session = Zeroizing::new(refresh_session);
                store_refresh_session(store, refresh_session.as_slice())
            })
            .await?;
        self.audit.record(AuditEvent {
            event_type: AuditEventType::SessionRecovered,
            outcome: AuditOutcome::Succeeded,
            occurred_at: server_unix,
            device_id: Some(identity.metadata.device_id),
            license_id: None,
            reason_code: None,
        });
        tracing::info!("license session recovery flow completed");
        Ok(())
    }

    pub async fn list_devices_with_api(
        &self,
        api: &dyn LicenseApi,
        cursor: Option<&str>,
        page_size: u32,
        local_unix: i64,
    ) -> Result<crate::RegisteredDevicePage> {
        let (session, proof) = self
            .authenticated_device_request(api, DEVICE_LIST_SCOPE, local_unix)
            .await?;
        api.list_devices(&session, &proof, cursor, page_size).await
    }

    pub async fn billing_summary_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<crate::BillingSummary> {
        let (session, proof) = self
            .authenticated_device_request(api, BILLING_SUMMARY_SCOPE, local_unix)
            .await?;
        api.billing_summary(&session, &proof).await
    }

    pub async fn submit_customer_payment_with_api(
        &self,
        api: &dyn LicenseApi,
        submission: crate::CustomerPaymentSubmission,
        local_unix: i64,
    ) -> Result<crate::ManualPaymentClaim> {
        let (session, proof) = self
            .authenticated_device_request(api, BILLING_PAYMENT_SCOPE, local_unix)
            .await?;
        api.submit_customer_payment(&session, &proof, submission)
            .await
    }

    pub async fn team_profile_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<crate::TeamProfile> {
        let (session, proof) = self
            .authenticated_device_request(api, TEAM_PROFILE_SCOPE, local_unix)
            .await?;
        api.team_profile(&session, &proof).await
    }

    pub async fn team_members_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::TeamMemberPageRequest,
        local_unix: i64,
    ) -> Result<crate::TeamMemberPage> {
        let (session, proof) = self
            .authenticated_device_request(api, TEAM_MEMBERS_SCOPE, local_unix)
            .await?;
        api.team_members(&session, &proof, request).await
    }

    pub async fn create_team_invitation_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::CreateTeamInvitation,
        local_unix: i64,
    ) -> Result<crate::TeamInvitation> {
        let (session, proof) = self
            .authenticated_device_request(api, TEAM_INVITATION_CREATE_SCOPE, local_unix)
            .await?;
        api.create_team_invitation(&session, &proof, request).await
    }

    pub async fn accept_team_invitation_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::AcceptTeamInvitation,
        local_unix: i64,
    ) -> Result<crate::TeamProfile> {
        let (session, proof) = self
            .authenticated_device_request(api, TEAM_INVITATION_ACCEPT_SCOPE, local_unix)
            .await?;
        api.accept_team_invitation(&session, &proof, request).await
    }

    pub async fn update_team_member_with_api(
        &self,
        api: &dyn LicenseApi,
        member_id: &str,
        request: crate::UpdateWorkspaceMember,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMember> {
        let member_id = crate::license_api::canonical_resource_id(member_id, "member_")?;
        let scope = format!("team:member:update:{member_id}");
        let (session, proof) = self
            .authenticated_device_request(api, scope, local_unix)
            .await?;
        api.update_team_member(&session, &proof, &member_id, request)
            .await
    }

    pub async fn create_team_device_enrollment_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::TeamOperationRequest,
        local_unix: i64,
    ) -> Result<crate::MemberDeviceEnrollment> {
        let (session, proof) = self
            .authenticated_device_request(api, TEAM_DEVICE_ENROLLMENT_CREATE_SCOPE, local_unix)
            .await?;
        api.create_team_device_enrollment(&session, &proof, request)
            .await
    }

    pub async fn create_team_member_device_enrollment_with_api(
        &self,
        api: &dyn LicenseApi,
        member_id: &str,
        request: crate::TeamOperationRequest,
        local_unix: i64,
    ) -> Result<crate::MemberDeviceEnrollment> {
        let member_id = crate::license_api::canonical_resource_id(member_id, "member_")?;
        let scope = format!("team:member:device-enrollment:create:{member_id}");
        let (session, proof) = self
            .authenticated_device_request(api, scope, local_unix)
            .await?;
        api.create_team_member_device_enrollment(&session, &proof, &member_id, request)
            .await
    }

    pub async fn accept_team_device_enrollment_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::AcceptMemberDeviceEnrollment,
        local_unix: i64,
    ) -> Result<crate::TeamProfile> {
        let (session, proof) = self
            .authenticated_device_request(api, TEAM_DEVICE_ENROLLMENT_ACCEPT_SCOPE, local_unix)
            .await?;
        api.accept_team_device_enrollment(&session, &proof, request)
            .await
    }

    pub async fn leave_team_workspace_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::LeaveWorkspace,
        local_unix: i64,
    ) -> Result<()> {
        let session = self.refresh_session().await?;
        if api
            .team_leave_operation_status(&session, &request)
            .await?
            .committed
        {
            return Ok(());
        }
        let (session, proof) = self
            .authenticated_device_request(api, TEAM_LEAVE_SCOPE, local_unix)
            .await?;
        api.leave_team_workspace(&session, &proof, request).await
    }

    pub async fn transfer_team_ownership_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::TransferWorkspaceOwnership,
        local_unix: i64,
    ) -> Result<crate::OwnershipTransferResult> {
        let member_id =
            crate::license_api::canonical_resource_id(&request.new_owner_member_id, "member_")?;
        let scope = format!("{TEAM_OWNERSHIP_TRANSFER_SCOPE}:{member_id}");
        let (session, proof) = self
            .authenticated_device_request(api, scope, local_unix)
            .await?;
        api.transfer_team_ownership(&session, &proof, request).await
    }

    pub async fn shared_configurations_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::SharedConfigurationPageRequest,
        local_unix: i64,
    ) -> Result<crate::SharedConfigurationPage> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_READ_SCOPE, local_unix)
            .await?;
        api.shared_configurations(&session, &proof, request).await
    }

    pub async fn shared_configuration_content_with_api(
        &self,
        api: &dyn LicenseApi,
        document_id: &str,
        request: crate::SharedConfigurationContentRequest,
        local_unix: i64,
    ) -> Result<crate::SharedConfigurationContent> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_READ_SCOPE, local_unix)
            .await?;
        api.shared_configuration_content(&session, &proof, document_id, request)
            .await
    }

    pub async fn create_shared_configuration_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::CreateSharedConfiguration,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_WRITE_SCOPE, local_unix)
            .await?;
        api.create_shared_configuration(&session, &proof, request)
            .await
    }

    pub async fn revise_shared_configuration_with_api(
        &self,
        api: &dyn LicenseApi,
        document_id: &str,
        request: crate::ReviseSharedConfiguration,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_WRITE_SCOPE, local_unix)
            .await?;
        api.revise_shared_configuration(&session, &proof, document_id, request)
            .await
    }

    pub async fn publish_shared_configuration_with_api(
        &self,
        api: &dyn LicenseApi,
        document_id: &str,
        request: crate::PublishSharedConfiguration,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_PUBLISH_SCOPE, local_unix)
            .await?;
        api.publish_shared_configuration(&session, &proof, document_id, request)
            .await
    }

    pub async fn delete_shared_configuration_with_api(
        &self,
        api: &dyn LicenseApi,
        document_id: &str,
        request: crate::VersionedWorkspaceMutation,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_WRITE_SCOPE, local_unix)
            .await?;
        api.delete_shared_configuration(&session, &proof, document_id, request)
            .await
    }

    pub async fn restore_shared_configuration_with_api(
        &self,
        api: &dyn LicenseApi,
        document_id: &str,
        request: crate::VersionedWorkspaceMutation,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_WRITE_SCOPE, local_unix)
            .await?;
        api.restore_shared_configuration(&session, &proof, document_id, request)
            .await
    }

    pub async fn purge_shared_configuration_with_api(
        &self,
        api: &dyn LicenseApi,
        document_id: &str,
        request: crate::VersionedWorkspaceMutation,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SHARED_PURGE_SCOPE, local_unix)
            .await?;
        api.purge_shared_configuration(&session, &proof, document_id, request)
            .await
    }

    pub async fn workspace_sync_feed_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::WorkspaceSyncFeedRequest,
        local_unix: i64,
    ) -> Result<crate::WorkspaceSyncFeed> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SYNC_READ_SCOPE, local_unix)
            .await?;
        api.workspace_sync_feed(&session, &proof, request).await
    }

    pub async fn workspace_checkpoint_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<Option<crate::WorkspaceDeviceCheckpoint>> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SYNC_READ_SCOPE, local_unix)
            .await?;
        api.workspace_checkpoint(&session, &proof).await
    }

    pub async fn advance_workspace_checkpoint_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::AdvanceWorkspaceCheckpoint,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_SYNC_WRITE_SCOPE, local_unix)
            .await?;
        api.advance_workspace_checkpoint(&session, &proof, request)
            .await
    }

    pub async fn workspace_alert_rules_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::WorkspaceAlertRulePageRequest,
        local_unix: i64,
    ) -> Result<crate::WorkspaceAlertRulePage> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_ALERTS_READ_SCOPE, local_unix)
            .await?;
        api.workspace_alert_rules(&session, &proof, request).await
    }

    pub async fn create_workspace_alert_rule_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::CreateWorkspaceAlertRule,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_ALERTS_MANAGE_SCOPE, local_unix)
            .await?;
        api.create_workspace_alert_rule(&session, &proof, request)
            .await
    }

    pub async fn update_workspace_alert_rule_with_api(
        &self,
        api: &dyn LicenseApi,
        rule_id: &str,
        request: crate::UpdateWorkspaceAlertRule,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_ALERTS_MANAGE_SCOPE, local_unix)
            .await?;
        api.update_workspace_alert_rule(&session, &proof, rule_id, request)
            .await
    }

    pub async fn delete_workspace_alert_rule_with_api(
        &self,
        api: &dyn LicenseApi,
        rule_id: &str,
        request: crate::VersionedWorkspaceMutation,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_ALERTS_MANAGE_SCOPE, local_unix)
            .await?;
        api.delete_workspace_alert_rule(&session, &proof, rule_id, request)
            .await
    }

    pub async fn workspace_alert_incidents_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::WorkspaceIncidentPageRequest,
        local_unix: i64,
    ) -> Result<crate::WorkspaceIncidentPage> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_ALERTS_READ_SCOPE, local_unix)
            .await?;
        api.workspace_alert_incidents(&session, &proof, request)
            .await
    }

    pub async fn acknowledge_workspace_alert_incident_with_api(
        &self,
        api: &dyn LicenseApi,
        incident_id: &str,
        request: crate::VersionedWorkspaceMutation,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_ALERTS_ACK_SCOPE, local_unix)
            .await?;
        api.acknowledge_workspace_alert_incident(&session, &proof, incident_id, request)
            .await
    }

    pub async fn resolve_workspace_alert_incident_with_api(
        &self,
        api: &dyn LicenseApi,
        incident_id: &str,
        request: crate::VersionedWorkspaceMutation,
        local_unix: i64,
    ) -> Result<crate::WorkspaceMutationReceipt> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_ALERTS_MANAGE_SCOPE, local_unix)
            .await?;
        api.resolve_workspace_alert_incident(&session, &proof, incident_id, request)
            .await
    }

    pub async fn workspace_audit_events_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::WorkspaceAuditPageRequest,
        local_unix: i64,
    ) -> Result<crate::WorkspaceAuditPage> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_AUDIT_READ_SCOPE, local_unix)
            .await?;
        api.workspace_audit_events(&session, &proof, request).await
    }

    pub async fn workspace_audit_event_types_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<crate::WorkspaceAuditEventTypes> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_AUDIT_READ_SCOPE, local_unix)
            .await?;
        api.workspace_audit_event_types(&session, &proof).await
    }

    pub async fn export_workspace_audit_events_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::WorkspaceAuditPageRequest,
        local_unix: i64,
    ) -> Result<crate::WorkspaceAuditExport> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_AUDIT_EXPORT_SCOPE, local_unix)
            .await?;
        api.export_workspace_audit_events(&session, &proof, request)
            .await
    }

    pub async fn workspace_webhook_endpoints_with_api(
        &self,
        api: &dyn LicenseApi,
        local_unix: i64,
    ) -> Result<Vec<crate::WorkspaceWebhookEndpoint>> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_WEBHOOK_LIST_SCOPE, local_unix)
            .await?;
        api.workspace_webhook_endpoints(&session, &proof).await
    }

    pub async fn create_workspace_webhook_endpoint_with_api(
        &self,
        api: &dyn LicenseApi,
        request: crate::CreateWorkspaceWebhookEndpoint,
        local_unix: i64,
    ) -> Result<crate::WorkspaceWebhookSecretResult> {
        let (session, proof) = self
            .authenticated_device_request(api, WORKSPACE_WEBHOOK_CREATE_SCOPE, local_unix)
            .await?;
        api.create_workspace_webhook_endpoint(&session, &proof, request)
            .await
    }

    pub async fn update_workspace_webhook_endpoint_with_api(
        &self,
        api: &dyn LicenseApi,
        endpoint_id: &str,
        request: crate::UpdateWorkspaceWebhookEndpoint,
        local_unix: i64,
    ) -> Result<crate::WorkspaceWebhookEndpoint> {
        let endpoint_id =
            crate::license_api::canonical_resource_id(endpoint_id, "webhook_endpoint_")?;
        let scope = format!("workspace:webhooks:update:{endpoint_id}");
        let (session, proof) = self
            .authenticated_device_request(api, scope, local_unix)
            .await?;
        api.update_workspace_webhook_endpoint(&session, &proof, &endpoint_id, request)
            .await
    }

    pub async fn rotate_workspace_webhook_secret_with_api(
        &self,
        api: &dyn LicenseApi,
        endpoint_id: &str,
        request: crate::RotateWorkspaceWebhookSecret,
        local_unix: i64,
    ) -> Result<crate::WorkspaceWebhookSecretResult> {
        let endpoint_id =
            crate::license_api::canonical_resource_id(endpoint_id, "webhook_endpoint_")?;
        let scope = format!("workspace:webhooks:rotate:{endpoint_id}");
        let (session, proof) = self
            .authenticated_device_request(api, scope, local_unix)
            .await?;
        api.rotate_workspace_webhook_secret(&session, &proof, &endpoint_id, request)
            .await
    }

    pub async fn delete_workspace_webhook_endpoint_with_api(
        &self,
        api: &dyn LicenseApi,
        endpoint_id: &str,
        request: crate::DeleteWorkspaceWebhookEndpoint,
        local_unix: i64,
    ) -> Result<crate::WorkspaceWebhookDeletion> {
        let endpoint_id =
            crate::license_api::canonical_resource_id(endpoint_id, "webhook_endpoint_")?;
        let scope = format!("workspace:webhooks:delete:{endpoint_id}");
        let (session, proof) = self
            .authenticated_device_request(api, scope, local_unix)
            .await?;
        api.delete_workspace_webhook_endpoint(&session, &proof, &endpoint_id, request)
            .await
    }

    pub async fn workspace_webhook_deliveries_with_api(
        &self,
        api: &dyn LicenseApi,
        endpoint_id: Option<&str>,
        limit: u16,
        local_unix: i64,
    ) -> Result<Vec<crate::WorkspaceWebhookDelivery>> {
        let endpoint_id = endpoint_id
            .map(|value| crate::license_api::canonical_resource_id(value, "webhook_endpoint_"))
            .transpose()?;
        let scope = endpoint_id.as_ref().map_or_else(
            || WORKSPACE_WEBHOOK_DELIVERIES_SCOPE.to_owned(),
            |endpoint_id| format!("{WORKSPACE_WEBHOOK_DELIVERIES_SCOPE}:{endpoint_id}"),
        );
        let (session, proof) = self
            .authenticated_device_request(api, scope, local_unix)
            .await?;
        api.workspace_webhook_deliveries(&session, &proof, endpoint_id.as_deref(), limit)
            .await
    }

    async fn authenticated_device_request(
        &self,
        api: &dyn LicenseApi,
        scope: impl Into<String>,
        local_unix: i64,
    ) -> Result<(crate::RefreshSession, crate::DeviceProof)> {
        self.refresh_time_policy(local_unix)?;
        let identity_provider = self.identity_provider();
        let identity = self
            .credential_operations
            .run("device_identity_load", move || identity_provider.load())
            .await?
            .ok_or(LicensingError::DeviceIdentityUnavailable)?;
        let session = self.refresh_session().await?;
        let challenge = api
            .issue_challenge(
                &session,
                crate::ChallengeRequest {
                    device_id: identity.metadata.device_id.clone(),
                    requested_scope: scope.into(),
                },
            )
            .await?;
        let challenge_now = self.challenge_validation_time(local_unix).await?;
        let proof =
            identity.sign_challenge(&challenge, challenge_now, self.client_build.wire_version())?;
        Ok((session, proof))
    }

    pub async fn remove_device_with_api(
        &self,
        api: &dyn LicenseApi,
        device_id: &str,
        operation_id: &str,
        local_unix: i64,
    ) -> Result<()> {
        let store = self.store.clone();
        let session = self
            .credential_operations
            .run("refresh_session_load", move || load_refresh_session(store))
            .await?;
        self.remove_device_with_session_and_api(api, session, device_id, operation_id, local_unix)
            .await
    }

    pub async fn remove_device_with_session_and_api(
        &self,
        api: &dyn LicenseApi,
        session: crate::RefreshSession,
        device_id: &str,
        operation_id: &str,
        local_unix: i64,
    ) -> Result<()> {
        if api
            .device_removal_status(&session, device_id, operation_id)
            .await?
            .committed
        {
            return Ok(());
        }
        let identity_provider = self.identity_provider();
        let identity = self
            .credential_operations
            .run("device_identity_load", move || identity_provider.load())
            .await?
            .ok_or(LicensingError::DeviceIdentityUnavailable)?;
        let challenge = api
            .issue_challenge(
                &session,
                crate::ChallengeRequest {
                    device_id: identity.metadata.device_id.clone(),
                    requested_scope: format!("device:remove:{device_id}"),
                },
            )
            .await?;
        let challenge_now = self.challenge_validation_time(local_unix).await?;
        let proof =
            identity.sign_challenge(&challenge, challenge_now, self.client_build.wire_version())?;
        api.remove_device(
            &session,
            device_id,
            crate::DeviceRemovalRequest {
                proof,
                operation_id: operation_id.to_owned(),
            },
        )
        .await
    }

    pub async fn logout_session_with_api(
        &self,
        api: &dyn LicenseApi,
        session: crate::RefreshSession,
    ) -> Result<()> {
        api.logout(session).await
    }

    /// Persists the user's local sign-out decision before any process stop or
    /// network operation, so a crash cannot restore a cached entitlement.
    pub async fn deauthorize_locally(
        &self,
        occurred_at: i64,
    ) -> Result<Option<crate::RefreshSession>> {
        self.invalidate_local_access();
        let store = self.store.clone();
        self.credential_operations
            .run("local_deauthorization", move || {
                let session = match load_refresh_session(store.clone()) {
                    Ok(session) => Some(session),
                    Err(LicensingError::AuthorizationRequired) => None,
                    Err(error) => return Err(error),
                };
                let marker_result = put_json(
                    store.as_ref(),
                    SecretKey::AuthorizationBlock,
                    &AuthorizationBlockMarker::LocalDeauthorization {
                        observed_at: occurred_at,
                    },
                );
                let clear_result = clear_persisted_session(store.as_ref());
                marker_result.and(clear_result)?;
                Ok(session)
            })
            .await
    }

    /// Returns the current validation clock without trusting or persisting a
    /// challenge's unsigned `issuedAt`. Only a successfully verified signed
    /// activation proof or entitlement may advance the durable online anchor.
    async fn challenge_validation_time(&self, local_unix: i64) -> Result<i64> {
        let trusted_time = self.trusted_time.clone();
        self.credential_operations
            .run(
                "trusted_time_observe_before_challenge",
                move || match trusted_time.observe(local_unix)? {
                    Some(observation) if observation.rollback_detected => {
                        Err(LicensingError::ClockRollback)
                    }
                    Some(observation) => Ok(observation.unix),
                    None if local_unix > 0 => Ok(local_unix),
                    None => Err(LicensingError::InvalidServerResponse),
                },
            )
            .await
    }

    pub async fn rotate_refresh_session(&self, new_session: &[u8]) -> Result<()> {
        let store = self.store.clone();
        let new_session = Zeroizing::new(new_session.to_vec());
        self.credential_operations
            .run("refresh_session_store", move || {
                store_refresh_session(store, new_session.as_slice())
            })
            .await
    }

    pub fn apply_device_state(&self, state: DeviceState) {
        match state {
            DeviceState::Active => {}
            DeviceState::PendingActivation => {
                self.guard
                    .replace_state(if self.store.mode() == SecureStoreMode::SessionOnly {
                        EntitlementState::SessionOnly
                    } else {
                        EntitlementState::ActivationPending
                    });
            }
            DeviceState::Removed | DeviceState::Revoked | DeviceState::Suspicious => {
                self.guard
                    .replace_state(EntitlementState::DeviceDenied { state });
            }
        }
    }

    pub async fn apply_device_state_persisted(
        &self,
        state: DeviceState,
        observed_at: i64,
    ) -> Result<()> {
        self.apply_device_state(state);
        if self.store.mode() == SecureStoreMode::SessionOnly {
            return Ok(());
        }
        let store = self.store.clone();
        let result = self
            .credential_operations
            .run("device_state_store", move || match state {
                DeviceState::Active | DeviceState::PendingActivation => {
                    store.delete_secret(SecretKey::AuthorizationBlock)
                }
                DeviceState::Removed | DeviceState::Revoked | DeviceState::Suspicious => {
                    let marker_result = put_json(
                        store.as_ref(),
                        SecretKey::AuthorizationBlock,
                        &AuthorizationBlockMarker::DeviceDenied { state, observed_at },
                    );
                    let clear_result = clear_cached_authorization_material(store.as_ref());
                    marker_result.and(clear_result)
                }
            })
            .await;
        self.clear_cached_authorization_material_after_marker_failure(result)
            .await
    }

    pub async fn apply_license_inactive(
        &self,
        reason: crate::LicenseInactiveReason,
        observed_at: i64,
    ) -> Result<()> {
        let entitlement = self.state().entitlement().cloned();
        self.guard.replace_state(EntitlementState::LicenseInactive {
            reason,
            entitlement,
        });
        let store = self.store.clone();
        let result = self
            .credential_operations
            .run("license_denial_store", move || {
                put_json(
                    store.as_ref(),
                    SecretKey::LicenseDenial,
                    &LicenseDenialMarker {
                        reason,
                        observed_at,
                    },
                )
            })
            .await;
        self.clear_cached_authorization_material_after_marker_failure(result)
            .await
    }

    pub async fn apply_revalidation_required(
        &self,
        reason: crate::RevalidationReason,
        observed_at: i64,
    ) -> Result<()> {
        self.guard
            .replace_state(EntitlementState::RevalidationRequired { reason });
        let store = self.store.clone();
        let result = self
            .credential_operations
            .run("revalidation_marker_store", move || {
                put_json(
                    store.as_ref(),
                    SecretKey::RevalidationMarker,
                    &RevalidationMarker {
                        reason,
                        observed_at,
                    },
                )
            })
            .await;
        self.clear_cached_authorization_material_after_marker_failure(result)
            .await
    }

    pub async fn apply_client_upgrade_required(
        &self,
        policy: crate::ClientVersionPolicy,
        local_observed_at: i64,
    ) -> Result<()> {
        let (minimum, _) = crate::validate_client_version_policy(&policy)?;
        if self.client_build.version() >= &minimum {
            return Err(LicensingError::InvalidServerResponse);
        }
        // A 426 is an authenticated online policy decision. A slow or rolled-back local wall
        // clock must not turn it into a pre-enforcement warning.
        let observed_at = local_observed_at.max(policy.enforce_after);
        let entitlement = self.state().entitlement().cloned();
        let device_id = entitlement
            .as_ref()
            .map(|entitlement| entitlement.claims.device_id.clone());
        let license_id = entitlement
            .as_ref()
            .map(|entitlement| entitlement.claims.license_id.clone());
        self.guard
            .replace_state(EntitlementState::ClientUpgradeRequired {
                policy: policy.clone(),
                entitlement,
            });
        self.audit.record(AuditEvent {
            event_type: AuditEventType::ClientUpgradeRequired,
            outcome: AuditOutcome::Denied,
            occurred_at: observed_at,
            device_id,
            license_id,
            reason_code: Some(format!("minimum_client_version:{}", policy.minimum_version)),
        });
        if self.store.mode() == SecureStoreMode::SessionOnly {
            return Ok(());
        }
        let blocked_build_version = self.client_build.wire_version().to_owned();
        let store = self.store.clone();
        let result = self
            .credential_operations
            .run("client_upgrade_marker_store", move || {
                put_json(
                    store.as_ref(),
                    SecretKey::ClientUpgradeMarker,
                    &ClientUpgradeMarker {
                        blocked_build_version,
                        policy,
                        observed_at,
                    },
                )?;
                store.delete_secret(SecretKey::RevalidationMarker)
            })
            .await;
        self.clear_cached_authorization_material_after_marker_failure(result)
            .await
    }

    pub async fn apply_minimum_license_epoch(
        &self,
        minimum_epoch: u64,
        observed_at: i64,
    ) -> Result<()> {
        if self
            .state()
            .entitlement()
            .is_some_and(|entitlement| entitlement.claims.license_epoch < minimum_epoch)
        {
            self.apply_revalidation_required(crate::RevalidationReason::ObsoleteEpoch, observed_at)
                .await?;
        }
        Ok(())
    }

    pub async fn clear_session(&self) -> Result<()> {
        self.invalidate_local_access();
        let store = self.store.clone();
        self.credential_operations
            .run("license_session_clear", move || {
                clear_persisted_session(store.as_ref())
            })
            .await
    }

    fn invalidate_local_access(&self) {
        self.guard.replace_state(self.empty_state());
    }

    /// Destroys the local device key and all license material so a subsequent
    /// activation is cryptographically independent from the previous license.
    pub async fn reset_device_identity(&self, occurred_at: i64) -> Result<()> {
        self.invalidate_local_access();
        *self
            .verifier
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;

        let store = self.store.clone();
        let identity_provider = self.identity_provider();
        let previous_device_id = self
            .credential_operations
            .run("device_identity_reset", move || {
                let previous_device_id = get_json::<crate::DeviceRegistrationMetadata>(
                    store.as_ref(),
                    SecretKey::DeviceRegistration,
                )
                .ok()
                .flatten()
                .map(|metadata| metadata.device_id);
                let session_result = clear_persisted_session(store.as_ref());
                let identity_result = identity_provider.reset_identity();
                let block_result = store.delete_secret(SecretKey::AuthorizationBlock);
                session_result.and(identity_result).and(block_result)?;
                Ok(previous_device_id)
            })
            .await?;
        self.audit.record(AuditEvent {
            event_type: AuditEventType::DeviceIdentityReset,
            outcome: AuditOutcome::Succeeded,
            occurred_at,
            device_id: previous_device_id,
            license_id: None,
            reason_code: Some("user_requested_license_switch".into()),
        });
        Ok(())
    }

    fn identity_provider(&self) -> DeviceIdentityProvider {
        DeviceIdentityProvider::new(self.store.clone())
    }

    fn empty_state(&self) -> EntitlementState {
        if self.store.mode() == SecureStoreMode::SessionOnly {
            EntitlementState::SessionOnly
        } else {
            EntitlementState::Unauthenticated
        }
    }

    fn ensure_verifier_for_metadata(&self, metadata: &crate::DeviceRegistrationMetadata) {
        let mut verifier = self
            .verifier
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if verifier.is_none() {
            *verifier = Some(self.verifier_for(metadata));
        }
    }

    fn verifier_for(&self, metadata: &crate::DeviceRegistrationMetadata) -> EntitlementVerifier {
        EntitlementVerifier::new(
            EntitlementVerifierConfig {
                issuer: self.authority.issuer.clone(),
                audience: self.authority.audience.clone(),
                device_id: metadata.device_id.clone(),
                device_key_thumbprint: metadata.public_key_thumbprint.clone(),
                minimum_license_epoch: self.authority.minimum_license_epoch,
                clock_skew_seconds: 60,
                client_build: self.client_build.clone(),
            },
            self.authority.keys.clone(),
        )
    }

    fn activation_verifier_for(
        &self,
        metadata: &crate::DeviceRegistrationMetadata,
    ) -> ActivationProofVerifier {
        ActivationProofVerifier::new(
            EntitlementVerifierConfig {
                issuer: self.authority.issuer.clone(),
                audience: activation_audience(&self.authority.audience),
                device_id: metadata.device_id.clone(),
                device_key_thumbprint: metadata.public_key_thumbprint.clone(),
                minimum_license_epoch: self.authority.minimum_license_epoch,
                clock_skew_seconds: 60,
                client_build: self.client_build.clone(),
            },
            self.authority.keys.clone(),
        )
    }

    async fn refresh_session(&self) -> Result<crate::RefreshSession> {
        let store = self.store.clone();
        self.credential_operations
            .run("refresh_session_load", move || load_refresh_session(store))
            .await
    }

    async fn clear_cached_authorization_material_after_marker_failure(
        &self,
        marker_result: Result<()>,
    ) -> Result<()> {
        let marker_error = match marker_result {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        let store = self.store.clone();
        if let Err(cleanup_error) = self
            .credential_operations
            .run(
                "authorization_material_clear_after_marker_failure",
                move || clear_cached_authorization_material(store.as_ref()),
            )
            .await
        {
            tracing::error!(
                %cleanup_error,
                "failed to clear cached authorization material after marker persistence failure"
            );
        }
        Err(marker_error)
    }

    fn refresh_time_policy(&self, local_unix: i64) -> Result<()> {
        let observation = match self.trusted_time.observe(local_unix) {
            Ok(observation) => observation,
            Err(error) => {
                self.guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::CorruptSecureStore,
                    });
                return Err(error);
            }
        };
        if let Some(observation) = observation {
            if observation.rollback_detected {
                self.guard
                    .replace_state(EntitlementState::RevalidationRequired {
                        reason: crate::RevalidationReason::ClockRollback,
                    });
                return Err(LicensingError::ClockRollback);
            }
            self.guard.apply_time_policy(observation.unix);
        } else {
            // Initialization already rejects a cached lease without a trusted
            // anchor. Once published, entitlement installation records the
            // anchor before it exposes Active state, so authorization checks
            // never need synchronous OS credential I/O.
            self.guard.apply_time_policy(local_unix);
        }
        Ok(())
    }
}

fn device_state_error(state: DeviceState) -> LicensingError {
    match state {
        DeviceState::PendingActivation => LicensingError::DeviceActivationPending,
        DeviceState::Removed => LicensingError::DeviceRemoved,
        DeviceState::Revoked => LicensingError::DeviceRevoked,
        DeviceState::Suspicious => LicensingError::DeviceSuspicious,
        DeviceState::Active => LicensingError::DeviceDenied,
    }
}

fn cached_entitlement_error_state(error: LicensingError) -> EntitlementState {
    match error {
        LicensingError::LicensePastDue => EntitlementState::LicenseInactive {
            reason: crate::LicenseInactiveReason::LicensePastDue,
            entitlement: None,
        },
        LicensingError::LicenseCanceled => EntitlementState::LicenseInactive {
            reason: crate::LicenseInactiveReason::LicenseCanceled,
            entitlement: None,
        },
        LicensingError::LicenseExpired => EntitlementState::LicenseInactive {
            reason: crate::LicenseInactiveReason::LicenseExpired,
            entitlement: None,
        },
        LicensingError::LicenseUnavailable => EntitlementState::LicenseInactive {
            reason: crate::LicenseInactiveReason::LicenseUnavailable,
            entitlement: None,
        },
        LicensingError::ClientUpgradeRequired { policy } => {
            EntitlementState::ClientUpgradeRequired {
                policy,
                entitlement: None,
            }
        }
        LicensingError::ObsoleteLicenseEpoch => EntitlementState::RevalidationRequired {
            reason: crate::RevalidationReason::ObsoleteEpoch,
        },
        LicensingError::SecureStoreCorrupt
        | LicensingError::SecureStoreUnavailable
        | LicensingError::SecureStoreBackend => EntitlementState::RevalidationRequired {
            reason: crate::RevalidationReason::CorruptSecureStore,
        },
        _ => EntitlementState::RevalidationRequired {
            reason: crate::RevalidationReason::InvalidServerProof,
        },
    }
}

fn activation_audience(audience: &str) -> String {
    format!("{audience}:activation")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, VecDeque},
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use p256::{
        ecdsa::SigningKey,
        elliptic_curve::Generate,
        pkcs8::{EncodePrivateKey, EncodePublicKey},
    };

    use super::*;
    use crate::{
        Capability, EntitlementClaims, InMemoryAuditSink, NumericLimit, Plan, SecureStore,
        SessionSecureStore,
    };

    #[derive(Default)]
    struct TestPersistentStore {
        values: Mutex<Vec<(SecretKey, Vec<u8>)>>,
        fail_reads: AtomicBool,
        fail_writes: AtomicBool,
    }

    impl SecureStore for TestPersistentStore {
        fn mode(&self) -> SecureStoreMode {
            SecureStoreMode::Persistent
        }

        fn get_secret(&self, key: SecretKey) -> Result<Option<Vec<u8>>> {
            if self.fail_reads.load(Ordering::Acquire) {
                return Err(LicensingError::SecureStoreBackend);
            }
            Ok(self
                .values
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|(stored, _)| *stored == key)
                .map(|(_, value)| value.clone()))
        }

        fn put_secret(&self, key: SecretKey, value: &[u8]) -> Result<()> {
            if self.fail_writes.load(Ordering::Acquire) {
                return Err(LicensingError::SecureStoreBackend);
            }
            let mut values = self
                .values
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            values.retain(|(stored, _)| *stored != key);
            values.push((key, value.to_vec()));
            Ok(())
        }

        fn delete_secret(&self, key: SecretKey) -> Result<()> {
            self.values
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(stored, _)| *stored != key);
            Ok(())
        }
    }

    fn empty_authority() -> LicensingAuthority {
        LicensingAuthority {
            issuer: "issuer".into(),
            audience: "audience".into(),
            minimum_license_epoch: 0,
            keys: TrustedEntitlementKeys::from_pem_keys([]).unwrap(),
        }
    }

    fn test_build() -> crate::ClientBuildIdentity {
        crate::ClientBuildIdentity::parse("1.0.0").expect("test build")
    }

    struct StatusLicenseApi {
        expected_session: String,
        challenges: Mutex<VecDeque<crate::DeviceChallenge>>,
        responses: Mutex<VecDeque<crate::EntitlementStatusResponse>>,
        challenge_requests: Mutex<Vec<crate::ChallengeRequest>>,
        proofs: Mutex<Vec<crate::DeviceProof>>,
        team_operation_committed: bool,
        team_operation_status_requests: Mutex<Vec<crate::LeaveWorkspace>>,
        team_leave_requests: Mutex<Vec<crate::LeaveWorkspace>>,
        team_leave_timeout: bool,
    }

    #[async_trait::async_trait]
    impl LicenseApi for StatusLicenseApi {
        async fn register_device(
            &self,
            _request: crate::DeviceRegistrationRequest,
        ) -> Result<crate::DeviceRegistrationResponse> {
            unreachable!("status flow must not register a device")
        }

        async fn confirm_activation(
            &self,
            _session: &crate::RefreshSession,
        ) -> Result<crate::DeviceActivationConfirmationResponse> {
            unreachable!("status flow must not confirm activation")
        }

        async fn verify_activation(
            &self,
            _session: &crate::RefreshSession,
            _request: crate::ActivationVerificationRequest,
        ) -> Result<crate::ActivationVerificationResponse> {
            unreachable!("status flow must not verify activation")
        }

        async fn issue_challenge(
            &self,
            session: &crate::RefreshSession,
            request: crate::ChallengeRequest,
        ) -> Result<crate::DeviceChallenge> {
            assert_eq!(session.expose(), self.expected_session);
            self.challenge_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request);
            Ok(self
                .challenges
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .expect("status challenge"))
        }

        async fn issue_session_recovery_challenge(
            &self,
            _request: crate::SessionRecoveryChallengeRequest,
        ) -> Result<crate::DeviceChallenge> {
            unreachable!("status flow must not recover a session")
        }

        async fn recover_session(
            &self,
            _request: crate::SessionRecoveryRequest,
        ) -> Result<crate::SessionRecoveryResponse> {
            unreachable!("status flow must not recover a session")
        }

        async fn refresh_entitlement(
            &self,
            _session: &crate::RefreshSession,
            _request: crate::EntitlementRefreshRequest,
        ) -> Result<crate::EntitlementRefreshResponse> {
            unreachable!("status flow must not refresh an entitlement")
        }

        async fn entitlement_status(
            &self,
            session: &crate::RefreshSession,
            request: crate::EntitlementStatusRequest,
        ) -> Result<crate::EntitlementStatusResponse> {
            assert_eq!(session.expose(), self.expected_session);
            self.proofs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.proof);
            Ok(self
                .responses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .expect("status response"))
        }

        async fn list_devices(
            &self,
            session: &crate::RefreshSession,
            proof: &crate::DeviceProof,
            _cursor: Option<&str>,
            _page_size: u32,
        ) -> Result<crate::RegisteredDevicePage> {
            assert_eq!(session.expose(), self.expected_session);
            self.proofs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(proof.clone());
            Ok(crate::RegisteredDevicePage {
                devices: Vec::new(),
                next_cursor: None,
            })
        }

        async fn remove_device(
            &self,
            _session: &crate::RefreshSession,
            _device_id: &str,
            _request: crate::DeviceRemovalRequest,
        ) -> Result<()> {
            unreachable!("status flow must not remove a device")
        }

        async fn billing_summary(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
        ) -> Result<crate::BillingSummary> {
            unreachable!("status flow must not load billing")
        }

        async fn submit_customer_payment(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _submission: crate::CustomerPaymentSubmission,
        ) -> Result<crate::ManualPaymentClaim> {
            unreachable!("status flow must not submit billing")
        }

        async fn team_profile(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
        ) -> Result<crate::TeamProfile> {
            unreachable!("status flow must not load team profile")
        }

        async fn team_leave_operation_status(
            &self,
            session: &crate::RefreshSession,
            request: &crate::LeaveWorkspace,
        ) -> Result<crate::TeamOperationStatusResponse> {
            assert_eq!(session.expose(), self.expected_session);
            self.team_operation_status_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.clone());
            Ok(crate::TeamOperationStatusResponse {
                committed: self.team_operation_committed,
            })
        }

        async fn team_members(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _request: crate::TeamMemberPageRequest,
        ) -> Result<crate::TeamMemberPage> {
            unreachable!("status flow must not load team members")
        }

        async fn create_team_invitation(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _request: crate::CreateTeamInvitation,
        ) -> Result<crate::TeamInvitation> {
            unreachable!("status flow must not create team invitations")
        }

        async fn accept_team_invitation(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _request: crate::AcceptTeamInvitation,
        ) -> Result<crate::TeamProfile> {
            unreachable!("status flow must not accept team invitations")
        }

        async fn update_team_member(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _member_id: &str,
            _request: crate::UpdateWorkspaceMember,
        ) -> Result<crate::WorkspaceMember> {
            unreachable!("status flow must not update team members")
        }

        async fn create_team_device_enrollment(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _request: crate::TeamOperationRequest,
        ) -> Result<crate::MemberDeviceEnrollment> {
            unreachable!("status flow must not create team device enrollments")
        }

        async fn accept_team_device_enrollment(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _request: crate::AcceptMemberDeviceEnrollment,
        ) -> Result<crate::TeamProfile> {
            unreachable!("status flow must not accept team device enrollments")
        }

        async fn leave_team_workspace(
            &self,
            session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            request: crate::LeaveWorkspace,
        ) -> Result<()> {
            assert_eq!(session.expose(), self.expected_session);
            self.team_leave_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request);
            if self.team_leave_timeout {
                Err(LicensingError::Timeout)
            } else {
                Ok(())
            }
        }

        async fn transfer_team_ownership(
            &self,
            _session: &crate::RefreshSession,
            _proof: &crate::DeviceProof,
            _request: crate::TransferWorkspaceOwnership,
        ) -> Result<crate::OwnershipTransferResult> {
            unreachable!("status flow must not transfer team ownership")
        }

        async fn logout(&self, _session: crate::RefreshSession) -> Result<()> {
            unreachable!("status flow must not log out")
        }
    }

    fn active_entitlement(
        metadata: &crate::DeviceRegistrationMetadata,
    ) -> crate::VerifiedEntitlement {
        crate::VerifiedEntitlement {
            key_id: "test-key".into(),
            claims: EntitlementClaims {
                schema_version: 3,
                iss: "issuer".into(),
                aud: "audience".into(),
                sub: "account".into(),
                license_id: "license".into(),
                device_id: metadata.device_id.clone(),
                device_key_thumbprint: metadata.public_key_thumbprint.clone(),
                plan: Plan::Pro,
                plan_revision: 2,
                policy_hash: "0".repeat(64),
                license_status: crate::LicenseStanding::Active,
                capabilities: vec![Capability::ManagedConfigSources],
                workspace_permissions: Vec::new(),
                limits: BTreeMap::from([
                    (NumericLimit::MaxPrograms, 20),
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
                issued_at: 1_000,
                refresh_after: 1_500,
                expires_at: 2_000,
                offline_access_ends_at: 3_000,
                token_id: "lease".into(),
                key_id: "test-key".into(),
            },
        }
    }

    #[test]
    fn session_only_storage_never_restores_offline_entitlement() {
        let store = Arc::new(SessionSecureStore::default());
        let authority = LicensingAuthority {
            issuer: "issuer".into(),
            audience: "audience".into(),
            minimum_license_epoch: 0,
            keys: TrustedEntitlementKeys::from_pem_keys([]).unwrap(),
        };
        let service = AuthorizationService::initialize(store, authority, test_build(), 1_000);
        assert!(matches!(service.state(), EntitlementState::SessionOnly));
    }

    #[tokio::test]
    async fn device_registration_metadata_is_loaded_through_the_credential_gate() {
        let service = AuthorizationService::initialize(
            Arc::new(SessionSecureStore::default()),
            empty_authority(),
            test_build(),
            1_000,
        );
        assert!(
            service
                .device_registration_metadata()
                .await
                .unwrap()
                .is_none()
        );

        let expected = service
            .identity_provider()
            .load_or_create("Windows", "2.0.0", Some("Workstation".into()))
            .expect("identity")
            .metadata;
        assert_eq!(
            service.device_registration_metadata().await.unwrap(),
            Some(expected)
        );
    }

    #[tokio::test]
    async fn entitlement_status_uses_fresh_device_proofs_and_applies_epoch_downgrades() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000);
        let identity = service
            .identity_provider()
            .load_or_create("test", "9.9.9", None)
            .expect("identity");
        let device_id = identity.metadata.device_id.clone();
        let public_key_pem = identity.metadata.public_key_pem.clone();
        service
            .rotate_refresh_session(b"status-refresh-session-material")
            .await
            .expect("session");
        service.guard.replace_state(EntitlementState::Active {
            entitlement: active_entitlement(&identity.metadata),
        });
        let challenges = [
            crate::DeviceChallenge {
                challenge_id: "status-challenge-1".into(),
                nonce: "first-status-challenge-nonce".into(),
                requested_scope: "entitlement:status".into(),
                issued_at: 1_100,
                expires_at: 1_160,
            },
            crate::DeviceChallenge {
                challenge_id: "status-challenge-2".into(),
                nonce: "second-status-challenge-nonce".into(),
                requested_scope: "entitlement:status".into(),
                issued_at: 1_110,
                expires_at: 1_170,
            },
        ];
        let api = StatusLicenseApi {
            expected_session: "status-refresh-session-material".into(),
            challenges: Mutex::new(VecDeque::from(challenges.clone())),
            responses: Mutex::new(VecDeque::from([
                crate::EntitlementStatusResponse {
                    device_state: DeviceState::Active,
                    license_epoch: 1,
                },
                crate::EntitlementStatusResponse {
                    device_state: DeviceState::Active,
                    license_epoch: 2,
                },
            ])),
            challenge_requests: Mutex::new(Vec::new()),
            proofs: Mutex::new(Vec::new()),
            team_operation_committed: false,
            team_operation_status_requests: Mutex::new(Vec::new()),
            team_leave_requests: Mutex::new(Vec::new()),
            team_leave_timeout: false,
        };

        let first = service
            .entitlement_status_with_api(&api, 1_100)
            .await
            .expect("first status");
        assert_eq!(first.license_epoch, 1);
        assert_eq!(
            service.refresh_session().await.unwrap().expose(),
            api.expected_session
        );
        assert_eq!(service.trusted_now(1_000).unwrap(), 1_000);
        assert!(store.get_secret(SecretKey::TrustedTime).unwrap().is_none());

        let second = service
            .entitlement_status_with_api(&api, 1_110)
            .await
            .expect("second status");
        assert_eq!(second.license_epoch, 2);
        assert!(matches!(
            service.state(),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::ObsoleteEpoch
            }
        ));

        let requests = api
            .challenge_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| {
            request.device_id == device_id && request.requested_scope == "entitlement:status"
        }));
        let proofs = api
            .proofs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(proofs.len(), 2);
        let verifier = crate::DeviceProofVerifier::from_public_key_pem(&public_key_pem)
            .expect("proof verifier");
        for (challenge, proof) in challenges.iter().zip(proofs.iter()) {
            assert_eq!(proof.app_version, "1.0.0");
            assert_eq!(proof.device_id, device_id);
            verifier
                .verify_and_consume(challenge, proof, &device_id, challenge.issued_at)
                .expect("current-build device proof");
        }
    }

    #[tokio::test]
    async fn unsigned_future_challenge_cannot_advance_trusted_time() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000);
        let identity = service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        service
            .rotate_refresh_session(b"future-challenge-refresh-session")
            .await
            .expect("session");
        service.guard.replace_state(EntitlementState::Active {
            entitlement: active_entitlement(&identity.metadata),
        });
        let api = StatusLicenseApi {
            expected_session: "future-challenge-refresh-session".into(),
            challenges: Mutex::new(VecDeque::from([crate::DeviceChallenge {
                challenge_id: "future-status-challenge".into(),
                nonce: "future-status-challenge-nonce".into(),
                requested_scope: "entitlement:status".into(),
                issued_at: 1_000_000,
                expires_at: 1_000_060,
            }])),
            responses: Mutex::new(VecDeque::new()),
            challenge_requests: Mutex::new(Vec::new()),
            proofs: Mutex::new(Vec::new()),
            team_operation_committed: false,
            team_operation_status_requests: Mutex::new(Vec::new()),
            team_leave_requests: Mutex::new(Vec::new()),
            team_leave_timeout: false,
        };

        assert!(matches!(
            service.entitlement_status_with_api(&api, 1_000).await,
            Err(LicensingError::InvalidChallenge)
        ));
        assert!(store.get_secret(SecretKey::TrustedTime).unwrap().is_none());
        assert!(
            api.proofs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn team_leave_retry_recovers_committed_operation_before_requesting_a_challenge() {
        let service = AuthorizationService::initialize(
            Arc::new(TestPersistentStore::default()),
            empty_authority(),
            test_build(),
            1_000,
        );
        service
            .rotate_refresh_session(b"team-leave-refresh-session-material")
            .await
            .expect("session");
        let operation_id = uuid::Uuid::new_v4().to_string();
        let api = StatusLicenseApi {
            expected_session: "team-leave-refresh-session-material".into(),
            challenges: Mutex::new(VecDeque::new()),
            responses: Mutex::new(VecDeque::new()),
            challenge_requests: Mutex::new(Vec::new()),
            proofs: Mutex::new(Vec::new()),
            team_operation_committed: true,
            team_operation_status_requests: Mutex::new(Vec::new()),
            team_leave_requests: Mutex::new(Vec::new()),
            team_leave_timeout: false,
        };

        service
            .leave_team_workspace_with_api(
                &api,
                crate::LeaveWorkspace {
                    operation_id: operation_id.clone(),
                    member_id: "member_leave_retry".into(),
                    row_version: 7,
                },
                1_100,
            )
            .await
            .expect("recover committed leave");

        assert_eq!(
            *api.team_operation_status_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [crate::LeaveWorkspace {
                operation_id,
                member_id: "member_leave_retry".into(),
                row_version: 7,
            }]
        );
        assert!(
            api.challenge_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        assert!(
            api.team_leave_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn ambiguous_team_leave_preserves_the_local_session_until_status_confirms() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_000);
        let identity = service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        let device_id = identity.metadata.device_id.clone();
        service
            .rotate_refresh_session(b"team-leave-ambiguous-session-material")
            .await
            .expect("session");
        service.guard.replace_state(EntitlementState::Active {
            entitlement: active_entitlement(&identity.metadata),
        });
        let challenge = crate::DeviceChallenge {
            challenge_id: "team-leave-ambiguous-challenge".into(),
            nonce: "team-leave-ambiguous-nonce".into(),
            requested_scope: TEAM_LEAVE_SCOPE.into(),
            issued_at: 1_100,
            expires_at: 1_160,
        };
        let request = crate::LeaveWorkspace {
            operation_id: uuid::Uuid::new_v4().to_string(),
            member_id: "member_leave_ambiguous".into(),
            row_version: 9,
        };
        let api = StatusLicenseApi {
            expected_session: "team-leave-ambiguous-session-material".into(),
            challenges: Mutex::new(VecDeque::from([challenge])),
            responses: Mutex::new(VecDeque::new()),
            challenge_requests: Mutex::new(Vec::new()),
            proofs: Mutex::new(Vec::new()),
            team_operation_committed: false,
            team_operation_status_requests: Mutex::new(Vec::new()),
            team_leave_requests: Mutex::new(Vec::new()),
            team_leave_timeout: true,
        };

        assert!(matches!(
            service
                .leave_team_workspace_with_api(&api, request.clone(), 1_100)
                .await,
            Err(LicensingError::Timeout)
        ));
        assert_eq!(
            service
                .refresh_session()
                .await
                .expect("ambiguous leave retains refresh session")
                .expose(),
            "team-leave-ambiguous-session-material"
        );
        {
            let status_requests = api
                .team_operation_status_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(status_requests.as_slice(), std::slice::from_ref(&request));
        }
        assert_eq!(
            *api.team_leave_requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            [request]
        );
        let challenges = api
            .challenge_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(challenges.len(), 1);
        assert_eq!(challenges[0].device_id, device_id);
        assert_eq!(challenges[0].requested_scope, TEAM_LEAVE_SCOPE);
    }

    #[tokio::test]
    async fn sensitive_customer_requests_use_scope_bound_device_proofs() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_000);
        let identity = service
            .identity_provider()
            .load_or_create("test", "9.9.9", None)
            .expect("identity");
        let device_id = identity.metadata.device_id.clone();
        service
            .rotate_refresh_session(b"sensitive-refresh-session-material")
            .await
            .expect("session");
        service.guard.replace_state(EntitlementState::Active {
            entitlement: active_entitlement(&identity.metadata),
        });
        let challenge = crate::DeviceChallenge {
            challenge_id: "device-list-challenge".into(),
            nonce: "device-list-challenge-nonce".into(),
            requested_scope: DEVICE_LIST_SCOPE.into(),
            issued_at: 1_100,
            expires_at: 1_160,
        };
        let api = StatusLicenseApi {
            expected_session: "sensitive-refresh-session-material".into(),
            challenges: Mutex::new(VecDeque::from([challenge.clone()])),
            responses: Mutex::new(VecDeque::new()),
            challenge_requests: Mutex::new(Vec::new()),
            proofs: Mutex::new(Vec::new()),
            team_operation_committed: false,
            team_operation_status_requests: Mutex::new(Vec::new()),
            team_leave_requests: Mutex::new(Vec::new()),
            team_leave_timeout: false,
        };

        let page = service
            .list_devices_with_api(&api, None, 50, 1_100)
            .await
            .expect("device list");
        assert!(page.devices.is_empty());
        let requests = api
            .challenge_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].device_id, device_id);
        assert_eq!(requests[0].requested_scope, DEVICE_LIST_SCOPE);
        drop(requests);
        let proofs = api
            .proofs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(proofs.len(), 1);
        crate::DeviceProofVerifier::from_public_key_pem(&identity.metadata.public_key_pem)
            .expect("proof verifier")
            .verify_and_consume(&challenge, &proofs[0], &device_id, challenge.issued_at)
            .expect("scope-bound device proof");
    }

    #[tokio::test]
    async fn persisted_registration_without_an_entitlement_resumes_as_activation_pending() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000);
        service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        service
            .rotate_refresh_session(b"refresh-session-material-123")
            .await
            .expect("session");

        let restored =
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_001);
        assert!(matches!(
            restored.state(),
            EntitlementState::ActivationPending
        ));
    }

    #[test]
    fn malformed_pending_refresh_session_fails_closed() {
        let store = Arc::new(TestPersistentStore::default());
        AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000)
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        store
            .put_secret(SecretKey::RefreshSession, b"short")
            .expect("corrupt session");

        let restored =
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_001);
        assert!(matches!(
            restored.state(),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::CorruptSecureStore
            }
        ));
    }

    #[tokio::test]
    async fn commercial_denials_survive_restart_and_fail_closed_on_storage_error() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000);
        service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        service
            .apply_license_inactive(crate::LicenseInactiveReason::AccountSuspended, 1_000)
            .await
            .expect("persist denial");
        let restored =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_001);
        assert!(matches!(
            restored.state(),
            EntitlementState::LicenseInactive {
                reason: crate::LicenseInactiveReason::AccountSuspended,
                ..
            }
        ));

        restored
            .rotate_refresh_session(b"refresh-session-material-123")
            .await
            .expect("session");
        store
            .put_secret(SecretKey::EntitlementLease, b"cached-entitlement")
            .expect("entitlement");
        store.fail_writes.store(true, Ordering::Release);
        assert!(
            restored
                .apply_revalidation_required(crate::RevalidationReason::InvalidServerProof, 1_002)
                .await
                .is_err()
        );
        assert!(matches!(
            restored.state(),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::InvalidServerProof
            }
        ));
        assert!(
            store
                .get_secret(SecretKey::RefreshSession)
                .expect("session")
                .is_none()
        );
        assert!(
            store
                .get_secret(SecretKey::EntitlementLease)
                .expect("entitlement")
                .is_none()
        );
    }

    #[tokio::test]
    async fn denial_marker_failure_clears_cached_authorization_material() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000);
        service
            .rotate_refresh_session(b"refresh-session-material-123")
            .await
            .expect("session");
        store
            .put_secret(SecretKey::EntitlementLease, b"cached-entitlement")
            .expect("entitlement");
        store.fail_writes.store(true, Ordering::Release);

        assert!(
            service
                .apply_license_inactive(crate::LicenseInactiveReason::LicenseCanceled, 1_001)
                .await
                .is_err()
        );
        assert!(matches!(
            service.state(),
            EntitlementState::LicenseInactive {
                reason: crate::LicenseInactiveReason::LicenseCanceled,
                ..
            }
        ));
        assert!(
            store
                .get_secret(SecretKey::RefreshSession)
                .expect("session")
                .is_none()
        );
        assert!(
            store
                .get_secret(SecretKey::EntitlementLease)
                .expect("entitlement")
                .is_none()
        );
    }

    #[tokio::test]
    async fn client_upgrade_denial_survives_restart_and_a_supported_build_clears_it() {
        let store = Arc::new(TestPersistentStore::default());
        let audit = Arc::new(InMemoryAuditSink::default());
        let service = AuthorizationService::initialize_with_audit(
            store.clone(),
            empty_authority(),
            test_build(),
            1_000,
            audit.clone(),
        );
        service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        let policy = crate::ClientVersionPolicy {
            minimum_version: "2.0.0".into(),
            recommended_version: "2.1.0".into(),
            enforce_after: 5_000,
        };
        service
            .apply_client_upgrade_required(policy.clone(), 900)
            .await
            .expect("persist upgrade denial");
        assert!(matches!(
            service.state(),
            EntitlementState::ClientUpgradeRequired { .. }
        ));
        assert!(audit.events().iter().any(|event| {
            event.event_type == AuditEventType::ClientUpgradeRequired
                && event.outcome == AuditOutcome::Denied
                && event.occurred_at == policy.enforce_after
        }));

        let restored =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_001);
        assert!(matches!(
            restored.state(),
            EntitlementState::ClientUpgradeRequired { .. }
        ));

        let supported = AuthorizationService::initialize(
            store.clone(),
            empty_authority(),
            crate::ClientBuildIdentity::parse("2.0.0").unwrap(),
            1_002,
        );
        assert!(matches!(
            supported.state(),
            EntitlementState::Unauthenticated
        ));
        assert_eq!(
            supported.trusted_now(1_002).expect("trusted marker floor"),
            policy.enforce_after
        );
        assert!(matches!(
            supported.apply_client_upgrade_required(policy, 1_002).await,
            Err(LicensingError::InvalidServerResponse)
        ));
        assert!(
            store
                .get_secret(SecretKey::ClientUpgradeMarker)
                .expect("upgrade marker")
                .is_none()
        );
    }

    #[tokio::test]
    async fn local_deauthorization_and_device_denial_survive_restart() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000);
        service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        service
            .rotate_refresh_session(b"refresh-session-material-123")
            .await
            .expect("session");
        let captured = service
            .deauthorize_locally(1_001)
            .await
            .expect("persist local sign-out");
        assert!(captured.is_some());
        assert!(matches!(
            AuthorizationService::initialize(
                store.clone(),
                empty_authority(),
                test_build(),
                1_002,
            )
            .state(),
            EntitlementState::Unauthenticated
        ));

        store
            .delete_secret(SecretKey::AuthorizationBlock)
            .expect("allow a new online state");
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_003);
        service
            .apply_device_state_persisted(DeviceState::Suspicious, 1_003)
            .await
            .expect("persist device denial");
        assert!(matches!(
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_004).state(),
            EntitlementState::DeviceDenied {
                state: DeviceState::Suspicious
            }
        ));
    }

    #[test]
    fn cached_entitlement_without_trusted_time_requires_revalidation() {
        let store = Arc::new(TestPersistentStore::default());
        let service =
            AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000);
        service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        store
            .put_secret(SecretKey::EntitlementLease, b"cached-entitlement")
            .expect("entitlement");

        let restored =
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_001);
        assert!(matches!(
            restored.state(),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::CorruptSecureStore
            }
        ));
    }

    #[test]
    fn cached_entitlement_failures_preserve_actionable_revalidation_reason() {
        assert!(matches!(
            cached_entitlement_error_state(LicensingError::InvalidSignature),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::InvalidServerProof
            }
        ));
        assert!(matches!(
            cached_entitlement_error_state(LicensingError::ObsoleteLicenseEpoch),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::ObsoleteEpoch
            }
        ));
        assert!(matches!(
            cached_entitlement_error_state(LicensingError::SecureStoreCorrupt),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::CorruptSecureStore
            }
        ));
        assert!(matches!(
            cached_entitlement_error_state(LicensingError::LicenseExpired),
            EntitlementState::LicenseInactive {
                reason: crate::LicenseInactiveReason::LicenseExpired,
                ..
            }
        ));
    }

    #[test]
    fn device_states_map_to_specific_actionable_errors() {
        assert!(matches!(
            device_state_error(DeviceState::PendingActivation),
            LicensingError::DeviceActivationPending
        ));
        assert!(matches!(
            device_state_error(DeviceState::Removed),
            LicensingError::DeviceRemoved
        ));
        assert!(matches!(
            device_state_error(DeviceState::Revoked),
            LicensingError::DeviceRevoked
        ));
        assert!(matches!(
            device_state_error(DeviceState::Suspicious),
            LicensingError::DeviceSuspicious
        ));
    }

    #[test]
    fn initialization_fails_closed_when_trusted_time_storage_is_corrupt() {
        let store = Arc::new(TestPersistentStore::default());
        AuthorizationService::initialize(store.clone(), empty_authority(), test_build(), 1_000)
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        store
            .put_secret(SecretKey::TrustedTime, b"not-json")
            .expect("corrupt trusted time");
        let service =
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_000);

        assert!(matches!(
            service.state(),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::CorruptSecureStore
            }
        ));
    }

    #[test]
    fn initialization_fails_closed_when_trusted_time_storage_is_unavailable() {
        let store = Arc::new(TestPersistentStore::default());
        store.fail_reads.store(true, Ordering::Release);
        let service =
            AuthorizationService::initialize(store, empty_authority(), test_build(), 1_000);

        assert!(matches!(
            service.state(),
            EntitlementState::RevalidationRequired {
                reason: crate::RevalidationReason::CorruptSecureStore
            }
        ));
    }

    #[tokio::test]
    async fn resetting_device_identity_removes_all_local_license_material() {
        let store = Arc::new(TestPersistentStore::default());
        let audit = Arc::new(InMemoryAuditSink::default());
        let service = AuthorizationService::initialize_with_audit(
            store.clone(),
            empty_authority(),
            test_build(),
            1_000,
            audit.clone(),
        );
        let device_id = service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity")
            .metadata
            .device_id;
        service
            .rotate_refresh_session(b"refresh-session-material-123")
            .await
            .expect("session");
        store
            .put_secret(SecretKey::EntitlementLease, b"cached-entitlement")
            .expect("entitlement");

        service
            .reset_device_identity(1_100)
            .await
            .expect("reset identity");

        assert!(matches!(service.state(), EntitlementState::Unauthenticated));
        assert!(
            service
                .identity_provider()
                .load()
                .expect("load identity")
                .is_none()
        );
        assert!(
            store
                .get_secret(SecretKey::RefreshSession)
                .expect("session")
                .is_none()
        );
        assert!(
            store
                .get_secret(SecretKey::EntitlementLease)
                .expect("entitlement")
                .is_none()
        );
        assert!(audit.events().iter().any(|event| {
            event.event_type == AuditEventType::DeviceIdentityReset
                && event.outcome == AuditOutcome::Succeeded
                && event.device_id.as_deref() == Some(device_id.as_str())
        }));
    }

    #[tokio::test]
    async fn rejects_short_refresh_session_material() {
        let store = Arc::new(SessionSecureStore::default());
        let service = AuthorizationService::initialize(
            store,
            LicensingAuthority {
                issuer: "issuer".into(),
                audience: "audience".into(),
                minimum_license_epoch: 0,
                keys: TrustedEntitlementKeys::from_pem_keys([]).unwrap(),
            },
            test_build(),
            1_000,
        );
        assert!(matches!(
            service.rotate_refresh_session(b"short").await,
            Err(LicensingError::InvalidServerResponse)
        ));
    }

    #[test]
    fn removed_and_revoked_devices_are_denied() {
        for state in [
            DeviceState::Removed,
            DeviceState::Revoked,
            DeviceState::Suspicious,
        ] {
            let service = AuthorizationService::initialize(
                Arc::new(SessionSecureStore::default()),
                LicensingAuthority {
                    issuer: "issuer".into(),
                    audience: "audience".into(),
                    minimum_license_epoch: 0,
                    keys: TrustedEntitlementKeys::from_pem_keys([]).unwrap(),
                },
                test_build(),
                1_000,
            );
            service.apply_device_state(state);
            assert!(matches!(
                service.state(),
                EntitlementState::DeviceDenied { state: denied } if denied == state
            ));
        }
    }

    #[tokio::test]
    async fn first_activation_can_install_entitlement_without_restarting() {
        let entitlement_key = SigningKey::try_generate().expect("test signing key entropy");
        let private = entitlement_key
            .to_pkcs8_pem(Default::default())
            .expect("private");
        let public = entitlement_key
            .verifying_key()
            .to_public_key_pem(Default::default())
            .expect("public");
        let store = Arc::new(SessionSecureStore::default());
        let service = AuthorizationService::initialize(
            store,
            LicensingAuthority {
                issuer: "issuer".into(),
                audience: "audience".into(),
                minimum_license_epoch: 1,
                keys: TrustedEntitlementKeys::from_pem_keys([(
                    "entitlement-key",
                    public.as_bytes(),
                )])
                .expect("keys"),
            },
            test_build(),
            1_000,
        );
        let identity = service
            .identity_provider()
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        let claims = EntitlementClaims {
            schema_version: 3,
            iss: "issuer".into(),
            aud: "audience".into(),
            sub: "account".into(),
            license_id: "license".into(),
            device_id: identity.metadata.device_id,
            device_key_thumbprint: identity.metadata.public_key_thumbprint,
            plan: Plan::Pro,
            plan_revision: 2,
            policy_hash: "0".repeat(64),
            license_status: crate::LicenseStanding::Active,
            capabilities: vec![Capability::ManagedConfigSources],
            workspace_permissions: Vec::new(),
            limits: BTreeMap::from([
                (NumericLimit::MaxPrograms, 20),
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
            issued_at: 1_000,
            refresh_after: 1_100,
            expires_at: 2_000,
            offline_access_ends_at: 3_000,
            token_id: "lease".into(),
            key_id: "entitlement-key".into(),
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("entitlement-key".into());
        crate::es256_provider::ensure_installed();
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_ec_pem(private.as_bytes()).expect("encoding"),
        )
        .expect("token");
        service
            .install_entitlement(&token, 1_050, 1_050)
            .await
            .expect("install");
        assert!(matches!(service.state(), EntitlementState::Active { .. }));
        assert!(matches!(
            service.state_at(2_051),
            EntitlementState::RestrictedOffline { .. }
        ));
        assert!(matches!(
            service.authorize(
                crate::RestrictedOperation::Protected(
                    crate::ProtectedOperation::UseManagedConfigSources,
                ),
                2_051,
            ),
            Err(LicensingError::AuthorizationRequired)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_credential_write_cannot_be_overtaken() {
        let operations = LocalCredentialOperations::with_timeout(Duration::from_millis(20));
        let value = Arc::new(AtomicUsize::new(0));

        let slow_value = value.clone();
        assert!(matches!(
            operations
                .run("slow_write", move || {
                    std::thread::sleep(Duration::from_millis(200));
                    slow_value.store(1, Ordering::Release);
                    Ok(())
                })
                .await,
            Err(LicensingError::SecureStoreTimeout)
        ));

        let overtaking_started = Arc::new(AtomicBool::new(false));
        let later_value = value.clone();
        let later_started = overtaking_started.clone();
        assert!(matches!(
            operations
                .run("later_write", move || {
                    later_started.store(true, Ordering::Release);
                    later_value.store(2, Ordering::Release);
                    Ok(())
                })
                .await,
            Err(LicensingError::SecureStoreTimeout)
        ));
        assert!(!overtaking_started.load(Ordering::Acquire));

        tokio::time::sleep(Duration::from_millis(220)).await;
        assert_eq!(value.load(Ordering::Acquire), 1);
        let final_value = value.clone();
        operations
            .run("final_write", move || {
                final_value.store(3, Ordering::Release);
                Ok(())
            })
            .await
            .expect("write after the timed-out worker settled");
        assert_eq!(value.load(Ordering::Acquire), 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_credential_caller_cannot_release_the_worker_gate() {
        let operations = LocalCredentialOperations::with_timeout(Duration::from_secs(1));
        let value = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(AtomicBool::new(false));
        let first_operations = operations.clone();
        let first_value = value.clone();
        let first_started = started.clone();
        let first = tokio::spawn(async move {
            first_operations
                .run("cancelled_write", move || {
                    first_started.store(true, Ordering::Release);
                    std::thread::sleep(Duration::from_millis(100));
                    first_value.store(1, Ordering::Release);
                    Ok(())
                })
                .await
        });
        while !started.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
        first.abort();
        let _ = first.await;

        let second_value = value.clone();
        operations
            .run("ordered_write", move || {
                second_value.store(2, Ordering::Release);
                Ok(())
            })
            .await
            .expect("ordered write");
        assert_eq!(value.load(Ordering::Acquire), 2);
    }
}
