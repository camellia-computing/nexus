use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use p256::elliptic_curve::Generate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{LicensingError, Result};

const PRODUCT_SCOPE: &str = "camellia.nexus.license";

#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub authorization_endpoint: Url,
    pub client_id: String,
    pub redirect_uri: Url,
}

impl OAuthConfig {
    pub fn validate(&self) -> Result<()> {
        if !valid_authorization_endpoint(&self.authorization_endpoint)
            || !valid_oauth_token(&self.client_id, 1, 128)
            || !valid_redirect_uri(&self.redirect_uri)
        {
            return Err(LicensingError::ServiceUnconfigured);
        }
        Ok(())
    }
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct PkceVerifier(String);

impl PkceVerifier {
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PkceVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PkceVerifier([REDACTED])")
    }
}

pub struct AuthorizationRequest {
    pub url: Url,
    pub state: String,
    pub verifier: PkceVerifier,
}

impl std::fmt::Debug for AuthorizationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizationRequest")
            .field("url", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .field("verifier", &self.verifier)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationCode {
    pub code: String,
    pub redirect_uri: String,
}

impl std::fmt::Debug for AuthorizationCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizationCode")
            .field("code", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .finish()
    }
}

pub fn begin_authorization(config: &OAuthConfig) -> Result<AuthorizationRequest> {
    config.validate()?;
    let verifier_bytes =
        <[u8; 32]>::try_generate().map_err(|_| LicensingError::EntropyUnavailable)?;
    let state_bytes = <[u8; 24]>::try_generate().map_err(|_| LicensingError::EntropyUnavailable)?;
    let verifier = URL_SAFE_NO_PAD.encode(verifier_bytes);
    let state = URL_SAFE_NO_PAD.encode(state_bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = config.authorization_endpoint.clone();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &config.client_id)
        .append_pair("redirect_uri", config.redirect_uri.as_str())
        .append_pair("scope", PRODUCT_SCOPE)
        .append_pair("state", &state)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(AuthorizationRequest {
        url,
        state,
        verifier: PkceVerifier(verifier),
    })
}

pub fn complete_authorization_callback(
    callback: &Url,
    expected_state: &str,
    redirect_uri: &Url,
) -> Result<AuthorizationCode> {
    if !valid_state(expected_state)
        || !valid_redirect_uri(redirect_uri)
        || callback.fragment().is_some()
    {
        return Err(LicensingError::InvalidOAuthCallback);
    }
    let mut callback_base = callback.clone();
    callback_base.set_query(None);
    if &callback_base != redirect_uri {
        return Err(LicensingError::InvalidOAuthCallback);
    }
    let mut code = None;
    let mut state = None;
    for (key, value) in callback.query_pairs() {
        let slot = match key.as_ref() {
            "code" => &mut code,
            "state" => &mut state,
            _ => return Err(LicensingError::InvalidOAuthCallback),
        };
        if slot.replace(value.into_owned()).is_some() {
            return Err(LicensingError::InvalidOAuthCallback);
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Err(LicensingError::InvalidOAuthCallback);
    }
    let code = code
        .filter(|value| valid_oauth_token(value, 43, 43))
        .ok_or(LicensingError::InvalidOAuthCallback)?;
    Ok(AuthorizationCode {
        code,
        redirect_uri: redirect_uri.to_string(),
    })
}

fn is_loopback_http(url: &Url) -> bool {
    url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "::1"))
        && url.port().is_some()
}

