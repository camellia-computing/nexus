use std::collections::{BTreeMap, BTreeSet};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

use crate::{
    ActivationProofClaims, Capability, ClientBuildIdentity, ClientVersionDisposition,
    EntitlementClaims, LicensingError, NumericLimit, Result, VerifiedActivationProof,
    VerifiedEntitlement, evaluate_client_version, validate_client_version_policy,
};

const WORKSPACE_PERMISSIONS: &[&str] = &[
    "alerts.ack",
    "alerts.history.read",
    "alerts.manage",
    "alerts.read",
    "audit.export",
    "audit.read",
    "billing.manage",
    "billing.read",
    "remote.read",
    "shared.publish",
    "shared.purge",
    "shared.read",
    "shared.write",
    "sync.read",
    "sync.write",
    "team.manage",
    "team.read",
    "team.transfer_ownership",
    "webhooks.delivery.read",
    "webhooks.manage",
    "webhooks.read",
];

fn workspace_authority_is_valid(claims: &EntitlementClaims) -> bool {
    if claims.plan != crate::Plan::Team {
        return claims.workspace_permissions.is_empty();
    }
    if claims.workspace_permissions.len() > WORKSPACE_PERMISSIONS.len()
        || !claims
            .workspace_permissions
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        || claims
            .workspace_permissions
            .iter()
            .any(|permission| !WORKSPACE_PERMISSIONS.contains(&permission.as_str()))
    {
        return false;
    }
    role_scoped_capabilities().into_iter().all(|capability| {
        claims.capabilities.contains(&capability)
            == claims
                .workspace_permissions
                .iter()
                .any(|permission| permission_grants_capability(permission, capability))
    })
}

const fn role_scoped_capabilities() -> [Capability; 7] {
    [
        Capability::CloudSync,
        Capability::RemoteDashboard,
        Capability::Alerts,
        Capability::SharedConfigurations,
        Capability::TeamAdministration,
        Capability::AuditLog,
        Capability::Webhooks,
    ]
}

fn permission_grants_capability(permission: &str, capability: Capability) -> bool {
    match capability {
        Capability::CloudSync => permission.starts_with("sync."),
        Capability::RemoteDashboard => permission == "remote.read",
        Capability::Alerts => permission.starts_with("alerts."),
        Capability::SharedConfigurations => permission.starts_with("shared."),
        Capability::TeamAdministration => {
            matches!(permission, "team.manage" | "team.transfer_ownership")
        }
        Capability::AuditLog => permission.starts_with("audit."),
        Capability::Webhooks => permission.starts_with("webhooks."),
        Capability::ManagedConfigSources
        | Capability::AdvancedDiagnostics
        | Capability::ManagedProgramPackages => false,
    }
}

fn signed_identifier_is_canonical(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn device_id_is_canonical(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|device_id| {
        device_id.get_version() == Some(uuid::Version::Random)
            && device_id.get_variant() == uuid::Variant::RFC4122
            && device_id.hyphenated().to_string() == value
    })
}

fn sha256_identifier_is_canonical(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone)]
pub struct EntitlementVerifierConfig {
    pub issuer: String,
    pub audience: String,
    pub device_id: String,
    pub device_key_thumbprint: String,
    pub minimum_license_epoch: u64,
    pub clock_skew_seconds: i64,
    pub client_build: ClientBuildIdentity,
}

#[derive(Clone)]
pub struct TrustedEntitlementKeys {
    keys: BTreeMap<String, DecodingKey>,
}

