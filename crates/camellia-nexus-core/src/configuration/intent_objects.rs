use super::*;
use crate::CoreCapabilityProfile;
use std::sync::{Mutex, OnceLock};
mod catalog;
mod compiler;
mod rules;
pub use catalog::intent_object_catalog;
use compiler::compile_object;
use rules::*;

static OBJECT_DESCRIPTORS: OnceLock<Mutex<BTreeMap<String, Vec<IntentObjectDescriptor>>>> =
    OnceLock::new();

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentPathEdit {
    pub path: SemanticPath,
    pub value: SemanticValue,
    #[serde(default)]
    pub list_edit: Option<IntentListEdit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentListEdit {
    pub original: Option<Value>,
    pub replacement: Option<Value>,
}

impl IntentListEdit {
    pub(super) fn apply(&self, items: &mut Vec<Value>) -> Result<()> {
        let matches = self
            .original
            .as_ref()
            .map(|original| {
                items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, value)| (value == original).then_some(index))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if matches.len() > 1 {
            return Err(invalid("INTENT_OBJECT_AMBIGUOUS"));
        }
        if let Some(index) = matches.first().copied() {
            if let Some(replacement) = &self.replacement {
                items[index] = replacement.clone();
            } else {
                items.remove(index);
            }
        } else if let Some(replacement) = &self.replacement
            && !items.contains(replacement)
        {
            items.push(replacement.clone());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntentObjectKind {
    Listener,
    DnsServer,
    Tun,
    RouteRule,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentObjectField {
    pub key: String,
    pub label: String,
    pub control: GuidedControl,
    pub allowed_values: Vec<String>,
    pub required: bool,
    pub secret: bool,
    pub advanced: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentObjectDescriptor {
    pub kind: IntentObjectKind,
    pub category: String,
    pub label: String,
    pub fields: Vec<IntentObjectField>,
    pub protocols: Vec<String>,
    pub can_create: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentObjectProjection {
    pub object_id: String,
    pub kind: IntentObjectKind,
    pub label: String,
    pub content_hash: String,
    pub values: BTreeMap<String, Value>,
    pub can_follow: bool,
    pub editable: bool,
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentTarget {
    pub id: String,
    pub label: String,
    pub kind: String,
}

struct ObjectLocation {
    kind: IntentObjectKind,
    path: SemanticPath,
    value: Value,
    list: bool,
    owner: Option<String>,
}

impl ObjectLocation {
    fn id(&self) -> String {
        self.owner.clone().unwrap_or_else(|| {
            let id = object_id(self.kind, &self.path);
            if self.list {
                format!("{id}:{}", semantic_document_hash(&self.value))
            } else {
                id
            }
        })
    }
}

fn object_id(kind: IntentObjectKind, path: &SemanticPath) -> String {
    format!(
        "{kind:?}:{}",
        hash_bytes(display_semantic_path(path).as_bytes())
    )
}

fn identity_location(
    kind: IntentObjectKind,
    path: &[&str],
    value: &Value,
) -> Option<ObjectLocation> {
    let tag = value.get("tag")?.as_str()?;
    let mut path = key_path(path);
    path.push(SemanticPathSegment::Identity {
        field: "tag".into(),
        value: tag.into(),
    });
    Some(ObjectLocation {
        kind,
        path,
        value: value.clone(),
        list: false,
        owner: None,
    })
}

fn locations(kind: ProgramKind, document: &Value) -> Vec<ObjectLocation> {
    let mut objects = Vec::new();
    if kind == ProgramKind::Mihomo {
        for (protocol, key) in [
            ("mixed", "mixed-port"),
            ("http", "port"),
            ("socks", "socks-port"),
        ] {
            if document
                .get(key)
                .and_then(Value::as_u64)
                .is_some_and(|port| port > 0)
            {
                objects.push(ObjectLocation { kind: IntentObjectKind::Listener, path: key_path(&[key]),
                    value: json!({"protocol":protocol,"port": document[key], "listen": document.get("bind-address"), "access":if document.get("allow-lan") == Some(&json!(true)) { "lan" } else { "local" }}), list: false, owner: None });
            }
        }
    } else if let Some(items) = document.get("inbounds").and_then(Value::as_array) {
        for item in items {
            let protocol = item
                .get(if kind == ProgramKind::SingBox {
                    "type"
                } else {
                    "protocol"
                })
                .and_then(Value::as_str)
                .unwrap_or_default();
            let object_kind = if protocol == "tun" {
                IntentObjectKind::Tun
            } else if ["mixed", "http", "socks"].contains(&protocol) {
                IntentObjectKind::Listener
            } else {
                continue;
            };
            if let Some(location) = identity_location(object_kind, &["inbounds"], item) {
                objects.push(location);
            }
        }
    }
    if matches!(kind, ProgramKind::SingBox | ProgramKind::Xray)
        && let Some(items) = document.pointer("/dns/servers").and_then(Value::as_array)
    {
        for item in items {
            if let Some(location) =
                identity_location(IntentObjectKind::DnsServer, &["dns", "servers"], item)
            {
                objects.push(location);
            }
        }
    }
    if kind == ProgramKind::Mihomo {
        for item in document
            .pointer("/dns/nameserver")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if item.is_string() {
                objects.push(ObjectLocation {
                    kind: IntentObjectKind::DnsServer,
                    path: key_path(&["dns", "nameserver"]),
                    value: item.clone(),
                    list: true,
                    owner: None,
                });
            }
        }
    }
    let path = route_list_path(kind);
    for item in semantic_value_at(document, &path)
        .ok()
        .and_then(|value| match value {
            SemanticValue::Present(Value::Array(items)) => Some(items),
            _ => None,
        })
        .unwrap_or_default()
    {
        if route_values(kind, &item).is_some() {
            objects.push(ObjectLocation {
                kind: IntentObjectKind::RouteRule,
                path: path.clone(),
                value: item,
                list: true,
                owner: None,
            });
        }
    }
    objects
}

fn state_locations(state: &ConfigurationState, document: &Value) -> Vec<ObjectLocation> {
    let mut objects = locations(state.kind, document);
    for object in &mut objects {
        if object.list {
            object.owner = state
                .guided_intent
                .path_edits
                .iter()
                .find(|(_, edit)| {
                    edit.path == object.path
                        && edit
                            .list_edit
                            .as_ref()
                            .and_then(|edit| edit.replacement.as_ref())
                            == Some(&object.value)
                })
                .map(|(id, _)| id.clone());
        }
    }
    objects
}

pub fn intent_targets(kind: ProgramKind, document: &Value) -> Vec<IntentTarget> {
    let mut targets = Vec::new();
    let lists: &[(&[&str], &str, &str)] = if kind == ProgramKind::Mihomo {
        &[
            (&["proxies"], "name", "connection"),
            (&["proxy-groups"], "name", "connection"),
        ]
    } else {
        &[
            (&["outbounds"], "tag", "connection"),
            (&["dns", "servers"], "tag", "dns"),
        ]
    };
    for (path, key, target_kind) in lists {
        for item in get_object_path(document, path)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = item.get(*key).and_then(Value::as_str) {
                targets.push(IntentTarget {
                    id: id.into(),
                    label: id.into(),
                    kind: (*target_kind).into(),
                });
            }
        }
    }
    if kind == ProgramKind::Mihomo {
        for id in ["DIRECT", "REJECT"] {
            targets.push(IntentTarget {
                id: id.into(),
                label: id.into(),
                kind: "connection".into(),
            });
        }
    }
    let mut counts = BTreeMap::new();
    for target in &targets {
        *counts
            .entry((target.kind.clone(), target.id.clone()))
            .or_insert(0) += 1;
    }
    targets.retain(|target| counts.get(&(target.kind.clone(), target.id.clone())) == Some(&1));
    targets
}

fn object_values(kind: ProgramKind, object: &ObjectLocation) -> BTreeMap<String, Value> {
    let value = &object.value;
    let mut fields = BTreeMap::new();
    if object.kind == IntentObjectKind::RouteRule {
        return route_values(kind, value).unwrap_or_default();
    } else if object.kind == IntentObjectKind::Listener {
        if kind == ProgramKind::Mihomo {
            for key in ["protocol", "port", "access", "listen"] {
                if let Some(value) = value.get(key) {
                    fields.insert(key.into(), value.clone());
                }
            }
        } else {
            let listen = value
                .get("listen")
                .and_then(Value::as_str)
                .unwrap_or_default();
            fields.insert(
                "protocol".into(),
                value
                    .get(if kind == ProgramKind::SingBox {
                        "type"
                    } else {
                        "protocol"
                    })
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            fields.insert(
                "port".into(),
                value
                    .get(if kind == ProgramKind::SingBox {
                        "listen_port"
                    } else {
                        "port"
                    })
                    .cloned()
                    .unwrap_or(Value::Null),
            );
            fields.insert("listen".into(), json!(listen));
            fields.insert(
                "access".into(),
                json!(if listen
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
                {
                    "local"
                } else {
                    "lan"
                }),
            );
            if kind == ProgramKind::Xray
                && let Some(udp) = value.pointer("/settings/udp")
            {
                fields.insert("udp".into(), udp.clone());
            }
        }
    } else if object.kind == IntentObjectKind::DnsServer {
        if kind == ProgramKind::Mihomo {
            if let Some(address) = value.as_str() {
                let (protocol, server) = if address == "system" || address == "system://" {
                    ("system", "")
                } else if let Some(server) = address.strip_prefix("tls://") {
                    ("tls", server)
                } else if address.starts_with("https://") {
                    ("https", address)
                } else if !address.contains("://") {
                    ("udp", address)
                } else {
                    return fields;
                };
                fields.insert("protocol".into(), json!(protocol));
                fields.insert("server".into(), json!(server));
            }
            return fields;
        }
        if kind == ProgramKind::Xray {
            if let Some(address) = value.get("address").and_then(Value::as_str) {
                fields.insert(
                    "protocol".into(),
                    json!(if address == "localhost" {
                        "system"
                    } else if address.starts_with("https://") {
                        "https"
                    } else {
                        "udp"
                    }),
                );
                fields.insert("server".into(), json!(address));
            }
            for (ui, native) in [("serverPort", "port"), ("timeout", "timeoutMs")] {
                if let Some(value) = value.get(native) {
                    fields.insert(ui.into(), value.clone());
                }
            }
            return fields;
        }
        for (ui, native) in [
            ("protocol", "type"),
            ("server", "server"),
            ("serverPort", "server_port"),
        ] {
            if let Some(value) = value.get(native) {
                fields.insert(ui.into(), value.clone());
            }
        }
        if fields.get("protocol") == Some(&json!("local")) {
            fields.insert("protocol".into(), json!("system"));
        }
        if fields.get("protocol") == Some(&json!("https"))
            && let Some(host) = value.get("server").and_then(Value::as_str)
        {
            let host = if host.contains(':') {
                format!("[{host}]")
            } else {
                host.into()
            };
            let port = value
                .get("server_port")
                .and_then(Value::as_u64)
                .map(|port| format!(":{port}"))
                .unwrap_or_default();
            fields.insert(
                "server".into(),
                json!(format!(
                    "https://{host}{port}{}",
                    value
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("/dns-query")
                )),
            );
        }
        if let Some(bootstrap) = value.get("domain_resolver").and_then(Value::as_str) {
            fields.insert("bootstrap".into(), json!(bootstrap));
        }
    } else if object.kind == IntentObjectKind::Tun {
        for (ui, native) in [
            ("autoRoute", "auto_route"),
            ("strictRoute", "strict_route"),
            ("dnsMode", "dns_mode"),
            ("stack", "stack"),
            ("mtu", "mtu"),
        ] {
            if let Some(value) = value.get(native) {
                fields.insert(ui.into(), value.clone());
            }
        }
        if let Some(addresses) = value.get("address").and_then(Value::as_array) {
            fields.insert(
                "address".into(),
                json!(
                    addresses
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            );
        }
    }
    fields
}

pub fn project_intent_objects(
    profile: &CoreCapabilityProfile,
    state: &ConfigurationState,
    view: &mut ConfigurationStateView,
) -> Result<()> {
    let document = parse_semantic_document(state.format, state.desired.content.as_bytes())?;
    view.intent_targets = intent_targets(state.kind, &document);
    let objects = state_locations(state, &document);
    let mut identities = BTreeMap::new();
    for object in &objects {
        *identities.entry(object.id()).or_insert(0) += 1;
    }
    view.intent_objects = objects
        .into_iter()
        .map(|object| {
            let id = object.id();
            IntentObjectProjection {
                object_id: if identities[&id] == 1 {
                    id.clone()
                } else {
                    format!("{id}:{}", semantic_document_hash(&object.value))
                },
                kind: object.kind,
                label: object
                    .value
                    .get("tag")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        object_values(state.kind, &object)
                            .get(if object.kind == IntentObjectKind::RouteRule {
                                "value"
                            } else {
                                "protocol"
                            })
                            .and_then(Value::as_str)
                            .unwrap_or("Local proxy")
                            .to_owned()
                    }),
                content_hash: semantic_document_hash(&object.value),
                values: object_values(state.kind, &object),
                can_follow: state
                    .guided_intent
                    .path_edits
                    .keys()
                    .any(|key| key.starts_with(&id)),
                editable: identities[&id] == 1 && !object_values(state.kind, &object).is_empty(),
                removed: false,
            }
        })
        .collect();
    // Identical duplicate identities are one non-editable problem entry.
    let mut seen = BTreeSet::new();
    view.intent_objects
        .retain(|object| seen.insert(object.object_id.clone()));
    let base = parse_semantic_document(state.format, state.base.content.as_bytes())?;
    for object in locations(state.kind, &base) {
        let id = object.id();
        let deleted_list = object.list
            && state
                .guided_intent
                .path_edits
                .get(&id)
                .and_then(|edit| edit.list_edit.as_ref())
                .is_some_and(|edit| edit.replacement.is_none())
            && !view
                .intent_objects
                .iter()
                .any(|entry| entry.object_id == id);
        if !seen.contains(&id)
            && state
                .guided_intent
                .path_edits
                .keys()
                .any(|key| key.starts_with(&id))
            && (deleted_list
                || semantic_value_at(&document, &object.path).ok() == Some(SemanticValue::Missing))
        {
            seen.insert(id.clone());
            view.intent_objects.push(IntentObjectProjection {
                object_id: id,
                kind: object.kind,
                label: object
                    .value
                    .get("tag")
                    .and_then(Value::as_str)
                    .unwrap_or("Local proxy")
                    .into(),
                content_hash: semantic_document_hash(&object.value),
                values: object_values(state.kind, &object),
                can_follow: true,
                editable: false,
                removed: true,
            });
        }
    }
    let cache = OBJECT_DESCRIPTORS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let cached = cache
        .lock()
        .map_err(|_| invalid("INTENT_SETTING_UNAVAILABLE"))?
        .get(&profile.profile_hash)
        .cloned();
    if let Some(descriptors) = cached {
        view.intent_object_descriptors = descriptors;
        return Ok(());
    }
    let mut descriptors = intent_object_catalog(state.kind);
    for descriptor in &mut descriptors {
        let protocols = descriptor.protocols.clone();
        descriptor.protocols.retain(|protocol| {
            let values = if descriptor.kind == IntentObjectKind::Listener {
                BTreeMap::from([
                    ("protocol".into(), json!(protocol)),
                    ("port".into(), json!(7890)),
                    ("access".into(), json!("local")),
                ])
            } else {
                BTreeMap::from([
                    ("protocol".into(), json!(protocol)),
                    (
                        "server".into(),
                        json!(if protocol == "https" {
                            "https://1.1.1.1/dns-query"
                        } else {
                            "1.1.1.1"
                        }),
                    ),
                ])
            };
            compile_object(profile, descriptor.kind, &values, None).is_ok()
        });
        descriptor.can_create = if matches!(
            descriptor.kind,
            IntentObjectKind::Tun | IntentObjectKind::RouteRule
        ) {
            compile_object(
                profile,
                descriptor.kind,
                &object_probe_values(descriptor.kind, &descriptor.protocols),
                None,
            )
            .is_ok()
        } else {
            !protocols.is_empty() && !descriptor.protocols.is_empty()
        };
        descriptor.fields.retain_mut(|field| {
            if matches!(field.key.as_str(), "protocol" | "username" | "password") {
                return true;
            }
            let mut values = object_probe_values(descriptor.kind, &descriptor.protocols);
            if field.key == "udp" {
                values.insert("protocol".into(), json!("socks"));
            }
            if field.key == "bootstrap" {
                values.insert("protocol".into(), json!("https"));
                values.insert(
                    "server".into(),
                    json!("https://resolver.example.invalid/dns-query"),
                );
            }
            let samples: Vec<Value> = if matches!(field.key.as_str(), "target" | "bootstrap") {
                vec![json!(if field.key == "target" {
                    "intent-probe"
                } else {
                    "resolver-probe"
                })]
            } else if field.control == GuidedControl::Select {
                field
                    .allowed_values
                    .iter()
                    .map(|option| json!(option))
                    .collect()
            } else {
                vec![match field.key.as_str() {
                    "address" => json!("172.19.0.1/30"),
                    "server" => json!("1.1.1.1"),
                    "listen" => json!("127.0.0.1"),
                    "bootstrap" => json!("resolver-probe"),
                    "mtu" => json!(1500),
                    "timeout" => json!(5000),
                    "serverPort" => json!(53),
                    "port" => json!(7890),
                    "value" => json!("example.invalid"),
                    "target" => json!("intent-probe"),
                    _ => json!(true),
                }]
            };
            let accepted = samples
                .into_iter()
                .filter(|sample| {
                    let mut probe = values.clone();
                    probe.insert(field.key.clone(), sample.clone());
                    compile_object(profile, descriptor.kind, &probe, None).is_ok()
                })
                .collect::<Vec<_>>();
            if field.control == GuidedControl::Select {
                field
                    .allowed_values
                    .retain(|option| accepted.contains(&json!(option)));
            }
            !accepted.is_empty()
        });
    }
    {
        let mut cache = cache
            .lock()
            .map_err(|_| invalid("INTENT_SETTING_UNAVAILABLE"))?;
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(profile.profile_hash.clone(), descriptors.clone());
    }
    view.intent_object_descriptors = descriptors;
    Ok(())
}

fn object_probe_values(kind: IntentObjectKind, protocols: &[String]) -> BTreeMap<String, Value> {
    match kind {
        IntentObjectKind::Listener => BTreeMap::from([
            (
                "protocol".into(),
                json!(protocols.first().map(String::as_str).unwrap_or("http")),
            ),
            ("port".into(), json!(7890)),
            ("access".into(), json!("local")),
        ]),
        IntentObjectKind::DnsServer => BTreeMap::from([
            ("protocol".into(), json!("udp")),
            ("server".into(), json!("1.1.1.1")),
        ]),
        IntentObjectKind::Tun => BTreeMap::from([("address".into(), json!("172.19.0.1/30"))]),
        IntentObjectKind::RouteRule => BTreeMap::from([
            ("match".into(), json!("domain")),
            ("value".into(), json!("example.invalid")),
            ("target".into(), json!("intent-probe")),
        ]),
    }
}

fn invalid(key: &str) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Review the highlighted setting")
        .with_message_key(key)
}

fn checked_port(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|port| (1..=65535).contains(port))
        .ok_or_else(|| invalid("INTENT_PORT_INVALID"))
}

fn value_at_native_path<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, key| match value {
        Value::Object(fields) => fields.get(key),
        Value::Array(items) => key.parse::<usize>().ok().and_then(|index| items.get(index)),
        _ => None,
    })
}

impl ConfigurationState {
    pub(super) fn update_intent_object(
        &mut self,
        profile: &CoreCapabilityProfile,
        change: &ConfigurationIntentAction,
    ) -> Result<()> {
        let upstream = self.upstream_document()?;
        let document = parse_semantic_document(self.format, self.desired.content.as_bytes())?;
        let candidates = state_locations(self, &document);
        let locate = |id: &str, hash: &str| -> Result<&ObjectLocation> {
            let matching = candidates
                .iter()
                .filter(|object| object.id() == id)
                .collect::<Vec<_>>();
            if matching.len() != 1 {
                return Err(invalid("INTENT_OBJECT_MISSING"));
            }
            let object = matching[0];
            if semantic_document_hash(&object.value) != hash {
                return Err(
                    CamelliaNexusError::new(ErrorCode::ConfigConflict, "Setting changed")
                        .with_message_key("INTENT_OBJECT_STALE"),
                );
            }
            Ok(object)
        };
        match change {
            ConfigurationIntentAction::CreateObject {
                object_kind,
                values,
            } => {
                let (path, mut value) = compile_object(profile, *object_kind, values, None)?;
                validate_object_references(self.kind, *object_kind, &document, &value)?;
                if *object_kind == IntentObjectKind::RouteRule && semantic_value_at(&document, &path).ok().is_some_and(|value| matches!(value, SemanticValue::Present(Value::Array(items)) if items.iter().any(|rule| route_is_catchall(self.kind, rule)))) {
                    return Err(invalid("INTENT_RULE_SHADOWED"));
                }
                if self.kind == ProgramKind::Mihomo && *object_kind == IntentObjectKind::Listener {
                    let existing = locations(self.kind, &document)
                        .iter()
                        .any(|object| object.kind == IntentObjectKind::Listener);
                    let access = document
                        .get("allow-lan")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if existing && value.get("allow-lan").and_then(Value::as_bool) != Some(access) {
                        return Err(invalid("INTENT_SHARED_ACCESS_CONFLICT"));
                    }
                    preserve_shared_authentication(&document, &mut value);
                }
                if *object_kind == IntentObjectKind::RouteRule
                    || (*object_kind == IntentObjectKind::DnsServer
                        && self.kind == ProgramKind::Mihomo)
                {
                    let owner = format!(
                        "{}:{}",
                        object_id(*object_kind, &path),
                        semantic_document_hash(&value)
                    );
                    self.record_intent_edit(
                        owner,
                        IntentPathEdit {
                            path,
                            value: SemanticValue::Missing,
                            list_edit: Some(IntentListEdit {
                                original: None,
                                replacement: Some(value),
                            }),
                        },
                    )?;
                } else if self.kind == ProgramKind::Mihomo
                    && *object_kind == IntentObjectKind::Listener
                {
                    let protocol = values
                        .get("protocol")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid("INTENT_REQUIRED_VALUE"))?;
                    let key = listener_port_key(protocol);
                    if document
                        .get(key)
                        .and_then(Value::as_u64)
                        .is_some_and(|port| port > 0)
                    {
                        return Err(invalid("INTENT_OBJECT_ALREADY_EXISTS"));
                    }
                    let owner = object_id(*object_kind, &key_path(&[key]));
                    for (key, value) in value
                        .as_object()
                        .ok_or_else(|| invalid("INTENT_VALUE_INVALID"))?
                    {
                        self.record_object_field(
                            &owner,
                            key_path(&[key]),
                            SemanticValue::Present(value.clone()),
                        )?;
                    }
                } else {
                    self.record_object_field(
                        &object_id(*object_kind, &path),
                        path,
                        SemanticValue::Present(value),
                    )?;
                }
            }
            ConfigurationIntentAction::UpdateObject {
                object_id: id,
                expected_hash,
                values,
            } => {
                let object = locate(id, expected_hash)?;
                let (path, mut value) =
                    compile_object(profile, object.kind, values, Some(&object.value))?;
                if self.kind == ProgramKind::Mihomo && object.kind == IntentObjectKind::Listener {
                    preserve_shared_authentication(&document, &mut value);
                }
                validate_object_references(self.kind, object.kind, &document, &value)?;
                if object.list {
                    self.record_list_edit(object, Some(value))?;
                } else if self.kind == ProgramKind::Mihomo
                    && object.kind == IntentObjectKind::Listener
                {
                    for (key, value) in value
                        .as_object()
                        .ok_or_else(|| invalid("INTENT_VALUE_INVALID"))?
                    {
                        if key
                            == listener_port_key(
                                object.value["protocol"].as_str().unwrap_or_default(),
                            )
                            && !values.contains_key("port")
                        {
                            continue;
                        }
                        self.record_object_field(
                            id,
                            key_path(&[key]),
                            SemanticValue::Present(value.clone()),
                        )?;
                    }
                } else {
                    for operation in diff_intent_operations(&object.value, &value) {
                        let relative = intent_operation_path(&operation);
                        let replacement = semantic_value_at(&value, &relative)
                            .map_err(|_| invalid("INTENT_VALUE_INVALID"))?;
                        let mut touched = path.clone();
                        touched.extend(relative);
                        self.record_object_field(id, touched, replacement)?;
                    }
                    for relative in touched_native_fields(self.kind, object.kind, values) {
                        let mut touched = path.clone();
                        touched.extend(relative.clone());
                        let replacement = semantic_value_at(&value, &relative)
                            .map_err(|_| invalid("INTENT_VALUE_INVALID"))?;
                        if semantic_value_at(&upstream, &touched).ok().as_ref()
                            != Some(&replacement)
                        {
                            self.record_object_field(id, touched, replacement)?;
                        }
                    }
                }
            }
            ConfigurationIntentAction::RemoveObject {
                object_id: id,
                expected_hash,
            } => {
                let object = locate(id, expected_hash)?;
                if object_referenced(&document, &object.value) {
                    return Err(invalid("INTENT_TARGET_IN_USE"));
                }
                if object.list {
                    self.record_list_edit(object, None)?;
                } else {
                    self.record_object_field(id, object.path.clone(), SemanticValue::Missing)?;
                }
            }
            ConfigurationIntentAction::FollowObject {
                object_id: id,
                expected_hash,
            } => {
                if locate(id, expected_hash).is_err() {
                    let base = parse_semantic_document(self.format, self.base.content.as_bytes())?;
                    let original = locations(self.kind, &base).into_iter().find(|object| {
                        object.id() == *id
                            && semantic_document_hash(&object.value) == *expected_hash
                    });
                    if original.is_none()
                        || !self
                            .guided_intent
                            .path_edits
                            .keys()
                            .any(|key| key.starts_with(id))
                    {
                        return Err(invalid("INTENT_OBJECT_STALE"));
                    }
                }
                self.guided_intent
                    .path_edits
                    .retain(|key, _| !key.starts_with(id));
            }
            _ => return Err(invalid("INTENT_VALUE_INVALID")),
        }
        Ok(())
    }

    fn record_object_field(
        &mut self,
        owner: &str,
        path: SemanticPath,
        value: SemanticValue,
    ) -> Result<()> {
        let key = format!(
            "{owner}:{}",
            hash_bytes(display_semantic_path(&path).as_bytes())
        );
        self.record_intent_edit(
            key,
            IntentPathEdit {
                path,
                value,
                list_edit: None,
            },
        )
    }

    fn record_intent_edit(&mut self, key: String, edit: IntentPathEdit) -> Result<()> {
        if self.guided_intent.path_edits.get(&key) == Some(&edit) {
            self.upstream.claim_intent_path(&key, &edit)?;
        } else {
            self.guided_intent.path_edits.insert(key, edit);
        }
        Ok(())
    }

    fn record_list_edit(
        &mut self,
        object: &ObjectLocation,
        replacement: Option<Value>,
    ) -> Result<()> {
        let id = object.id();
        let original = self
            .guided_intent
            .path_edits
            .get(&id)
            .and_then(|edit| edit.list_edit.as_ref())
            .map(|edit| edit.original.clone())
            .unwrap_or_else(|| Some(object.value.clone()));
        if original.is_none() && replacement.is_none() {
            self.guided_intent.path_edits.remove(&id);
            return Ok(());
        }
        self.record_intent_edit(
            id,
            IntentPathEdit {
                path: object.path.clone(),
                value: SemanticValue::Missing,
                list_edit: Some(IntentListEdit {
                    original,
                    replacement,
                }),
            },
        )
    }
}

fn listener_port_key(protocol: &str) -> &'static str {
    match protocol {
        "mixed" => "mixed-port",
        "http" => "port",
        _ => "socks-port",
    }
}

