use super::*;

pub(super) fn compile_object(
    profile: &CoreCapabilityProfile,
    kind: IntentObjectKind,
    values: &BTreeMap<String, Value>,
    previous: Option<&Value>,
) -> Result<(SemanticPath, Value)> {
    let program = profile.program;
    let descriptor = intent_object_catalog(program)
        .into_iter()
        .find(|d| d.kind == kind)
        .ok_or_else(|| invalid("INTENT_SETTING_UNAVAILABLE"))?;
    if values
        .keys()
        .any(|key| !descriptor.fields.iter().any(|f| &f.key == key))
    {
        return Err(invalid("INTENT_FIELD_UNKNOWN"));
    }
    for (key, value) in values {
        let field = descriptor
            .fields
            .iter()
            .find(|field| &field.key == key)
            .expect("Fields were checked");
        let valid = match field.control {
            GuidedControl::Toggle => value.is_boolean(),
            GuidedControl::Number => value.is_u64(),
            GuidedControl::Select => value.as_str().is_some_and(|value| {
                field.allowed_values.is_empty()
                    || field.allowed_values.iter().any(|option| option == value)
            }),
            GuidedControl::Text => value
                .as_str()
                .is_some_and(|value| value.len() <= 4096 && !value.contains('\0')),
        };
        if !valid {
            return Err(invalid("INTENT_VALUE_INVALID"));
        }
    }
    let text = |key: &str| values.get(key).and_then(Value::as_str);
    let previous_fields = previous.map(|value| {
        object_values(
            program,
            &ObjectLocation {
                kind,
                path: Vec::new(),
                value: value.clone(),
                list: false,
                owner: None,
            },
        )
    });
    if let (Some(protocol), Some(previous_protocol)) = (
        text("protocol"),
        previous_fields
            .as_ref()
            .and_then(|fields| fields.get("protocol"))
            .and_then(Value::as_str),
    ) && protocol != previous_protocol
    {
        return Err(invalid("INTENT_PROTOCOL_CHANGE_REQUIRES_NEW_OBJECT"));
    }
    let tag = previous
        .and_then(|value| value.get("tag"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("nexus-{}", uuid::Uuid::new_v4().simple()));
    let mut object = previous.cloned().unwrap_or_else(|| json!({"tag":tag}));
    let path;
    match kind {
        IntentObjectKind::Listener => {
            let protocol = text("protocol")
                .or_else(|| {
                    previous
                        .and_then(|v| {
                            v.get(if program == ProgramKind::SingBox {
                                "type"
                            } else {
                                "protocol"
                            })
                        })
                        .and_then(Value::as_str)
                })
                .ok_or_else(|| invalid("INTENT_REQUIRED_VALUE"))?;
            if !descriptor.protocols.iter().any(|option| option == protocol) {
                return Err(invalid("INTENT_SETTING_UNAVAILABLE"));
            }
            let port = values
                .get("port")
                .or_else(|| {
                    previous.and_then(|v| {
                        v.get(if program == ProgramKind::SingBox {
                            "listen_port"
                        } else {
                            "port"
                        })
                    })
                })
                .ok_or_else(|| invalid("INTENT_REQUIRED_VALUE"))?;
            checked_port(port)?;
            let previous_listen = previous
                .and_then(|value| value.get("listen"))
                .and_then(Value::as_str);
            let previous_access = previous
                .and_then(|value| value.get("access"))
                .and_then(Value::as_str)
                .unwrap_or(
                    if previous_listen.is_some_and(|listen| {
                        listen
                            .parse::<std::net::IpAddr>()
                            .is_ok_and(|ip| !ip.is_loopback())
                    }) {
                        "lan"
                    } else {
                        "local"
                    },
                );
            let access = text("access").unwrap_or(previous_access);
            let listen = text("listen")
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    if values.contains_key("access") && access != previous_access {
                        None
                    } else {
                        previous_listen
                    }
                })
                .unwrap_or(if access == "lan" {
                    "0.0.0.0"
                } else {
                    "127.0.0.1"
                });
            let ip = listen
                .parse::<std::net::IpAddr>()
                .map_err(|_| invalid("INTENT_ADDRESS_INVALID"))?;
            if access == "local" && !ip.is_loopback() {
                return Err(invalid("INTENT_LOCAL_ADDRESS_REQUIRED"));
            }
            let authentication = match (text("username"), text("password")) {
                (Some(user), Some(pass)) if !user.is_empty() && !pass.is_empty() => {
                    if user.contains([':', '\r', '\n']) || pass.contains(['\r', '\n']) {
                        return Err(invalid("INTENT_VALUE_INVALID"));
                    }
                    Some((user, pass))
                }
                (None, None) => None,
                _ => return Err(invalid("INTENT_AUTH_REQUIRED")),
            };
            if access == "lan"
                && (previous.is_none() || previous_access != "lan")
                && authentication.is_none()
            {
                return Err(invalid("INTENT_AUTH_REQUIRED"));
            }
            if program == ProgramKind::Mihomo {
                let key = match protocol {
                    "mixed" => "mixed-port",
                    "http" => "port",
                    _ => "socks-port",
                };
                object = json!({key:port});
                if previous.is_none() || values.contains_key("access") {
                    object["allow-lan"] = json!(access == "lan");
                }
                if previous.is_none()
                    || values.contains_key("listen")
                    || values.contains_key("access")
                {
                    object["bind-address"] = json!(listen);
                }
                if let Some((user, pass)) = authentication {
                    object["authentication"] = json!([format!("{user}:{pass}")]);
                }
                path = Vec::new();
            } else {
                object[if program == ProgramKind::SingBox {
                    "type"
                } else {
                    "protocol"
                }] = json!(protocol);
                object[if program == ProgramKind::SingBox {
                    "listen_port"
                } else {
                    "port"
                }] = port.clone();
                if previous.is_none()
                    || values.contains_key("listen")
                    || values.contains_key("access")
                {
                    object["listen"] = json!(listen);
                }
                if let Some((user, pass)) = authentication {
                    if program == ProgramKind::SingBox {
                        object["users"] = merge_listener_account(
                            object.get("users"),
                            "username",
                            "password",
                            user,
                            pass,
                        )?;
                    } else {
                        object["settings"]["accounts"] = merge_listener_account(
                            object.pointer("/settings/accounts"),
                            "user",
                            "pass",
                            user,
                            pass,
                        )?;
                        if protocol == "socks" {
                            object["settings"]["auth"] = json!("password");
                        }
                    }
                }
                if program == ProgramKind::Xray
                    && protocol == "socks"
                    && let Some(udp) = values.get("udp")
                {
                    object["settings"]["udp"] = udp.clone();
                }
                path = identity_location(kind, &["inbounds"], &object)
                    .expect("Object tag exists")
                    .path;
            }
        }
        IntentObjectKind::DnsServer => {
            let protocol = text("protocol")
                .or_else(|| {
                    previous_fields
                        .as_ref()
                        .and_then(|fields| fields.get("protocol"))
                        .and_then(Value::as_str)
                })
                .ok_or_else(|| invalid("INTENT_REQUIRED_VALUE"))?;
            let protocol = if protocol == "local" {
                "system"
            } else {
                protocol
            };
            if !descriptor.protocols.iter().any(|option| option == protocol) {
                return Err(invalid("INTENT_SETTING_UNAVAILABLE"));
            }
            let server = text("server").or_else(|| {
                previous_fields
                    .as_ref()
                    .and_then(|fields| fields.get("server"))
                    .and_then(Value::as_str)
            });
            if protocol != "system" && server.is_none_or(str::is_empty) {
                return Err(invalid("INTENT_REQUIRED_VALUE"));
            }
            if let Some(port) = values.get("serverPort") {
                checked_port(port)?;
            }
            if values.get("timeout").is_some_and(|value| {
                value
                    .as_u64()
                    .is_none_or(|value| !(1..=300_000).contains(&value))
            }) {
                return Err(invalid("INTENT_VALUE_INVALID"));
            }
            if let Some(server) = server
                && protocol != "system"
            {
                if protocol == "https" {
                    let url =
                        url::Url::parse(server).map_err(|_| invalid("INTENT_ADDRESS_INVALID"))?;
                    if url.scheme() != "https"
                        || url.host_str().is_none()
                        || !url.username().is_empty()
                        || url.password().is_some()
                        || url.fragment().is_some()
                    {
                        return Err(invalid("INTENT_ADDRESS_INVALID"));
                    }
                } else if program == ProgramKind::SingBox {
                    if server.parse::<std::net::IpAddr>().is_err() && !valid_domain(server) {
                        return Err(invalid("INTENT_ADDRESS_INVALID"));
                    }
                } else if server.parse::<std::net::IpAddr>().is_err() {
                    let host = url::Url::parse(&format!("udp://{server}"))
                        .map_err(|_| invalid("INTENT_ADDRESS_INVALID"))?;
                    if host.host_str().is_none()
                        || !host.username().is_empty()
                        || host.password().is_some()
                        || !host.path().is_empty()
                        || host.query().is_some()
                        || host.fragment().is_some()
                    {
                        return Err(invalid("INTENT_ADDRESS_INVALID"));
                    }
                    if program == ProgramKind::Xray
                        && host.host_str().is_none_or(|host| {
                            host.trim_matches(['[', ']'])
                                .parse::<std::net::IpAddr>()
                                .is_err()
                        })
                    {
                        return Err(invalid("INTENT_ADDRESS_INVALID"));
                    }
                }
            }
            if program == ProgramKind::SingBox {
                object["type"] = json!(if protocol == "system" {
                    "local"
                } else {
                    protocol
                });
                if protocol != "system"
                    && let Some(server) = server
                {
                    if protocol == "https" {
                        let url = url::Url::parse(server)
                            .map_err(|_| invalid("INTENT_ADDRESS_INVALID"))?;
                        if url.scheme() != "https"
                            || !url.username().is_empty()
                            || url.password().is_some()
                            || url.fragment().is_some()
                        {
                            return Err(invalid("INTENT_ADDRESS_INVALID"));
                        }
                        object["server"] = json!(
                            url.host_str()
                                .ok_or_else(|| invalid("INTENT_ADDRESS_INVALID"))?
                        );
                        object["path"] = json!(if let Some(query) = url.query() {
                            format!("{}?{query}", url.path())
                        } else {
                            url.path().into()
                        });
                        if let Some(port) = url.port() {
                            object["server_port"] = json!(port);
                        } else if values.contains_key("server") {
                            object
                                .as_object_mut()
                                .expect("DNS object")
                                .remove("server_port");
                        }
                    } else {
                        object["server"] = json!(server);
                    }
                    if object["server"]
                        .as_str()
                        .is_some_and(|server| server.parse::<std::net::IpAddr>().is_err())
                    {
                        let bootstrap = text("bootstrap")
                            .or_else(|| {
                                previous
                                    .and_then(|value| value.get("domain_resolver"))
                                    .and_then(Value::as_str)
                            })
                            .filter(|v| !v.is_empty())
                            .ok_or_else(|| invalid("INTENT_DNS_BOOTSTRAP_REQUIRED"))?;
                        object["domain_resolver"] = json!(bootstrap);
                    }
                }
                if let Some(port) = values.get("serverPort") {
                    object["server_port"] = port.clone();
                }
                path = identity_location(kind, &["dns", "servers"], &object)
                    .expect("Object tag exists")
                    .path;
            } else {
                let address = if protocol == "system" {
                    if program == ProgramKind::Xray {
                        "localhost".into()
                    } else {
                        "system".into()
                    }
                } else {
                    let server = server.expect("Required server checked");
                    match protocol {
                        "udp" => server.to_owned(),
                        "tls" => format!("tls://{server}"),
                        "https" => {
                            if !server.starts_with("https://") {
                                return Err(invalid("INTENT_ADDRESS_INVALID"));
                            }
                            server.into()
                        }
                        _ => unreachable!(),
                    }
                };
                let value = if program == ProgramKind::Xray {
                    let mut value = previous.cloned().unwrap_or_else(|| json!({"tag":tag}));
                    value["address"] = json!(address);
                    if let Some(port) = values.get("serverPort") {
                        value["port"] = port.clone();
                    }
                    if let Some(timeout) = values.get("timeout") {
                        value["timeoutMs"] = timeout.clone();
                    }
                    value
                } else {
                    json!(address)
                };
                if program == ProgramKind::Xray {
                    path = identity_location(kind, &["dns", "servers"], &value)
                        .expect("DNS tag exists")
                        .path;
                    object = value;
                } else {
                    path = key_path(&["dns", "nameserver"]);
                    object = value;
                }
            }
        }
        IntentObjectKind::Tun => {
            object["type"] = json!("tun");
            if let Some(address) = text("address") {
                let entries: Vec<_> = address.split_whitespace().collect();
                if entries.is_empty()
                    || entries.iter().any(|entry| {
                        let Some((ip, bits)) = entry.split_once('/') else {
                            return true;
                        };
                        let Ok(ip) = ip.parse::<std::net::IpAddr>() else {
                            return true;
                        };
                        !bits
                            .parse::<u16>()
                            .is_ok_and(|bits| bits <= if ip.is_ipv4() { 32 } else { 128 })
                    })
                {
                    return Err(invalid("INTENT_ADDRESS_INVALID"));
                }
                object["address"] = json!(entries);
            }
            if previous.is_none() && object.get("address").is_none() {
                return Err(invalid("INTENT_REQUIRED_VALUE"));
            }
            for (ui, native) in [
                ("autoRoute", "auto_route"),
                ("strictRoute", "strict_route"),
                ("dnsMode", "dns_mode"),
                ("stack", "stack"),
                ("mtu", "mtu"),
            ] {
                if let Some(value) = values.get(ui) {
                    object[native] = value.clone();
                }
            }
            if values.get("mtu").is_some_and(|value| {
                value
                    .as_u64()
                    .is_none_or(|value| !(576..=65535).contains(&value))
            }) {
                return Err(invalid("INTENT_VALUE_INVALID"));
            }
            path = identity_location(kind, &["inbounds"], &object)
                .expect("Object tag exists")
                .path;
        }
        IntentObjectKind::RouteRule => {
            let fields = previous_fields
                .unwrap_or_default()
                .into_iter()
                .chain(values.clone())
                .collect();
            object = compile_route_rule(program, &fields)?;
            path = route_list_path(program);
        }
    }
    let mut probe = json!({});
    let list = kind == IntentObjectKind::RouteRule
        || (kind == IntentObjectKind::DnsServer && program == ProgramKind::Mihomo);
    set_semantic_path_creating_anchors(
        &mut probe,
        &path,
        if list {
            json!([object])
        } else {
            object.clone()
        },
    )
    .map_err(|_| invalid("INTENT_VALUE_INVALID"))?;
    let mut previous_probe = json!({});
    if let Some(previous) = previous {
        if program == ProgramKind::Mihomo && kind == IntentObjectKind::Listener {
            previous_probe = json!({});
        } else {
            set_semantic_path_creating_anchors(
                &mut previous_probe,
                &path,
                if list {
                    json!([previous])
                } else {
                    previous.clone()
                },
            )
            .map_err(|_| invalid("INTENT_VALUE_INVALID"))?;
        }
    }
    let old_issues = profile.assess(&previous_probe)?.issues;
    let unsupported = crate::configuration_field_evidence::undeclared_entries(profile, &probe)?
        .into_iter()
        .any(|entry| {
            previous.is_none()
                || value_at_native_path(&probe, &entry.path)
                    != value_at_native_path(&previous_probe, &entry.path)
        });
    if unsupported
        || profile
            .assess(&probe)?
            .issues
            .iter()
            .any(|issue| !old_issues.contains(issue))
    {
        return Err(invalid("INTENT_SETTING_UNAVAILABLE"));
    }
    Ok((path, object))
}