fn valid_authorization_endpoint(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn valid_redirect_uri(url: &Url) -> bool {
    let clean = url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    clean
        && ((url.scheme() == "camellia-nexus"
            && url.host_str() == Some("auth")
            && url.port().is_none()
            && url.path() == "/callback")
            || (is_loopback_http(url) && url.path() == "/auth/callback"))
}

fn valid_oauth_token(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_state(value: &str) -> bool {
    (32..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "flow", rename_all = "camelCase")]
pub enum AuthenticationFlow {
    BrowserPkce,
    DeviceCode {
        verification_uri: String,
        user_code: String,
        expires_at: i64,
        poll_interval_seconds: u64,
    },
}

impl std::fmt::Debug for AuthenticationFlow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BrowserPkce => formatter.write_str("BrowserPkce"),
            Self::DeviceCode {
                expires_at,
                poll_interval_seconds,
                ..
            } => formatter
                .debug_struct("DeviceCode")
                .field("verification_uri", &"[REDACTED]")
                .field("user_code", &"[REDACTED]")
                .field("expires_at", expires_at)
                .field("poll_interval_seconds", poll_interval_seconds)
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_pkce_request_and_validates_state() {
        let config = OAuthConfig {
            authorization_endpoint: Url::parse("https://login.example/authorize").unwrap(),
            client_id: "desktop".into(),
            redirect_uri: Url::parse("camellia-nexus://auth/callback").unwrap(),
        };
        let request = begin_authorization(&config).expect("request");
        assert!(request.url.as_str().contains("code_challenge_method=S256"));
        let callback = Url::parse(&format!(
            "camellia-nexus://auth/callback?code={}&state={}",
            "a".repeat(43),
            request.state
        ))
        .unwrap();
        let code = complete_authorization_callback(&callback, &request.state, &config.redirect_uri)
            .expect("callback");
        assert_eq!(code.code, "a".repeat(43));
    }

    #[test]
    fn authorization_diagnostics_redact_browser_and_device_secrets() {
        let config = OAuthConfig {
            authorization_endpoint: Url::parse("https://login.example/authorize").unwrap(),
            client_id: "desktop".into(),
            redirect_uri: Url::parse("camellia-nexus://auth/callback").unwrap(),
        };
        let request = begin_authorization(&config).expect("request");
        let request_debug = format!("{request:?}");
        assert!(request_debug.contains("AuthorizationRequest"));
        assert!(request_debug.contains("[REDACTED]"));
        assert!(!request_debug.contains(&request.state));
        assert!(!request_debug.contains("code_challenge="));

        let flow = AuthenticationFlow::DeviceCode {
            verification_uri: "https://login.example/device?code=uri-secret".into(),
            user_code: "device-user-code-secret".into(),
            expires_at: 1_800_000_000,
            poll_interval_seconds: 5,
        };
        let flow_debug = format!("{flow:?}");
        assert!(flow_debug.contains("[REDACTED]"));
        assert!(!flow_debug.contains("uri-secret"));
        assert!(!flow_debug.contains("device-user-code-secret"));
    }

    #[test]
    fn rejects_ambiguous_callback_and_redirect_authority() {
        let redirect = Url::parse("http://127.0.0.1:54321/auth/callback").unwrap();
        let state = "s".repeat(32);
        let code = "c".repeat(43);
        for callback in [
            format!("http://127.0.0.1:54322/auth/callback?code={code}&state={state}"),
            format!("http://127.0.0.1:54321/auth/callback?code={code}&code={code}&state={state}"),
            format!("http://127.0.0.1:54321/auth/callback?code={code}&state={state}&extra=1"),
        ] {
            assert!(matches!(
                complete_authorization_callback(&Url::parse(&callback).unwrap(), &state, &redirect),
                Err(LicensingError::InvalidOAuthCallback)
            ));
        }
    }

    #[test]
    fn rejects_unsafe_authorization_and_callback_configuration() {
        for (authorization_endpoint, redirect_uri) in [
            (
                "https://user:secret@login.example/authorize",
                "camellia-nexus://auth/callback",
            ),
            (
                "https://login.example/authorize?client=overridden",
                "camellia-nexus://auth/callback",
            ),
            (
                "https://login.example/authorize",
                "camellia-nexus://other/callback",
            ),
            (
                "https://login.example/authorize",
                "http://localhost:49152/auth/callback",
            ),
            (
                "https://login.example/authorize",
                "http://127.0.0.1/auth/callback",
            ),
        ] {
            let config = OAuthConfig {
                authorization_endpoint: Url::parse(authorization_endpoint).unwrap(),
                client_id: "desktop".into(),
                redirect_uri: Url::parse(redirect_uri).unwrap(),
            };
            assert!(matches!(
                config.validate(),
                Err(LicensingError::ServiceUnconfigured)
            ));
        }
    }
}
