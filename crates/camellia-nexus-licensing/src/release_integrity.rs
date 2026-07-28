use std::collections::{BTreeMap, BTreeSet};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{LicensingError, Result, version_policy::canonical_version};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateManifest {
    pub iss: String,
    pub aud: String,
    pub artifact_url: Url,
    pub version: String,
    pub sha256: String,
    pub minimum_supported_version: String,
    pub signing_key_id: String,
    #[serde(rename = "iat")]
    pub issued_at: i64,
    #[serde(rename = "exp")]
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUpdateManifest {
    pub manifest: UpdateManifest,
    pub version: Version,
    pub minimum_supported_version: Version,
}

pub struct TrustedUpdateKeys {
    keys: BTreeMap<String, DecodingKey>,
}

impl TrustedUpdateKeys {
    pub fn from_pem_keys<'a>(keys: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Result<Self> {
        let mut trusted = BTreeMap::new();
        for (key_id, pem) in keys {
            if key_id.trim().is_empty() || trusted.contains_key(key_id) {
                return Err(LicensingError::InvalidUpdateManifest);
            }
            trusted.insert(
                key_id.to_owned(),
                DecodingKey::from_ec_pem(pem).map_err(|_| LicensingError::InvalidUpdateManifest)?,
            );
        }
        Ok(Self { keys: trusted })
    }
}

#[derive(Debug, Clone)]
pub struct UpdateUrlPolicy {
    allowed_origins: BTreeSet<String>,
    required_path_prefix: String,
}

impl UpdateUrlPolicy {
    pub fn new(
        allowed_origins: impl IntoIterator<Item = String>,
        required_path_prefix: impl Into<String>,
    ) -> Result<Self> {
        let allowed_origins = allowed_origins.into_iter().collect::<BTreeSet<_>>();
        let required_path_prefix = required_path_prefix.into();
        if allowed_origins.is_empty() || !required_path_prefix.starts_with('/') {
            return Err(LicensingError::InvalidUpdateManifest);
        }
        Ok(Self {
            allowed_origins,
            required_path_prefix,
        })
    }

    fn allows(&self, url: &Url) -> bool {
        if url.scheme() != "https"
            || url.username() != ""
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query().is_some()
            || !url.path().starts_with(&self.required_path_prefix)
        {
            return false;
        }
        let origin = url.origin().ascii_serialization();
        self.allowed_origins.contains(&origin)
    }
}

pub struct UpdateManifestVerifier {
    issuer: String,
    audience: String,
    keys: TrustedUpdateKeys,
    url_policy: UpdateUrlPolicy,
}

impl UpdateManifestVerifier {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        keys: TrustedUpdateKeys,
        url_policy: UpdateUrlPolicy,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audience: audience.into(),
            keys,
            url_policy,
        }
    }

    pub fn verify(
        &self,
        compact_jws: &str,
        trusted_now: i64,
        current_version: &Version,
        highest_accepted_version: Option<&Version>,
    ) -> Result<VerifiedUpdateManifest> {
        if compact_jws.len() > 64 * 1024 || compact_jws.matches('.').count() != 2 {
            return Err(LicensingError::InvalidUpdateManifest);
        }
        let header =
            decode_header(compact_jws).map_err(|_| LicensingError::InvalidUpdateManifest)?;
        if header.alg != Algorithm::ES256 {
            return Err(LicensingError::UnsupportedAlgorithm);
        }
        let key_id = header
            .kid
            .as_deref()
            .ok_or(LicensingError::UnknownSigningKey)?;
        let key = self
            .keys
            .keys
            .get(key_id)
            .ok_or(LicensingError::UnknownSigningKey)?;
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        validation.leeway = 0;
        crate::es256_provider::ensure_installed();
        let manifest = decode::<UpdateManifest>(compact_jws, key, &validation)
            .map_err(|_| LicensingError::InvalidSignature)?
            .claims;
        if manifest.iss != self.issuer
            || manifest.aud != self.audience
            || manifest.signing_key_id != key_id
            || manifest.issued_at > trusted_now + 300
            || manifest.expires_at <= trusted_now
            || manifest.expires_at <= manifest.issued_at
            || !is_sha256(&manifest.sha256)
        {
            return Err(LicensingError::InvalidUpdateManifest);
        }
        if !self.url_policy.allows(&manifest.artifact_url) {
            return Err(LicensingError::UpdateUrlDenied);
        }
        let version =
            canonical_version(&manifest.version).ok_or(LicensingError::InvalidUpdateManifest)?;
        let minimum_supported_version = canonical_version(&manifest.minimum_supported_version)
            .ok_or(LicensingError::InvalidUpdateManifest)?;
        if version <= *current_version
            || highest_accepted_version.is_some_and(|highest| version < *highest)
            || minimum_supported_version > version
        {
            return Err(LicensingError::UpdateRollback);
        }
        Ok(VerifiedUpdateManifest {
            manifest,
            version,
            minimum_supported_version,
        })
    }

    pub fn verify_artifact(&self, manifest: &VerifiedUpdateManifest, bytes: &[u8]) -> Result<()> {
        let actual = hex(&Sha256::digest(bytes));
        if actual.eq_ignore_ascii_case(&manifest.manifest.sha256) {
            Ok(())
        } else {
            Err(LicensingError::ArtifactDigestMismatch)
        }
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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

    #[test]
    fn verifies_manifest_url_version_and_artifact_digest() {
        let key = SigningKey::try_generate().expect("test signing key entropy");
        let private = key.to_pkcs8_pem(Default::default()).unwrap();
        let public = key
            .verifying_key()
            .to_public_key_pem(Default::default())
            .unwrap();
        let artifact = b"signed artifact";
        let manifest = UpdateManifest {
            iss: "updates.example".into(),
            aud: "desktop-update".into(),
            artifact_url: Url::parse("https://releases.example/camellia/v1.1.0/app.exe").unwrap(),
            version: "1.1.0".into(),
            sha256: hex(&Sha256::digest(artifact)),
            minimum_supported_version: "1.0.0".into(),
            signing_key_id: "update-key".into(),
            issued_at: 1_000,
            expires_at: 2_000,
        };
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some("update-key".into());
        crate::es256_provider::ensure_installed();
        let token = encode(
            &header,
            &manifest,
            &EncodingKey::from_ec_pem(private.as_bytes()).unwrap(),
        )
        .unwrap();
        let verifier = UpdateManifestVerifier::new(
            "updates.example",
            "desktop-update",
            TrustedUpdateKeys::from_pem_keys([("update-key", public.as_bytes())]).unwrap(),
            UpdateUrlPolicy::new(["https://releases.example".into()], "/camellia/").unwrap(),
        );
        let verified = verifier
            .verify(&token, 1_100, &Version::parse("1.0.0").unwrap(), None)
            .unwrap();
        verifier.verify_artifact(&verified, artifact).unwrap();
        assert!(matches!(
            verifier.verify_artifact(&verified, b"tampered"),
            Err(LicensingError::ArtifactDigestMismatch)
        ));
    }
}
