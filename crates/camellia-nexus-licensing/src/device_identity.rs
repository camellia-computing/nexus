use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::{
    ecdsa::{Signature, SigningKey, signature::Signer},
    elliptic_curve::Generate,
    pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    DeviceRegistrationMetadata, DynSecureStore, LicensingError, Result, SecretKey, SecureStoreMode,
    get_json, put_json,
};

const PROOF_DOMAIN: &[u8] = b"camellia.nexus.device-proof.v1";
static DEVICE_IDENTITY_MUTATION: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceChallenge {
    pub challenge_id: String,
    pub nonce: String,
    pub requested_scope: String,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProof {
    pub challenge_id: String,
    pub device_id: String,
    pub app_version: String,
    pub requested_scope: String,
    pub issued_at: i64,
    pub signature: String,
}

pub struct DeviceIdentity {
    pub metadata: DeviceRegistrationMetadata,
    signing_key: SigningKey,
    consumed_challenges: Mutex<HashSet<String>>,
}

impl std::fmt::Debug for DeviceIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceIdentity")
            .field("device_id", &self.metadata.device_id)
            .field(
                "public_key_thumbprint",
                &self.metadata.public_key_thumbprint,
            )
            .finish_non_exhaustive()
    }
}

impl DeviceIdentity {
    pub fn sign_challenge(
        &self,
        challenge: &DeviceChallenge,
        now: i64,
        app_version: &str,
    ) -> Result<DeviceProof> {
        if challenge.challenge_id.trim().is_empty()
            || challenge.nonce.len() < 16
            || challenge.requested_scope.trim().is_empty()
            || challenge.issued_at > now + 60
            || challenge.expires_at <= now
            || challenge.expires_at <= challenge.issued_at
        {
            return Err(LicensingError::InvalidChallenge);
        }
        let mut consumed = self
            .consumed_challenges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !consumed.insert(challenge.challenge_id.clone()) {
            return Err(LicensingError::ChallengeReplay);
        }
        if crate::ClientBuildIdentity::parse(app_version).is_err() {
            return Err(LicensingError::InvalidClientBuild);
        }
        let payload = proof_payload(
            &challenge.nonce,
            &self.metadata.device_id,
            app_version,
            &challenge.requested_scope,
            challenge.issued_at,
        );
        let signature: Signature = self.signing_key.sign(&payload);
        Ok(DeviceProof {
            challenge_id: challenge.challenge_id.clone(),
            device_id: self.metadata.device_id.clone(),
            app_version: app_version.to_owned(),
            requested_scope: challenge.requested_scope.clone(),
            issued_at: challenge.issued_at,
            signature: URL_SAFE_NO_PAD.encode(signature.to_bytes()),
        })
    }
}

pub struct DeviceIdentityProvider {
    store: DynSecureStore,
    key_provider: Arc<dyn DeviceKeyProvider>,
}

pub trait DeviceKeyProvider: Send + Sync {
    fn load_signing_key(&self) -> Result<Option<SigningKey>>;
    fn store_signing_key(&self, key: &SigningKey) -> Result<()>;
    fn delete_signing_key(&self) -> Result<()>;
}

pub struct SecureStoreDeviceKeyProvider {
    store: DynSecureStore,
}

impl SecureStoreDeviceKeyProvider {
    pub fn new(store: DynSecureStore) -> Self {
        Self { store }
    }
}

impl DeviceKeyProvider for SecureStoreDeviceKeyProvider {
    fn load_signing_key(&self) -> Result<Option<SigningKey>> {
        self.store
            .get_secret(SecretKey::DevicePrivateKey)?
            .map(|private| {
                let private = Zeroizing::new(private);
                SigningKey::from_pkcs8_der(&private).map_err(|_| LicensingError::SecureStoreCorrupt)
            })
            .transpose()
    }

    fn store_signing_key(&self, key: &SigningKey) -> Result<()> {
        let private = Zeroizing::new(
            key.to_pkcs8_der()
                .map_err(|_| LicensingError::DeviceIdentityUnavailable)?
                .as_bytes()
                .to_vec(),
        );
        self.store.put_secret(SecretKey::DevicePrivateKey, &private)
    }