fn preserve_shared_authentication(document: &Value, update: &mut Value) {
    if let Some(additions) = update.get("authentication").and_then(Value::as_array) {
        let mut accounts = document
            .get("authentication")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for account in additions {
            if !accounts.contains(account) {
                accounts.push(account.clone());
            }
        }
        update["authentication"] = json!(accounts);
    }
}

fn merge_listener_account(
    previous: Option<&Value>,
    user_key: &str,
    password_key: &str,
    user: &str,
    password: &str,
) -> Result<Value> {
    let mut accounts = match previous {
        None => Vec::new(),
        Some(Value::Array(accounts)) => accounts.clone(),
        _ => return Err(invalid("INTENT_VALUE_INVALID")),
    };
    let indexes = accounts
        .iter()
        .enumerate()
        .filter_map(|(index, account)| {
            (account.get(user_key).and_then(Value::as_str) == Some(user)).then_some(index)
        })
        .collect::<Vec<_>>();
    if indexes.len() > 1 {
        return Err(invalid("INTENT_OBJECT_AMBIGUOUS"));
    }
    if let Some(index) = indexes.first() {
        accounts[*index][password_key] = json!(password);
    } else {
        accounts.push(json!({user_key:user, password_key:password}));
    }
    Ok(json!(accounts))
}

