use semver::Version;

use crate::{ClientVersionPolicy, LicensingError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientBuildIdentity {
    version: Version,
    wire_version: String,
}

impl ClientBuildIdentity {
    pub fn parse(value: &str) -> Result<Self> {
        let version = canonical_version(value).ok_or(LicensingError::InvalidClientBuild)?;
        Ok(Self {
            wire_version: value.to_owned(),
            version,
        })
    }

    pub fn wire_version(&self) -> &str {
        &self.wire_version
    }

    pub fn version(&self) -> &Version {
        &self.version
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientVersionDisposition {
    Current,
    UpgradeRecommended,
    UpgradeRequiredBefore,
    UpgradeRequired,
}

pub fn evaluate_client_version(
    build: &ClientBuildIdentity,
    policy: &ClientVersionPolicy,
    trusted_now: i64,
) -> Result<ClientVersionDisposition> {
    let (minimum, recommended) = validate_client_version_policy(policy)?;
    if build.version() < &minimum {
        return Ok(if trusted_now >= policy.enforce_after {
            ClientVersionDisposition::UpgradeRequired
        } else {
            ClientVersionDisposition::UpgradeRequiredBefore
        });
    }
    Ok(if build.version() < &recommended {
        ClientVersionDisposition::UpgradeRecommended
    } else {
        ClientVersionDisposition::Current
    })
}

pub(crate) fn validate_client_version_policy(
    policy: &ClientVersionPolicy,
) -> Result<(Version, Version)> {
    let minimum =
        canonical_version(&policy.minimum_version).ok_or(LicensingError::InvalidClaims)?;
    let recommended =
        canonical_version(&policy.recommended_version).ok_or(LicensingError::InvalidClaims)?;
    // Unix epoch zero is a valid way to express a policy that is already enforced.
    if policy.enforce_after < 0 || recommended < minimum {
        return Err(LicensingError::InvalidClaims);
    }
    Ok((minimum, recommended))
}

pub(crate) fn canonical_version(value: &str) -> Option<Version> {
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    let version = Version::parse(value).ok()?;
    (version.to_string() == value).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(enforce_after: i64) -> ClientVersionPolicy {
        ClientVersionPolicy {
            minimum_version: "2.0.0".into(),
            recommended_version: "2.1.0".into(),
            enforce_after,
        }
    }

    #[test]
    fn build_and_policy_versions_are_canonical() {
        assert!(ClientBuildIdentity::parse("1.2.3").is_ok());
        for invalid in [
            "",
            "v1.2.3",
            "01.2.3",
            " 1.2.3",
            "1.2",
            "1.0.0+this-build-identifier-is-deliberately-longer-than-sixty-four-bytes",
        ] {
            assert!(matches!(
                ClientBuildIdentity::parse(invalid),
                Err(LicensingError::InvalidClientBuild)
            ));
        }
        let invalid_policy = ClientVersionPolicy {
            minimum_version: "2.0.0".into(),
            recommended_version: "1.9.0".into(),
            enforce_after: 100,
        };
        assert!(matches!(
            validate_client_version_policy(&invalid_policy),
            Err(LicensingError::InvalidClaims)
        ));
        let already_enforced = ClientVersionPolicy {
            minimum_version: "1.0.0".into(),
            recommended_version: "1.0.0".into(),
            enforce_after: 0,
        };
        assert!(validate_client_version_policy(&already_enforced).is_ok());
        let invalid_time = ClientVersionPolicy {
            enforce_after: -1,
            ..already_enforced
        };
        assert!(matches!(
            validate_client_version_policy(&invalid_time),
            Err(LicensingError::InvalidClaims)
        ));
    }

    #[test]
    fn enforcement_boundary_is_exact() {
        let old = ClientBuildIdentity::parse("1.0.0").unwrap();
        assert_eq!(
            evaluate_client_version(&old, &policy(100), 99).unwrap(),
            ClientVersionDisposition::UpgradeRequiredBefore
        );
        assert_eq!(
            evaluate_client_version(&old, &policy(100), 100).unwrap(),
            ClientVersionDisposition::UpgradeRequired
        );
        let supported = ClientBuildIdentity::parse("2.0.0").unwrap();
        assert_eq!(
            evaluate_client_version(&supported, &policy(100), 100).unwrap(),
            ClientVersionDisposition::UpgradeRecommended
        );
    }
}