    fn delete_signing_key(&self) -> Result<()> {
        self.store.delete_secret(SecretKey::DevicePrivateKey)
    }
}

pub struct DeviceProofVerifier {
    verifying_key: p256::ecdsa::VerifyingKey,
    consumed_challenges: Mutex<HashSet<String>>,
}

impl DeviceProofVerifier {
    pub fn from_public_key_pem(pem: &str) -> Result<Self> {
        Ok(Self {
            verifying_key: p256::ecdsa::VerifyingKey::from_public_key_pem(pem)
                .map_err(|_| LicensingError::InvalidChallenge)?,
            consumed_challenges: Mutex::new(HashSet::new()),
        })
    }

    pub fn verify_and_consume(
        &self,
        challenge: &DeviceChallenge,
        proof: &DeviceProof,
        expected_device_id: &str,
        now: i64,
    ) -> Result<()> {
        use p256::ecdsa::signature::Verifier;

        if proof.challenge_id != challenge.challenge_id
            || proof.device_id != expected_device_id
            || proof.requested_scope != challenge.requested_scope
            || proof.issued_at != challenge.issued_at
            || challenge.expires_at <= now
        {
            return Err(LicensingError::InvalidChallenge);
        }
        let signature = URL_SAFE_NO_PAD
            .decode(&proof.signature)
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .ok_or(LicensingError::InvalidChallenge)?;
        let payload = proof_payload(
            &challenge.nonce,
            &proof.device_id,
            &proof.app_version,
            &proof.requested_scope,
            proof.issued_at,
        );
        self.verifying_key
            .verify(&payload, &signature)
            .map_err(|_| LicensingError::InvalidChallenge)?;
        let mut consumed = self
            .consumed_challenges
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !consumed.insert(challenge.challenge_id.clone()) {
            return Err(LicensingError::ChallengeReplay);
        }
        Ok(())
    }
}

impl DeviceIdentityProvider {
    pub fn new(store: DynSecureStore) -> Self {
        Self {
            key_provider: Arc::new(SecureStoreDeviceKeyProvider::new(store.clone())),
            store,
        }
    }

    pub fn with_key_provider(
        store: DynSecureStore,
        key_provider: Arc<dyn DeviceKeyProvider>,
    ) -> Self {
        Self {
            store,
            key_provider,
        }
    }

    pub fn load(&self) -> Result<Option<DeviceIdentity>> {
        let metadata: Option<DeviceRegistrationMetadata> =
            get_json(self.store.as_ref(), SecretKey::DeviceRegistration)?;
        let signing_key = self.key_provider.load_signing_key()?;
        match (metadata, signing_key) {
            (None, None) => Ok(None),
            (Some(metadata), Some(signing_key)) => {
                validate_metadata_key(&metadata, &signing_key)?;
                Ok(Some(DeviceIdentity {
                    metadata,
                    signing_key,
                    consumed_challenges: Mutex::new(HashSet::new()),
                }))
            }
            _ => Err(LicensingError::SecureStoreCorrupt),
        }
    }

