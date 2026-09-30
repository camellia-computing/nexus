use super::*;

pub(super) fn route_list_path(program: ProgramKind) -> SemanticPath {
    key_path(match program {
        ProgramKind::SingBox => &["route", "rules"],
        ProgramKind::Xray => &["routing", "rules"],
        _ => &["rules"],
    })
}

pub(super) fn route_values(program: ProgramKind, rule: &Value) -> Option<BTreeMap<String, Value>> {
    let (mode, value, target) = match program {
        ProgramKind::Mihomo => {
            let parts = rule.as_str()?.split(',').collect::<Vec<_>>();
            if parts.len() != 3 {
                return None;
            }
            (
                match parts[0] {
                    "DOMAIN" => "domain",
                    "DOMAIN-SUFFIX" => "subdomain",
                    "IP-CIDR" | "IP-CIDR6" => "ip",
                    _ => return None,
                },
                parts[1].to_owned(),
                parts[2].to_owned(),
            )
        }
        ProgramKind::SingBox => {
            let fields = rule.as_object()?;
            if fields.keys().any(|key| {
                !["domain", "domain_suffix", "ip_cidr", "outbound", "action"]
                    .contains(&key.as_str())
            }) || fields.get("action").is_some_and(|action| action != "route")
            {
                return None;
            }
            let keys = [
                ("domain", "domain"),
                ("subdomain", "domain_suffix"),
                ("ip", "ip_cidr"),
            ]
            .into_iter()
            .filter(|(_, key)| fields.contains_key(*key))
            .collect::<Vec<_>>();
            if keys.len() != 1 {
                return None;
            }
            let value = fields.get(keys[0].1)?;
            let value = value.as_str().map(str::to_owned).or_else(|| {
                let items = value.as_array()?;
                (items.len() == 1)
                    .then(|| items[0].as_str().map(str::to_owned))
                    .flatten()
            })?;
            (keys[0].0, value, rule.get("outbound")?.as_str()?.to_owned())
        }
        ProgramKind::Xray => {
            let fields = rule.as_object()?;
            if fields
                .keys()
                .any(|key| !["type", "domain", "ip", "outboundTag"].contains(&key.as_str()))
                || fields.get("type") != Some(&json!("field"))
            {
                return None;
            }
            let (mode, value) = if let Some(values) = fields.get("domain").and_then(Value::as_array)
            {
                if values.len() != 1 || fields.contains_key("ip") {
                    return None;
                }
                let value = values[0].as_str()?;
                if let Some(value) = value.strip_prefix("full:") {
                    ("domain", value.to_owned())
                } else {
                    let value = value.strip_prefix("domain:")?;
                    ("subdomain", value.to_owned())
                }
            } else {
                let values = fields.get("ip")?.as_array()?;
                if values.len() != 1 {
                    return None;
                }
                ("ip", values[0].as_str()?.to_owned())
            };
            (mode, value, fields.get("outboundTag")?.as_str()?.to_owned())
        }
        ProgramKind::Generic => return None,
    };
    Some(BTreeMap::from([
        ("match".into(), json!(mode)),
        ("value".into(), json!(value)),
        ("target".into(), json!(target)),
    ]))
}

pub(super) fn compile_route_rule(
    program: ProgramKind,
    values: &BTreeMap<String, Value>,
) -> Result<Value> {
    let text = |key| {
        values
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("INTENT_REQUIRED_VALUE"))
    };
    let mode = text("match")?;
    let mut value = text("value")?.trim().to_owned();
    let target = text("target")?;
    if target.contains([',', '\r', '\n']) || target.len() > 256 {
        return Err(invalid("INTENT_TARGET_MISSING"));
    }
    if mode == "ip" {
        let (ip, mask) = value
            .split_once('/')
            .map_or((value.as_str(), None), |(ip, mask)| (ip, Some(mask)));
        let ip = ip
            .parse::<std::net::IpAddr>()
            .map_err(|_| invalid("INTENT_ADDRESS_INVALID"))?;
        if mask.is_some_and(|mask| {
            !mask
                .parse::<u16>()
                .is_ok_and(|mask| mask <= if ip.is_ipv4() { 32 } else { 128 })
        }) {
            return Err(invalid("INTENT_ADDRESS_INVALID"));
        }
        if mask.is_none() {
            value = format!("{ip}/{}", if ip.is_ipv4() { 32 } else { 128 });
        }
    } else if !["domain", "subdomain"].contains(&mode) || !valid_domain(&value) {
        return Err(invalid("INTENT_ADDRESS_INVALID"));
    }
    Ok(match program {
        ProgramKind::SingBox => {
            json!({match mode { "domain" => "domain", "subdomain" => "domain_suffix", _ => "ip_cidr" }: [value], "action":"route", "outbound":target})
        }
        ProgramKind::Xray => {
            if mode == "ip" {
                json!({"type":"field", "ip":[value], "outboundTag":target})
            } else {
                json!({"type":"field", "domain":[format!("{}:{value}", if mode == "domain" {"full"} else {"domain"})], "outboundTag":target})
            }
        }
        ProgramKind::Mihomo => json!(format!(
            "{},{value},{target}",
            match mode {
                "domain" => "DOMAIN",
                "subdomain" => "DOMAIN-SUFFIX",
                _ if value.contains(':') => "IP-CIDR6",
                _ => "IP-CIDR",
            }
        )),
        _ => return Err(invalid("INTENT_SETTING_UNAVAILABLE")),
    })
}

pub(super) fn valid_domain(value: &str) -> bool {
    value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub(super) fn route_is_catchall(program: ProgramKind, rule: &Value) -> bool {
    match program {
        ProgramKind::Mihomo => rule
            .as_str()
            .is_some_and(|rule| rule.starts_with("MATCH,") || rule.starts_with("FINAL,")),
        ProgramKind::SingBox => rule.as_object().is_some_and(|fields| {
            fields
                .keys()
                .all(|key| ["action", "outbound", "type"].contains(&key.as_str()))
        }),
        ProgramKind::Xray => rule.as_object().is_some_and(|fields| {
            fields
                .keys()
                .all(|key| ["type", "outboundTag", "ruleTag", "network"].contains(&key.as_str()))
                && fields
                    .get("network")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value == "tcp,udp" || value == "udp,tcp")
        }),
        _ => false,
    }
}