impl std::fmt::Debug for TrustedEntitlementKeys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TrustedEntitlementKeys")
            .field("key_ids", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl TrustedEntitlementKeys {
    pub fn from_pem_keys<'a>(keys: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Result<Self> {
        let mut trusted = BTreeMap::new();
        for (key_id, pem) in keys {
            if key_id.trim().is_empty() || trusted.contains_key(key_id) {
                return Err(LicensingError::InvalidClaims);
            }
            let key = DecodingKey::from_ec_pem(pem).map_err(|_| LicensingError::InvalidClaims)?;
            trusted.insert(key_id.to_owned(), key);
        }
        Ok(Self { keys: trusted })
    }

    fn get(&self, key_id: &str) -> Option<&DecodingKey> {
        self.keys.get(key_id)
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

pub struct EntitlementVerifier {
    config: EntitlementVerifierConfig,
    keys: TrustedEntitlementKeys,
}

pub struct ActivationProofVerifier {
    config: EntitlementVerifierConfig,
    keys: TrustedEntitlementKeys,
}

impl EntitlementVerifier {
    pub fn new(config: EntitlementVerifierConfig, keys: TrustedEntitlementKeys) -> Self {
        Self { config, keys }
    }

    pub fn verify(&self, compact_jws: &str, trusted_now: i64) -> Result<VerifiedEntitlement> {
        self.verify_internal(compact_jws, trusted_now, false)
    }

    pub fn verify_cached(
        &self,
        compact_jws: &str,
        trusted_now: i64,
    ) -> Result<VerifiedEntitlement> {
        self.verify_internal(compact_jws, trusted_now, true)
    }

    fn verify_internal(
        &self,
        compact_jws: &str,
        trusted_now: i64,
        allow_expired: bool,
    ) -> Result<VerifiedEntitlement> {
        if compact_jws.len() > 64 * 1024 || compact_jws.matches('.').count() != 2 {
            return Err(LicensingError::MalformedEntitlement);
        }
        let header =
            decode_header(compact_jws).map_err(|_| LicensingError::MalformedEntitlement)?;
        if header.alg != Algorithm::ES256 {
            return Err(LicensingError::UnsupportedAlgorithm);
        }
        let key_id = header
            .kid
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(LicensingError::UnknownSigningKey)?;
        let key = self
            .keys
            .get(key_id)
            .ok_or(LicensingError::UnknownSigningKey)?;
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        validation.leeway = 0;
        crate::es256_provider::ensure_installed();
        let claims = decode::<EntitlementClaims>(compact_jws, key, &validation)
            .map_err(|_| LicensingError::InvalidSignature)?
            .claims;
        self.validate_claims(&claims, key_id, trusted_now, allow_expired)?;
        Ok(VerifiedEntitlement {
            claims,
            key_id: key_id.to_owned(),
        })
    }

    fn validate_claims(
        &self,
        claims: &EntitlementClaims,
        header_key_id: &str,
        trusted_now: i64,
        allow_expired: bool,
    ) -> Result<()> {
        if claims.iss != self.config.issuer {
            return Err(LicensingError::WrongIssuer);
        }
        if claims.aud != self.config.audience {
            return Err(LicensingError::WrongAudience);
        }
        if claims.device_id != self.config.device_id {
            return Err(LicensingError::DeviceMismatch);
        }
        if claims.device_key_thumbprint != self.config.device_key_thumbprint {
            return Err(LicensingError::DeviceKeyMismatch);
        }
        if claims.key_id != header_key_id {
            return Err(LicensingError::UnknownSigningKey);
        }
        validate_client_version_policy(&claims.client_version_policy)?;
        let version_disposition = evaluate_client_version(
            &self.config.client_build,
            &claims.client_version_policy,
            trusted_now,
        )?;
        if version_disposition == ClientVersionDisposition::UpgradeRequired {
            return Err(LicensingError::ClientUpgradeRequired {
                policy: claims.client_version_policy.clone(),
            });
        }
        if version_disposition == ClientVersionDisposition::UpgradeRequiredBefore
            && claims.expires_at > claims.client_version_policy.enforce_after
        {
            return Err(LicensingError::InvalidClaims);
        }
        if claims.license_epoch < self.config.minimum_license_epoch {
            return Err(LicensingError::ObsoleteLicenseEpoch);
        }
        let skew = self.config.clock_skew_seconds.max(0);
        if claims.issued_at > trusted_now.saturating_add(skew)
            || claims.expires_at <= claims.issued_at
            || claims.refresh_after < claims.issued_at
            || claims.refresh_after > claims.expires_at
            || claims
                .license_expires_at
                .is_some_and(|expires_at| expires_at < claims.expires_at)
            || claims.offline_access_ends_at < claims.expires_at
            || claims.offline_access_ends_at > claims.issued_at.saturating_add(24 * 60 * 60)
            || claims
                .license_expires_at
                .is_some_and(|expires_at| expires_at < claims.offline_access_ends_at)
        {
            return Err(LicensingError::InvalidClaims);
        }
        let license_term_ended = claims
            .license_expires_at
            .is_some_and(|expires_at| expires_at <= trusted_now.saturating_sub(skew));
        match claims.license_status {
            crate::LicenseStanding::Active if license_term_ended => {
                return Err(LicensingError::LicenseExpired);
            }
            crate::LicenseStanding::PastDue
                if license_term_ended || claims.license_expires_at.is_none() =>
            {
                return Err(LicensingError::LicensePastDue);
            }
            crate::LicenseStanding::Canceled
                if license_term_ended || claims.license_expires_at.is_none() =>
            {
                return Err(LicensingError::LicenseCanceled);
            }
            _ => {}
        }
        if !allow_expired && claims.expires_at <= trusted_now.saturating_sub(skew) {
            return Err(LicensingError::EntitlementExpired);
        }
        if !signed_identifier_is_canonical(&claims.sub, 128)
            || !signed_identifier_is_canonical(&claims.license_id, 128)
            || !signed_identifier_is_canonical(&claims.token_id, 128)
            || !signed_identifier_is_canonical(&claims.key_id, 128)
            || !device_id_is_canonical(&claims.device_id)
            || !sha256_identifier_is_canonical(&claims.device_key_thumbprint)
            || claims.schema_version != 3
            || claims.plan_revision != 2
            || claims.policy_hash.len() != 64
            || !claims
                .policy_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || claims.device_limit == 0
            || claims.member_limit == 0
            || claims.capability_set().len() != claims.capabilities.len()
            || !workspace_authority_is_valid(claims)
            || claims
                .limits
                .values()
                .any(|value| *value > 1_099_511_627_776)
            || [
                NumericLimit::MaxPrograms,
                NumericLimit::MaxConfigSourcesPerProgram,
                NumericLimit::MaxTeamMembers,
                NumericLimit::MaxRemoteMonitors,
                NumericLimit::MaxSharedPrograms,
                NumericLimit::MaxWebhookEndpoints,
                NumericLimit::MaxWorkspaceStorageBytes,
                NumericLimit::MaxAlertRules,
                NumericLimit::MaxAuditExportEvents,
            ]
            .into_iter()
            .any(|limit| !claims.limits.contains_key(&limit))
        {
            return Err(LicensingError::InvalidClaims);
        }
        Ok(())
    }
}

impl ActivationProofVerifier {
    pub fn new(config: EntitlementVerifierConfig, keys: TrustedEntitlementKeys) -> Self {
        Self { config, keys }
    }

    pub fn verify(&self, compact_jws: &str, trusted_now: i64) -> Result<VerifiedActivationProof> {
        if compact_jws.len() > 64 * 1024 || compact_jws.matches('.').count() != 2 {
            return Err(LicensingError::MalformedEntitlement);
        }
        let header =
            decode_header(compact_jws).map_err(|_| LicensingError::MalformedEntitlement)?;
        if header.alg != Algorithm::ES256 {
            return Err(LicensingError::UnsupportedAlgorithm);
        }
        let key_id = header
            .kid
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(LicensingError::UnknownSigningKey)?;
        let key = self
            .keys
            .get(key_id)
            .ok_or(LicensingError::UnknownSigningKey)?;
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        validation.leeway = 0;
        crate::es256_provider::ensure_installed();
        let claims = decode::<ActivationProofClaims>(compact_jws, key, &validation)
            .map_err(|_| LicensingError::InvalidSignature)?
            .claims;
        self.validate_claims(&claims, key_id, trusted_now)?;
        Ok(VerifiedActivationProof {
            claims,
            key_id: key_id.to_owned(),
        })
    }

    fn validate_claims(
        &self,
        claims: &ActivationProofClaims,
        header_key_id: &str,
        trusted_now: i64,
    ) -> Result<()> {
        if claims.iss != self.config.issuer {
            return Err(LicensingError::WrongIssuer);
        }
        if claims.aud != self.config.audience {
            return Err(LicensingError::WrongAudience);
        }
        if claims.device_id != self.config.device_id {
            return Err(LicensingError::DeviceMismatch);
        }
        if claims.device_key_thumbprint != self.config.device_key_thumbprint {
            return Err(LicensingError::DeviceKeyMismatch);
        }
        if claims.key_id != header_key_id {
            return Err(LicensingError::UnknownSigningKey);
        }
        if claims.license_epoch < self.config.minimum_license_epoch {
            return Err(LicensingError::ObsoleteLicenseEpoch);
        }
        let skew = self.config.clock_skew_seconds.max(0);
        if claims.issued_at > trusted_now.saturating_add(skew)
            || claims.expires_at <= claims.issued_at
            || claims.expires_at <= trusted_now.saturating_sub(skew)
            || claims.purpose != "activation_verify"
            || claims.sub.trim().is_empty()
            || claims.license_id.trim().is_empty()
            || claims.token_id.trim().is_empty()
            || claims.device_id.trim().is_empty()
            || claims.device_key_thumbprint.trim().is_empty()
        {
            return Err(LicensingError::InvalidClaims);
        }
        Ok(())
    }
}

pub fn capabilities(values: &[Capability]) -> BTreeSet<Capability> {
    values.iter().copied().collect()
}

pub fn limits(values: &[(NumericLimit, u64)]) -> BTreeMap<NumericLimit, u64> {
    values.iter().copied().collect()
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use p256::{
        ecdsa::SigningKey,
        elliptic_curve::Generate,
        pkcs8::{EncodePrivateKey, EncodePublicKey},
    };

    use super::*;
    use crate::Plan;

    struct Fixture {
        verifier: EntitlementVerifier,
        encoding: EncodingKey,
        claims: EntitlementClaims,
    }

    fn fixture() -> Fixture {
        let key = SigningKey::try_generate().expect("test signing key entropy");
        let private = key.to_pkcs8_pem(Default::default()).expect("private pem");
        let public = key
            .verifying_key()
            .to_public_key_pem(Default::default())
            .expect("public pem");
        let claims = EntitlementClaims {
            schema_version: 3,
            iss: "https://license.example".into(),
            aud: "desktop".into(),
            sub: "account-1".into(),
            license_id: "license-1".into(),
            device_id: "11111111-1111-4111-8111-111111111111".into(),
            device_key_thumbprint: format!("sha256:{}", "a".repeat(64)),
            plan: Plan::Pro,
            plan_revision: 2,
            policy_hash: "0".repeat(64),
            license_status: crate::LicenseStanding::Active,
            capabilities: vec![Capability::ManagedConfigSources],
            workspace_permissions: Vec::new(),
            limits: limits(&[
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
            license_epoch: 4,
            device_limit: 3,
            member_limit: 1,
            issued_at: 1_000,
            refresh_after: 1_100,
            expires_at: 2_000,
            offline_access_ends_at: 3_000,
            token_id: "lease-1".into(),
            key_id: "key-2026".into(),
        };
        let verifier = EntitlementVerifier::new(
            EntitlementVerifierConfig {
                issuer: claims.iss.clone(),
                audience: claims.aud.clone(),
                device_id: claims.device_id.clone(),
                device_key_thumbprint: claims.device_key_thumbprint.clone(),
                minimum_license_epoch: 4,
                clock_skew_seconds: 0,
                client_build: crate::ClientBuildIdentity::parse("1.0.0").unwrap(),
            },
            TrustedEntitlementKeys::from_pem_keys([("key-2026", public.as_bytes())]).expect("keys"),
        );
        Fixture {
            verifier,
            encoding: EncodingKey::from_ec_pem(private.as_bytes()).expect("encoding"),
            claims,
        }
    }

    fn token(fixture: &Fixture, claims: &EntitlementClaims) -> String {
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("key-2026".into());
        crate::es256_provider::ensure_installed();
        encode(&header, claims, &fixture.encoding).expect("token")
    }

    fn unsigned_token_for_test(algorithm: Algorithm, claims: &EntitlementClaims) -> String {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

        let mut header = Header::new(algorithm);
        header.kid = Some("key-2026".into());
        format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header")),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims"))
        )
    }

    #[test]
    fn verifies_valid_es256_entitlement() {
        let fixture = fixture();
        let verified = fixture
            .verifier
            .verify(&token(&fixture, &fixture.claims), 1_200)
            .expect("verified");
        assert_eq!(verified.claims.license_id, "license-1");
    }

    #[test]
    fn rejects_wrong_issuer_audience_device_thumbprint_and_epoch() {
        type InvalidCase = (fn(&mut EntitlementClaims), LicensingError);
        let fixture = fixture();
        let cases: Vec<InvalidCase> = vec![
            (
                |claims| claims.iss = "wrong".into(),
                LicensingError::WrongIssuer,
            ),
            (
                |claims| claims.aud = "wrong".into(),
                LicensingError::WrongAudience,
            ),
            (
                |claims| claims.device_id = "wrong".into(),
                LicensingError::DeviceMismatch,
            ),
            (
                |claims| claims.device_key_thumbprint = "wrong".into(),
                LicensingError::DeviceKeyMismatch,
            ),
            (
                |claims| claims.license_epoch = 3,
                LicensingError::ObsoleteLicenseEpoch,
            ),
        ];
        for (mutate, expected) in cases {
            let mut claims = fixture.claims.clone();
            mutate(&mut claims);
            let error = fixture
                .verifier
                .verify(&token(&fixture, &claims), 1_200)
                .expect_err("rejected");
            assert_eq!(
                std::mem::discriminant(&error),
                std::mem::discriminant(&expected)
            );
        }
    }

    #[test]
    fn rejects_expired_duplicate_capability_and_invalid_signature() {
        let fixture = fixture();
        assert!(matches!(
            fixture
                .verifier
                .verify(&token(&fixture, &fixture.claims), 2_100),
            Err(LicensingError::EntitlementExpired)
        ));
        let mut duplicate = fixture.claims.clone();
        duplicate
            .capabilities
            .push(Capability::ManagedConfigSources);
        assert!(matches!(
            fixture.verifier.verify(&token(&fixture, &duplicate), 1_200),
            Err(LicensingError::InvalidClaims)
        ));
        let mut tampered = token(&fixture, &fixture.claims);
        tampered.push('x');
        assert!(matches!(
            fixture.verifier.verify(&tampered, 1_200),
            Err(LicensingError::InvalidSignature)
        ));
    }

    #[test]
    fn rejects_noncurrent_plan_revision_and_noncanonical_policy_hash() {
        let fixture = fixture();
        let mut claims = fixture.claims.clone();
        claims.plan_revision = 1;
        assert!(matches!(
            fixture.verifier.verify(&token(&fixture, &claims), 1_200),
            Err(LicensingError::InvalidClaims)
        ));

        claims.plan_revision = 2;
        claims.policy_hash = "A".repeat(64);
        assert!(matches!(
            fixture.verifier.verify(&token(&fixture, &claims), 1_200),
            Err(LicensingError::InvalidClaims)
        ));
    }

    #[test]
    fn requires_role_scoped_team_capabilities_to_match_signed_permissions() {
        let fixture = fixture();
        let mut claims = fixture.claims.clone();
        claims.plan = Plan::Team;
        claims.capabilities.push(Capability::CloudSync);
        claims.workspace_permissions = vec!["sync.read".to_owned()];
        assert!(
            fixture
                .verifier
                .verify(&token(&fixture, &claims), 1_500)
                .is_ok()
        );

        claims.workspace_permissions.clear();
        assert!(matches!(
            fixture.verifier.verify(&token(&fixture, &claims), 1_500),
            Err(LicensingError::InvalidClaims)
        ));

        claims
            .capabilities
            .retain(|capability| *capability != Capability::CloudSync);
        claims.workspace_permissions = vec!["unknown.permission".to_owned()];
        assert!(matches!(
            fixture.verifier.verify(&token(&fixture, &claims), 1_500),
            Err(LicensingError::InvalidClaims)
        ));

        claims.plan = Plan::Pro;
        claims.workspace_permissions = vec!["sync.read".to_owned()];
        assert!(matches!(
            fixture.verifier.verify(&token(&fixture, &claims), 1_500),
            Err(LicensingError::InvalidClaims)
        ));
    }

    #[test]
    fn rejects_entitlement_when_license_expiry_is_before_lease_expiry() {
        let fixture = fixture();
        let mut claims = fixture.claims.clone();
        claims.license_expires_at = Some(claims.expires_at - 1);
        assert!(matches!(
            fixture.verifier.verify(&token(&fixture, &claims), 1_200),
            Err(LicensingError::InvalidClaims)
        ));
    }

    #[test]
    fn rejects_signed_inactive_license_standings() {
        let fixture = fixture();
        for (standing, expected) in [
            (
                crate::LicenseStanding::PastDue,
                LicensingError::LicensePastDue,
            ),
            (
                crate::LicenseStanding::Canceled,
                LicensingError::LicenseCanceled,
            ),
        ] {
            let mut claims = fixture.claims.clone();
            claims.license_status = standing;
            let compact_jws = token(&fixture, &claims);
            assert!(matches!(
                fixture.verifier.verify(&compact_jws, 1_200),
                Err(error) if std::mem::discriminant(&error) == std::mem::discriminant(&expected)
            ));
            assert!(matches!(
                fixture.verifier.verify_cached(&compact_jws, 1_200),
                Err(error) if std::mem::discriminant(&error) == std::mem::discriminant(&expected)
            ));
        }
    }

    #[test]
    fn accepts_signed_non_active_standing_only_during_an_explicit_grace_term() {
        let fixture = fixture();
        for standing in [
            crate::LicenseStanding::PastDue,
            crate::LicenseStanding::Canceled,
        ] {
            let mut claims = fixture.claims.clone();
            claims.license_status = standing;
            claims.license_expires_at = Some(2_200);
            claims.offline_access_ends_at = 2_200;
            let compact_jws = token(&fixture, &claims);
            fixture
                .verifier
                .verify(&compact_jws, 1_200)
                .expect("grace term is usable");
            assert!(fixture.verifier.verify_cached(&compact_jws, 2_200).is_err());
        }
    }

    #[test]
    fn rejects_wrong_algorithm_and_unknown_key_id() {
        let fixture = fixture();
        let hs = unsigned_token_for_test(Algorithm::HS256, &fixture.claims);
        assert!(matches!(
            fixture.verifier.verify(&hs, 1_200),
            Err(LicensingError::UnsupportedAlgorithm)
        ));
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("unknown".into());
        crate::es256_provider::ensure_installed();
        let unknown = encode(&header, &fixture.claims, &fixture.encoding).expect("token");
        assert!(matches!(
            fixture.verifier.verify(&unknown, 1_200),
            Err(LicensingError::UnknownSigningKey)
        ));
    }
}