fn touched_native_fields(
    program: ProgramKind,
    kind: IntentObjectKind,
    values: &BTreeMap<String, Value>,
) -> Vec<SemanticPath> {
    let mut paths = Vec::new();
    for key in values.keys().map(String::as_str) {
        let native: &[&[&str]] = match (kind, key, program) {
            (IntentObjectKind::Listener, "port", ProgramKind::SingBox) => &[&["listen_port"]],
            (IntentObjectKind::Listener, "port", _) => &[&["port"]],
            (IntentObjectKind::Listener, "listen" | "access", _) => &[&["listen"]],
            (IntentObjectKind::Listener, "username" | "password", ProgramKind::SingBox) => {
                &[&["users"]]
            }
            (IntentObjectKind::Listener, "username" | "password", ProgramKind::Xray) => {
                &[&["settings", "accounts"], &["settings", "auth"]]
            }
            (IntentObjectKind::Listener, "udp", ProgramKind::Xray) => &[&["settings", "udp"]],
            (IntentObjectKind::DnsServer, "server", ProgramKind::SingBox) => {
                &[&["server"], &["path"], &["server_port"]]
            }
            (IntentObjectKind::DnsServer, "server", ProgramKind::Xray) => &[&["address"]],
            (IntentObjectKind::DnsServer, "serverPort", ProgramKind::SingBox) => {
                &[&["server_port"]]
            }
            (IntentObjectKind::DnsServer, "serverPort", ProgramKind::Xray) => &[&["port"]],
            (IntentObjectKind::DnsServer, "bootstrap", ProgramKind::SingBox) => {
                &[&["domain_resolver"]]
            }
            (IntentObjectKind::DnsServer, "timeout", ProgramKind::Xray) => &[&["timeoutMs"]],
            (IntentObjectKind::Tun, "address", _) => &[&["address"]],
            (IntentObjectKind::Tun, "autoRoute", _) => &[&["auto_route"]],
            (IntentObjectKind::Tun, "strictRoute", _) => &[&["strict_route"]],
            (IntentObjectKind::Tun, "dnsMode", _) => &[&["dns_mode"]],
            (IntentObjectKind::Tun, "stack", _) => &[&["stack"]],
            (IntentObjectKind::Tun, "mtu", _) => &[&["mtu"]],
            _ => &[],
        };
        paths.extend(native.iter().map(|path| key_path(path)));
    }
    paths
}