    pub fn load_or_create(
        &self,
        platform: impl Into<String>,
        app_version: impl Into<String>,
        display_name: Option<String>,
    ) -> Result<DeviceIdentity> {
        let platform = platform.into();
        let app_version = app_version.into();
        let _mutation = DEVICE_IDENTITY_MUTATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(mut identity) = self.load()? {
            if identity.metadata.platform != platform
                || identity.metadata.app_version != app_version
                || identity.metadata.display_name != display_name
            {
                identity.metadata.platform = platform;
                identity.metadata.app_version = app_version;
                identity.metadata.display_name = display_name;
                put_json(
                    self.store.as_ref(),
                    SecretKey::DeviceRegistration,
                    &identity.metadata,
                )?;
            }
            return Ok(identity);
        }
        let signing_key =
            SigningKey::try_generate().map_err(|_| LicensingError::EntropyUnavailable)?;
        let public_der = signing_key
            .verifying_key()
            .to_public_key_der()
            .map_err(|_| LicensingError::DeviceIdentityUnavailable)?;
        let public_key_pem = signing_key
            .verifying_key()
            .to_public_key_pem(Default::default())
            .map_err(|_| LicensingError::DeviceIdentityUnavailable)?;
        let metadata = DeviceRegistrationMetadata {
            device_id: uuid::Uuid::new_v4().to_string(),
            public_key_pem,
            public_key_thumbprint: format!("sha256:{}", hex(&Sha256::digest(public_der.as_ref()))),
            platform,
            app_version,
            display_name,
        };
        self.key_provider.store_signing_key(&signing_key)?;
        if let Err(error) = put_json(
            self.store.as_ref(),
            SecretKey::DeviceRegistration,
            &metadata,
        ) {
            let _ = self.key_provider.delete_signing_key();
            return Err(error);
        }
        Ok(DeviceIdentity {
            metadata,
            signing_key,
            consumed_challenges: Mutex::new(HashSet::new()),
        })
    }

    pub fn reset_identity(&self) -> Result<()> {
        let _mutation = DEVICE_IDENTITY_MUTATION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.key_provider.delete_signing_key()?;
        self.store.delete_secret(SecretKey::DeviceRegistration)
    }

    pub fn supports_offline_continuity(&self) -> bool {
        self.store.mode() == SecureStoreMode::Persistent
    }
}

pub fn proof_payload(
    nonce: &str,
    device_id: &str,
    app_version: &str,
    requested_scope: &str,
    issued_at: i64,
) -> Vec<u8> {
    let mut output = Vec::with_capacity(160);
    output.extend_from_slice(PROOF_DOMAIN);
    for value in [nonce, device_id, app_version, requested_scope] {
        let bytes = value.as_bytes();
        output.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        output.extend_from_slice(bytes);
    }
    output.extend_from_slice(&issued_at.to_be_bytes());
    output
}

