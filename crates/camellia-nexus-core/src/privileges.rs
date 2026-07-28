use crate::{CamelliaNexusError, ErrorCode, PrivilegeReason, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrivilegeAssessmentContext {
    pub privileged_ports_require_elevation: bool,
}

impl PrivilegeAssessmentContext {
    pub const fn for_current_platform() -> Self {
        Self {
            privileged_ports_require_elevation: cfg!(unix),
        }
    }
}

pub fn assess_json_proxy_configuration(
    content: &[u8],
    context: PrivilegeAssessmentContext,
) -> Result<Vec<PrivilegeReason>> {
    let normalized = crate::normalize_jsonc(content);
    let document: serde_json::Value = serde_json::from_slice(&normalized).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "Could not assess privileges because the configuration is invalid",
        )
        .with_details(error.to_string())
    })?;
    Ok(inspect_json_inbounds(&document, context))
}

pub fn assess_mihomo_configuration(
    content: &[u8],
    context: PrivilegeAssessmentContext,
) -> Result<Vec<PrivilegeReason>> {
    let yaml: serde_yaml_ng::Value = serde_yaml_ng::from_slice(content).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "Could not assess privileges because the configuration is invalid",
        )
        .with_details(error.to_string())
    })?;
    let document = serde_json::to_value(yaml).map_err(CamelliaNexusError::storage)?;
    Ok(inspect_mihomo(&document, context))
}

fn inspect_json_inbounds(
    document: &serde_json::Value,
    context: PrivilegeAssessmentContext,
) -> Vec<PrivilegeReason> {
    let mut reasons = Vec::new();
    for collection in ["inbounds", "endpoints"] {
        let Some(entries) = document
            .get(collection)
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for inbound in entries {
            let kind = inbound
                .get("type")
                .or_else(|| inbound.get("protocol"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            match kind.as_str() {
                "tun" => reasons.push(PrivilegeReason::TunInterface),
                "redirect" | "tproxy" => reasons.push(PrivilegeReason::TransparentProxy),
                _ => {}
            }
            if context.privileged_ports_require_elevation {
                for key in ["listen_port", "listenPort", "port"] {
                    if let Some(port) = json_port(inbound.get(key))
                        && port < 1024
                    {
                        reasons.push(PrivilegeReason::PrivilegedPort { port });
                    }
                }
            }
            if inbound
                .pointer("/streamSettings/sockopt/tproxy")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| !value.is_empty() && value != "off")
            {
                reasons.push(PrivilegeReason::TransparentProxy);
            }
        }
    }
    normalize_reasons(reasons)
}

fn inspect_mihomo(
    document: &serde_json::Value,
    context: PrivilegeAssessmentContext,
) -> Vec<PrivilegeReason> {
    let mut reasons = Vec::new();
    if document
        .pointer("/tun/enable")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        reasons.push(PrivilegeReason::TunInterface);
    }
    for key in ["redir-port", "tproxy-port"] {
        if json_port(document.get(key)).is_some_and(|port| port != 0) {
            reasons.push(PrivilegeReason::TransparentProxy);
        }
    }
    if context.privileged_ports_require_elevation {
        for key in ["port", "socks-port", "mixed-port"] {
            if let Some(port) = json_port(document.get(key))
                && port != 0
                && port < 1024
            {
                reasons.push(PrivilegeReason::PrivilegedPort { port });
            }
        }
        if let Some(listeners) = document
            .get("listeners")
            .and_then(serde_json::Value::as_array)
        {
            for listener in listeners {
                if let Some(port) = json_port(listener.get("port"))
                    && port != 0
                    && port < 1024
                {
                    reasons.push(PrivilegeReason::PrivilegedPort { port });
                }
            }
        }
    }
    normalize_reasons(reasons)
}

fn json_port(value: Option<&serde_json::Value>) -> Option<u16> {
    u16::try_from(value?.as_u64()?).ok()
}

pub fn normalize_reasons(mut reasons: Vec<PrivilegeReason>) -> Vec<PrivilegeReason> {
    reasons.sort_by_key(reason_sort_key);
    reasons.dedup();
    reasons
}

fn reason_sort_key(reason: &PrivilegeReason) -> (u8, u16) {
    match reason {
        PrivilegeReason::TunInterface => (0, 0),
        PrivilegeReason::TransparentProxy => (1, 0),
        PrivilegeReason::PrivilegedPort { port } => (2, *port),
        PrivilegeReason::ExecutableManifest => (3, 0),
        PrivilegeReason::ExplicitPolicy => (4, 0),
        PrivilegeReason::ConfigurationUnavailable => (5, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_json_analyzer_handles_jsonc_and_deduplicates_reasons() {
        let reasons = assess_json_proxy_configuration(
            br#"{/* comment */ "inbounds": [
                {"type":"tun",},
                {"protocol":"tun"},
                {"protocol":"dokodemo-door","port":80,
                 "streamSettings":{"sockopt":{"tproxy":"tproxy"}}}
            ],}"#,
            PrivilegeAssessmentContext {
                privileged_ports_require_elevation: true,
            },
        )
        .expect("assessment");
        assert_eq!(
            reasons,
            vec![
                PrivilegeReason::TunInterface,
                PrivilegeReason::TransparentProxy,
                PrivilegeReason::PrivilegedPort { port: 80 },
            ]
        );
    }

    #[test]
    fn privileged_ports_follow_the_platform_context() {
        let configuration = br#"{"inbounds":[{"type":"http","listen_port":80}]}"#;
        assert!(
            assess_json_proxy_configuration(
                configuration,
                PrivilegeAssessmentContext {
                    privileged_ports_require_elevation: false,
                },
            )
            .expect("assessment")
            .is_empty()
        );
    }
}
