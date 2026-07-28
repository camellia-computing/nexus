use camellia_nexus_core::{CamelliaNexusError, ManagedConfigSpec, Result, XrayDashboardSpec};
use serde_json::{Map, Value, json};

const MANAGED_API_TAG: &str = "camellia-nexus-api";
const MANAGED_METRICS_TAG: &str = "camellia-nexus-metrics";
const API_SERVICES: [&str; 5] = [
    "HandlerService",
    "LoggerService",
    "StatsService",
    "RoutingService",
    "ReflectionService",
];

pub fn merge_object(
    target: &mut Map<String, Value>,
    source: Map<String, Value>,
    append_outbounds: bool,
) {
    for (key, value) in source {
        if matches!(key.as_str(), "inbounds" | "outbounds") {
            merge_tagged_section(target, key, value, append_outbounds);
        } else {
            target.insert(key, value);
        }
    }
}

fn merge_tagged_section(
    target: &mut Map<String, Value>,
    key: String,
    value: Value,
    append_outbounds: bool,
) {
    let source_values = match value {
        Value::Array(values) => values,
        other => {
            target.insert(key, other);
            return;
        }
    };
    let target_value = target
        .entry(key.clone())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(target_values) = target_value.as_array_mut() {
        let prepend_new = key == "outbounds" && !append_outbounds;
        merge_tagged_array(target_values, source_values, prepend_new);
    } else {
        *target_value = Value::Array(source_values);
    }
}

fn merge_tagged_array(target: &mut Vec<Value>, source: Vec<Value>, prepend_new: bool) {
    let mut new_values = Vec::new();
    for value in source {
        let tag = value.get("tag").and_then(Value::as_str);
        if let Some(index) = tag.and_then(|tag| {
            target
                .iter()
                .position(|existing| existing.get("tag").and_then(Value::as_str) == Some(tag))
        }) {
            target[index] = value;
        } else {
            new_values.push(value);
        }
    }
    if prepend_new {
        new_values.append(target);
        *target = new_values;
    } else {
        target.extend(new_values);
    }
}

pub fn apply_features(root: &mut Map<String, Value>, managed: &ManagedConfigSpec) -> Result<bool> {
    let before = Value::Object(root.clone());
    match managed.xray_dashboard.as_ref() {
        Some(dashboard) => insert_dashboard(root, dashboard)?,
        None => remove_dashboard(root)?,
    }
    Ok(before != Value::Object(root.clone()))
}

fn insert_dashboard(root: &mut Map<String, Value>, dashboard: &XrayDashboardSpec) -> Result<()> {
    insert_api(root, dashboard.api_port)?;
    insert_metrics(root, dashboard.metrics_port)?;
    insert_stats(root)?;
    insert_policy_stats(root)?;
    Ok(())
}

fn insert_api(root: &mut Map<String, Value>, port: u16) -> Result<()> {
    let api = ensure_object(root, "api", "Xray api must be an object")?;
    api.insert("tag".into(), Value::String(MANAGED_API_TAG.into()));
    api.insert("listen".into(), Value::String(format!("127.0.0.1:{port}")));
    let services = api
        .entry("services")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("Xray api.services must be an array"))?;
    for service in API_SERVICES {
        if !services
            .iter()
            .any(|existing| existing.as_str() == Some(service))
        {
            services.push(Value::String(service.into()));
        }
    }
    Ok(())
}

fn insert_metrics(root: &mut Map<String, Value>, port: u16) -> Result<()> {
    let metrics = ensure_object(root, "metrics", "Xray metrics must be an object")?;
    metrics.insert("tag".into(), Value::String(MANAGED_METRICS_TAG.into()));
    metrics.insert("listen".into(), Value::String(format!("127.0.0.1:{port}")));
    Ok(())
}

fn insert_stats(root: &mut Map<String, Value>) -> Result<()> {
    match root.get("stats") {
        Some(Value::Object(_)) => Ok(()),
        Some(_) => Err(CamelliaNexusError::invalid_spec(
            "Xray stats must be an object",
        )),
        None => {
            root.insert("stats".into(), json!({}));
            Ok(())
        }
    }
}

fn insert_policy_stats(root: &mut Map<String, Value>) -> Result<()> {
    let policy = ensure_object(root, "policy", "Xray policy must be an object")?;
    let system = policy
        .entry("system")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("Xray policy.system must be an object"))?;
    for key in [
        "statsInboundUplink",
        "statsInboundDownlink",
        "statsOutboundUplink",
        "statsOutboundDownlink",
    ] {
        system.insert(key.into(), Value::Bool(true));
    }
    Ok(())
}

fn remove_dashboard(root: &mut Map<String, Value>) -> Result<()> {
    if root
        .get("api")
        .and_then(Value::as_object)
        .and_then(|api| api.get("tag"))
        .and_then(Value::as_str)
        == Some(MANAGED_API_TAG)
    {
        root.remove("api");
    }
    if root
        .get("metrics")
        .and_then(Value::as_object)
        .and_then(|metrics| metrics.get("tag"))
        .and_then(Value::as_str)
        == Some(MANAGED_METRICS_TAG)
    {
        root.remove("metrics");
    }
    Ok(())
}

fn ensure_object<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
    error: &str,
) -> Result<&'a mut Map<String, Value>> {
    let value = root.entry(key).or_insert_with(|| Value::Object(Map::new()));
    value
        .as_object_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec(error))
}

#[cfg(test)]
pub(crate) fn managed_api_tag() -> &'static str {
    MANAGED_API_TAG
}

#[cfg(test)]
pub(crate) fn managed_metrics_tag() -> &'static str {
    MANAGED_METRICS_TAG
}
