use camellia_nexus_core::{
    CamelliaNexusError, ManagedConfigSpec, Result, SingBoxClashDashboardSpec, SingBoxDashboardSpec,
};
use serde_json::{Map, Value, json};

const MANAGED_API_TAG: &str = "camellia-nexus-api";
const MANAGED_CLASH_UI: &str = "clash-dashboard";

pub fn merge_object(target: &mut Map<String, Value>, source: Map<String, Value>) {
    for (key, value) in source {
        match target.get_mut(&key) {
            Some(current) => merge_value(current, value),
            None => {
                target.insert(key, value);
            }
        }
    }
}

pub fn apply_features(root: &mut Map<String, Value>, managed: &ManagedConfigSpec) -> Result<bool> {
    let api_changed = apply_api(root, managed.sing_box_dashboard.as_ref())?;
    let clash_changed = apply_clash_api(root, managed.sing_box_clash_dashboard.as_ref())?;
    Ok(api_changed || clash_changed)
}

fn merge_value(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => merge_object(target, source),
        (Value::Array(target), Value::Array(source)) => merge_tagged_array(target, source),
        (target, source) => *target = source,
    }
}

fn merge_tagged_array(target: &mut Vec<Value>, source: Vec<Value>) {
    for value in source {
        let tag = value.get("tag").and_then(Value::as_str);
        if let Some(index) = tag.and_then(|tag| {
            target
                .iter()
                .position(|existing| existing.get("tag").and_then(Value::as_str) == Some(tag))
        }) {
            target[index] = value;
        } else {
            target.push(value);
        }
    }
}

fn apply_api(
    root: &mut Map<String, Value>,
    dashboard: Option<&SingBoxDashboardSpec>,
) -> Result<bool> {
    match dashboard {
        Some(dashboard) => insert_api(root, dashboard),
        None => remove_api(root),
    }
}

fn remove_api(root: &mut Map<String, Value>) -> Result<bool> {
    let Some(value) = root.get_mut("services") else {
        return Ok(false);
    };
    let services = value
        .as_array_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("sing-box services must be an array"))?;
    let previous_len = services.len();
    services.retain(|service| !is_managed_api(service));
    Ok(services.len() != previous_len)
}

fn insert_api(root: &mut Map<String, Value>, dashboard: &SingBoxDashboardSpec) -> Result<bool> {
    let services = match root.get_mut("services") {
        Some(value) => value.as_array_mut().ok_or_else(|| {
            CamelliaNexusError::invalid_spec("sing-box services must be an array")
        })?,
        None => {
            root.insert("services".into(), Value::Array(Vec::new()));
            root.get_mut("services")
                .and_then(Value::as_array_mut)
                .expect("new services array")
        }
    };
    let service = json!({
        "type": "api",
        "tag": MANAGED_API_TAG,
        "listen": "127.0.0.1",
        "listen_port": dashboard.listen_port,
        "dashboard": {
            "enabled": true,
            "update_interval": dashboard.update_interval,
        },
    });
    let managed_indexes: Vec<_> = services
        .iter()
        .enumerate()
        .filter_map(|(index, existing)| is_managed_api(existing).then_some(index))
        .collect();
    if managed_indexes.len() == 1
        && managed_indexes[0] + 1 == services.len()
        && services.last() == Some(&service)
    {
        return Ok(false);
    }
    services.retain(|service| !is_managed_api(service));
    services.push(service);
    Ok(true)
}

fn is_managed_api(service: &Value) -> bool {
    service.get("type").and_then(Value::as_str) == Some("api")
        && service.get("tag").and_then(Value::as_str) == Some(MANAGED_API_TAG)
}

fn apply_clash_api(
    root: &mut Map<String, Value>,
    dashboard: Option<&SingBoxClashDashboardSpec>,
) -> Result<bool> {
    let Some(dashboard) = dashboard else {
        return remove_clash_api(root);
    };
    if !root.contains_key("experimental") {
        root.insert("experimental".into(), Value::Object(Map::new()));
    }
    let experimental = root
        .get_mut("experimental")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CamelliaNexusError::invalid_spec("sing-box experimental must be an object")
        })?;
    let mut clash = experimental
        .get("clash_api")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let clash_object = clash
        .as_object_mut()
        .ok_or_else(|| CamelliaNexusError::invalid_spec("sing-box clash_api must be an object"))?;
    clash_object.insert(
        "external_controller".into(),
        Value::String(format!("127.0.0.1:{}", dashboard.listen_port)),
    );
    clash_object.insert("external_ui".into(), Value::String(MANAGED_CLASH_UI.into()));
    match &dashboard.download_url {
        Some(url) => {
            clash_object.insert(
                "external_ui_download_url".into(),
                Value::String(url.clone()),
            );
        }
        None => {
            clash_object.remove("external_ui_download_url");
        }
    }
    let changed = experimental.get("clash_api") != Some(&clash);
    experimental.insert("clash_api".into(), clash);
    Ok(changed)
}

fn remove_clash_api(root: &mut Map<String, Value>) -> Result<bool> {
    let Some(experimental_value) = root.get_mut("experimental") else {
        return Ok(false);
    };
    let experimental = experimental_value.as_object_mut().ok_or_else(|| {
        CamelliaNexusError::invalid_spec("sing-box experimental must be an object")
    })?;
    let managed = experimental
        .get("clash_api")
        .and_then(Value::as_object)
        .is_some_and(|clash| {
            clash.get("external_ui").and_then(Value::as_str) == Some(MANAGED_CLASH_UI)
                && clash
                    .get("external_controller")
                    .and_then(Value::as_str)
                    .is_some_and(|value| value.starts_with("127.0.0.1:"))
        });
    if !managed {
        return Ok(false);
    }
    let clash = experimental
        .get_mut("clash_api")
        .and_then(Value::as_object_mut)
        .expect("managed Clash API object");
    clash.remove("external_controller");
    clash.remove("external_ui");
    clash.remove("external_ui_download_url");
    if clash.is_empty() {
        experimental.remove("clash_api");
    }
    if experimental.is_empty() {
        root.remove("experimental");
    }
    Ok(true)
}

#[cfg(test)]
pub(crate) fn managed_clash_ui() -> &'static str {
    MANAGED_CLASH_UI
}