fn validate_object_references(
    program: ProgramKind,
    kind: IntentObjectKind,
    document: &Value,
    value: &Value,
) -> Result<()> {
    if kind == IntentObjectKind::RouteRule {
        let fields = route_values(program, value).ok_or_else(|| invalid("INTENT_VALUE_INVALID"))?;
        if !intent_targets(program, document).iter().any(|target| {
            target.kind == "connection" && fields.get("target") == Some(&json!(target.id))
        }) {
            return Err(invalid("INTENT_TARGET_MISSING"));
        }
    }
    if program == ProgramKind::SingBox
        && kind == IntentObjectKind::DnsServer
        && let Some(reference) = value.get("domain_resolver").and_then(Value::as_str)
    {
        let targets = intent_targets(program, document);
        if value.get("tag").and_then(Value::as_str) == Some(reference)
            || !targets
                .iter()
                .any(|target| target.kind == "dns" && target.id == reference)
        {
            return Err(invalid("INTENT_TARGET_MISSING"));
        }
        let tag = value.get("tag").and_then(Value::as_str).unwrap_or_default();
        let mut next = Some(reference);
        let mut seen = BTreeSet::new();
        while let Some(reference) = next {
            if reference == tag || !seen.insert(reference) {
                return Err(invalid("INTENT_DNS_CYCLE"));
            }
            next = document
                .pointer("/dns/servers")
                .and_then(Value::as_array)
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item.get("tag").and_then(Value::as_str) == Some(reference))
                })
                .and_then(|item| item.get("domain_resolver"))
                .and_then(Value::as_str);
        }
    }
    Ok(())
}

fn object_referenced(document: &Value, object: &Value) -> bool {
    let Some(tag) = object.get("tag").and_then(Value::as_str) else {
        return false;
    };
    fn referenced(value: &Value, tag: &str) -> bool {
        match value {
            Value::Object(fields) => fields.iter().any(|(key, value)| {
                matches!(
                    key.as_str(),
                    "final"
                        | "server"
                        | "domain_resolver"
                        | "detour"
                        | "outbound"
                        | "outboundTag"
                        | "inbound"
                        | "inboundTag"
                ) && (value.as_str() == Some(tag)
                    || value
                        .as_array()
                        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(tag))))
                    || referenced(value, tag)
            }),
            Value::Array(items) => items.iter().any(|item| referenced(item, tag)),
            _ => false,
        }
    }
    referenced(document, tag)
}
