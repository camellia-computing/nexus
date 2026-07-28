use std::sync::Arc;

use camellia_nexus_licensing::{
    AuthorizationRequest, AuthorizationService, ClientBuildIdentity, DynSecureStore,
    HttpLicenseApi, LicensingAuthority, LicensingError, OAuthConfig, OsSecureStore, SecretKey,
    SecureStore, SessionSecureStore, TrustedEntitlementKeys, begin_authorization,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddedAuthority {
    issuer: String,
    audience: String,
    minimum_license_epoch: u64,
    keys: Vec<EmbeddedKey>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EmbeddedKey {
    key_id: String,
    public_key_pem: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LicenseServiceSettings {
    pub(crate) configured: bool,
    pub(crate) base_url: Option<String>,
    pub(crate) loopback_development: bool,
    pub(crate) authorization_configured: bool,
    pub(crate) authorization_endpoint: Option<String>,
    pub(crate) redirect_uri: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LicenseAuthorizationRequest {
    pub(crate) authorization_url: String,
    pub(crate) state: String,
    pub(crate) callback_mode: &'static str,
    pub(crate) suggested_device_name: String,
}

pub(crate) struct LicenseAuthorizationSession {
    pub(crate) request: LicenseAuthorizationRequest,
    pub(crate) pkce_verifier: camellia_nexus_licensing::SecretValue,
    pub(crate) redirect_uri: String,
}

pub(crate) fn initialize() -> Arc<AuthorizationService> {
    let authority = embedded_authority();
    let keys = TrustedEntitlementKeys::from_pem_keys(
        authority
            .keys
            .iter()
            .map(|key| (key.key_id.as_str(), key.public_key_pem.as_bytes())),
    )
    .expect("embedded entitlement public keys must be valid ES256 keys");
    let secure_store = secure_store();
    Arc::new(AuthorizationService::initialize(
        secure_store,
        LicensingAuthority {
            issuer: authority.issuer,
            audience: authority.audience,
            minimum_license_epoch: authority.minimum_license_epoch,
            keys,
        },
        ClientBuildIdentity::parse(env!("CARGO_PKG_VERSION"))
            .expect("Cargo package version must be canonical SemVer"),
        unix_now(),
    ))
}

fn embedded_authority() -> EmbeddedAuthority {
    let authority_config = authority_config_json();
    serde_json::from_str(&authority_config)
        .expect("entitlement authority configuration must be valid JSON")
}

fn authority_config_json() -> String {
    development_runtime_value("CAMELLIA_NEXUS_ENTITLEMENT_KEYS_PATH")
        .map(|path| {
            std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!("failed to read entitlement authority file {path}: {error}")
            })
        })
        .unwrap_or_else(|| include_str!("../entitlement-keys.json").to_owned())
}

pub(crate) fn service_settings() -> LicenseServiceSettings {
    let base_url = service_base_url();
    let loopback_development = base_url.as_deref().is_some_and(|url| {
        url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost")
    });
    let oauth_config = oauth_config().ok();
    LicenseServiceSettings {
        configured: base_url.is_some(),
        base_url,
        loopback_development,
        authorization_configured: oauth_config.is_some(),
        authorization_endpoint: oauth_config
            .as_ref()
            .map(|config| config.authorization_endpoint.to_string()),
        redirect_uri: oauth_config.map(|config| config.redirect_uri.to_string()),
    }
}

pub(crate) fn begin_loopback_license_authorization(
    redirect_uri: &str,
) -> camellia_nexus_licensing::Result<LicenseAuthorizationSession> {
    begin_license_authorization_with_mode(Some(redirect_uri), "loopback")
}

fn begin_license_authorization_with_mode(
    redirect_uri: Option<&str>,
    callback_mode: &'static str,
) -> camellia_nexus_licensing::Result<LicenseAuthorizationSession> {
    let config = oauth_config_with_redirect(redirect_uri)?;
    let request = begin_authorization(&config)?;
    Ok(authorization_session(
        request,
        config.redirect_uri.to_string(),
        callback_mode,
    ))
}

fn authorization_session(
    request: AuthorizationRequest,
    redirect_uri: String,
    callback_mode: &'static str,
) -> LicenseAuthorizationSession {
    LicenseAuthorizationSession {
        request: LicenseAuthorizationRequest {
            authorization_url: request.url.to_string(),
            state: request.state,
            callback_mode,
            suggested_device_name: suggested_device_name(),
        },
        pkce_verifier: camellia_nexus_licensing::SecretValue(request.verifier.expose().to_owned()),
        redirect_uri,
    }
}

fn suggested_device_name() -> String {
    let hostname = std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(read_unix_hostname)
        .map(|value| clean_device_name(&value))
        .filter(|value| !value.is_empty());
    let platform = platform_display_name();
    hostname
        .map(|name| {
            if name
                .to_ascii_lowercase()
                .contains(&platform.to_ascii_lowercase())
            {
                name
            } else {
                format!("{name} · {platform}")
            }
        })
        .unwrap_or_else(|| format!("{platform} device"))
}

#[cfg(unix)]
fn read_unix_hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname").ok()
}

#[cfg(not(unix))]
fn read_unix_hostname() -> Option<String> {
    None
}

fn clean_device_name(value: &str) -> String {
    let collapsed = value
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    collapsed
        .trim_matches(|character: char| character == '-' || character == '_' || character == '.')
        .chars()
        .take(64)
        .collect()
}

fn platform_display_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        "freebsd" => "FreeBSD",
        _ => "Desktop",
    }
}