fn validate_metadata_key(metadata: &DeviceRegistrationMetadata, key: &SigningKey) -> Result<()> {
    let public_der = key
        .verifying_key()
        .to_public_key_der()
        .map_err(|_| LicensingError::SecureStoreCorrupt)?;
    let thumbprint = format!("sha256:{}", hex(&Sha256::digest(public_der.as_ref())));
    if metadata.public_key_thumbprint != thumbprint {
        return Err(LicensingError::SecureStoreCorrupt);
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{SecureStore, SessionSecureStore};

    #[test]
    fn generates_stores_and_reloads_a_device_identity() {
        let store = Arc::new(SessionSecureStore::default());
        let provider = DeviceIdentityProvider::new(store);
        let first = provider
            .load_or_create("test", "1.0.0", Some("Test device".into()))
            .expect("create");
        let second = provider.load().expect("load").expect("identity");
        assert_eq!(first.metadata, second.metadata);
        assert!(!provider.supports_offline_continuity());
    }

    #[test]
    fn refreshes_mutable_metadata_without_rotating_the_device_identity() {
        let store = Arc::new(SessionSecureStore::default());
        let provider = DeviceIdentityProvider::new(store);
        let original = provider
            .load_or_create("Windows", "1.0.0", Some("Old name".into()))
            .expect("create");
        let stable_identity = (
            original.metadata.device_id.clone(),
            original.metadata.public_key_pem.clone(),
            original.metadata.public_key_thumbprint.clone(),
        );

        let updated = provider
            .load_or_create("Windows", "2.0.0", Some("New name".into()))
            .expect("update");
        assert_eq!(
            (
                updated.metadata.device_id.clone(),
                updated.metadata.public_key_pem.clone(),
                updated.metadata.public_key_thumbprint.clone(),
            ),
            stable_identity
        );
        assert_eq!(updated.metadata.app_version, "2.0.0");
        assert_eq!(updated.metadata.display_name.as_deref(), Some("New name"));
        assert_eq!(
            provider.load().unwrap().unwrap().metadata,
            updated.metadata,
            "updated metadata must survive restart"
        );

        let proof = updated
            .sign_challenge(
                &DeviceChallenge {
                    challenge_id: "updated-version".into(),
                    nonce: "0123456789abcdef".into(),
                    requested_scope: "entitlement:refresh".into(),
                    issued_at: 100,
                    expires_at: 160,
                },
                120,
                "2.0.0",
            )
            .expect("proof");
        assert_eq!(
            proof.app_version, "2.0.0",
            "proofs report the explicitly injected desktop build"
        );
    }

    #[test]
    fn rejects_challenge_replay_and_expiry() {
        let provider = DeviceIdentityProvider::new(Arc::new(SessionSecureStore::default()));
        let identity = provider
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        let challenge = DeviceChallenge {
            challenge_id: "challenge-1".into(),
            nonce: "0123456789abcdef".into(),
            requested_scope: "entitlement:refresh".into(),
            issued_at: 100,
            expires_at: 160,
        };
        identity
            .sign_challenge(&challenge, 120, "1.0.0")
            .expect("proof");
        assert!(matches!(
            identity.sign_challenge(&challenge, 120, "1.0.0"),
            Err(LicensingError::ChallengeReplay)
        ));
        let expired = DeviceChallenge {
            challenge_id: "challenge-2".into(),
            ..challenge
        };
        assert!(matches!(
            identity.sign_challenge(&expired, 200, "1.0.0"),
            Err(LicensingError::InvalidChallenge)
        ));
    }

    #[test]
    fn server_side_verifier_rejects_invalid_proof_and_replay() {
        let provider = DeviceIdentityProvider::new(Arc::new(SessionSecureStore::default()));
        let identity = provider
            .load_or_create("test", "1.0.0", None)
            .expect("identity");
        let challenge = DeviceChallenge {
            challenge_id: "challenge-verify".into(),
            nonce: "0123456789abcdef".into(),
            requested_scope: "entitlement:refresh".into(),
            issued_at: 100,
            expires_at: 160,
        };
        let proof = identity
            .sign_challenge(&challenge, 120, "1.0.0")
            .expect("proof");
        let verifier = DeviceProofVerifier::from_public_key_pem(&identity.metadata.public_key_pem)
            .expect("verifier");
        let mut invalid = proof.clone();
        invalid.app_version = "tampered".into();
        assert!(matches!(
            verifier.verify_and_consume(&challenge, &invalid, &identity.metadata.device_id, 120,),
            Err(LicensingError::InvalidChallenge)
        ));
        verifier
            .verify_and_consume(&challenge, &proof, &identity.metadata.device_id, 120)
            .expect("valid proof");
        assert!(matches!(
            verifier.verify_and_consume(&challenge, &proof, &identity.metadata.device_id, 120,),
            Err(LicensingError::ChallengeReplay)
        ));
    }

    #[test]
    fn corrupt_identity_can_be_reset_without_touching_other_secrets() {
        let store = Arc::new(SessionSecureStore::default());
        store
            .put_secret(SecretKey::DevicePrivateKey, b"invalid-key")
            .expect("private");
        store
            .put_secret(SecretKey::RefreshSession, b"unrelated-session")
            .expect("session");
        let provider = DeviceIdentityProvider::new(store.clone());
        assert!(matches!(
            provider.load(),
            Err(LicensingError::SecureStoreCorrupt)
        ));
        provider.reset_identity().expect("reset");
        assert!(provider.load().expect("load").is_none());
        assert_eq!(
            store
                .get_secret(SecretKey::RefreshSession)
                .expect("session"),
            Some(b"unrelated-session".to_vec())
        );
    }

    #[test]
    fn concurrent_creation_returns_one_install_identity() {
        let store = Arc::new(SessionSecureStore::default());
        let handles = (0..2)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    DeviceIdentityProvider::new(store)
                        .load_or_create("test", "1.0.0", None)
                        .expect("identity")
                        .metadata
                })
            })
            .collect::<Vec<_>>();
        let identities = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread"))
            .collect::<Vec<_>>();
        assert_eq!(identities[0], identities[1]);
    }
}