pub(crate) fn http_api() -> camellia_nexus_licensing::Result<HttpLicenseApi> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let base_url =
        service_base_url().ok_or(camellia_nexus_licensing::LicensingError::ServiceUnconfigured)?;
    let url = Url::parse(&base_url)
        .map_err(|_| camellia_nexus_licensing::LicensingError::ServiceUnconfigured)?;
    HttpLicenseApi::new(url)
}

fn service_base_url() -> Option<String> {
    development_runtime_value("CAMELLIA_NEXUS_LICENSE_URL")
        .or_else(|| {
            option_env!("CAMELLIA_NEXUS_LICENSE_URL")
                .map(str::to_owned)
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| Some(embedded_authority().issuer).filter(|value| !value.trim().is_empty()))
}

fn oauth_config() -> camellia_nexus_licensing::Result<OAuthConfig> {
    oauth_config_with_redirect(None)
}

fn oauth_config_with_redirect(
    redirect_uri: Option<&str>,
) -> camellia_nexus_licensing::Result<OAuthConfig> {
    let authorization_endpoint = authorization_endpoint()
        .ok_or(camellia_nexus_licensing::LicensingError::ServiceUnconfigured)?;
    let redirect_uri = redirect_uri
        .map(str::to_owned)
        .unwrap_or_else(oauth_redirect_uri);
    let config = OAuthConfig {
        authorization_endpoint: Url::parse(&authorization_endpoint)
            .map_err(|_| camellia_nexus_licensing::LicensingError::ServiceUnconfigured)?,
        client_id: oauth_client_id(),
        redirect_uri: Url::parse(&redirect_uri)
            .map_err(|_| camellia_nexus_licensing::LicensingError::ServiceUnconfigured)?,
    };
    config.validate()?;
    Ok(config)
}

fn authorization_endpoint() -> Option<String> {
    development_runtime_value("CAMELLIA_NEXUS_AUTHORIZATION_ENDPOINT")
        .or_else(|| {
            option_env!("CAMELLIA_NEXUS_AUTHORIZATION_ENDPOINT")
                .map(str::to_owned)
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            service_base_url().and_then(|base_url| {
                let mut url = Url::parse(&base_url).ok()?;
                if !url.path().ends_with('/') {
                    let path = format!("{}/", url.path());
                    url.set_path(&path);
                }
                url.join("oauth/authorize").ok().map(|url| url.to_string())
            })
        })
}

fn oauth_client_id() -> String {
    development_runtime_value("CAMELLIA_NEXUS_OAUTH_CLIENT_ID")
        .or_else(|| {
            option_env!("CAMELLIA_NEXUS_OAUTH_CLIENT_ID")
                .map(str::to_owned)
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "camellia-nexus-desktop".to_owned())
}

fn oauth_redirect_uri() -> String {
    development_runtime_value("CAMELLIA_NEXUS_OAUTH_REDIRECT_URI")
        .or_else(|| {
            option_env!("CAMELLIA_NEXUS_OAUTH_REDIRECT_URI")
                .map(str::to_owned)
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(default_redirect_uri)
}

fn default_redirect_uri() -> String {
    "camellia-nexus://auth/callback".to_owned()
}

/// Runtime license endpoint and trust-root overrides are development-only. Production packages
/// obtain these values exclusively from compile-time configuration and embedded signed metadata.
#[cfg(debug_assertions)]
fn development_runtime_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(not(debug_assertions))]
const fn development_runtime_value(_name: &str) -> Option<String> {
    None
}

fn secure_store() -> DynSecureStore {
    let store = Arc::new(os_secure_store());
    match store.get_secret(SecretKey::DeviceRegistration) {
        Ok(_) => store,
        Err(LicensingError::SecureStoreUnavailable) => {
            tracing::warn!(
                "OS secure storage is unavailable; membership credentials are session-only"
            );
            Arc::new(SessionSecureStore::default())
        }
        Err(error) => {
            tracing::warn!(%error, "OS secure storage needs reauthentication or repair");
            store
        }
    }
}

fn os_secure_store() -> OsSecureStore {
    #[cfg(feature = "desktop-e2e")]
    {
        let namespace = std::env::var("CAMELLIA_NEXUS_E2E_NAMESPACE")
            .expect("CAMELLIA_NEXUS_E2E_NAMESPACE is required by desktop-e2e builds");
        OsSecureStore::for_test_namespace(&namespace)
            .expect("CAMELLIA_NEXUS_E2E_NAMESPACE must be a safe credential namespace")
    }
    #[cfg(not(feature = "desktop-e2e"))]
    OsSecureStore::new()
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_authority_contains_no_private_key_material() {
        let embedded = include_str!("../entitlement-keys.json");
        assert!(!embedded.contains("PRIVATE KEY"));
        assert!(!embedded.contains("skip"));
        let authority: EmbeddedAuthority = serde_json::from_str(embedded).expect("authority");
        assert!(
            authority
                .keys
                .iter()
                .all(|key| key.public_key_pem.contains("PUBLIC KEY"))
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn production_configuration_ignores_runtime_license_overrides() {
        assert!(development_runtime_value("CAMELLIA_NEXUS_LICENSE_URL").is_none());
        assert!(development_runtime_value("CAMELLIA_NEXUS_ENTITLEMENT_KEYS_PATH").is_none());
    }
}
