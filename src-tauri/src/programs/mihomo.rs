use camellia_nexus_core::{ManagedConfigSpec, MihomoDashboardSpec, Result};
use serde_yaml_ng::{Mapping, Value};

const MANAGED_UI_DIRECTORY: &str = "camellia-nexus-mihomo-dashboard";

pub fn merge_mapping(target: &mut Mapping, source: Mapping) {
    for (key, value) in source {
        let section = key.as_str().map(str::to_owned);
        match target.get_mut(&key) {
            Some(current) => merge_value(current, value, section.as_deref()),
            None => {
                target.insert(key, value);
            }
        }
    }
}

pub fn apply_features(root: &mut Mapping, managed: &ManagedConfigSpec) -> Result<bool> {
    let before = root.clone();
    match managed.mihomo_dashboard.as_ref() {
        Some(dashboard) => insert_dashboard(root, dashboard),
        None => remove_dashboard(root),
    }
    Ok(before != *root)
}

fn merge_value(target: &mut Value, source: Value, section: Option<&str>) {
    match (target, source) {
        (Value::Mapping(target), Value::Mapping(source)) => merge_mapping(target, source),
        (Value::Sequence(target), Value::Sequence(source)) => {
            // Mihomo resolves these sections by name. Replacing in place keeps the source-order
            // priority of surrounding entries, while other ordered lists (notably rules) append.
            if section
                .is_some_and(|section| matches!(section, "proxies" | "proxy-groups" | "listeners"))
            {
                merge_named_sequence(target, source);
            } else {
                target.extend(source);
            }
        }
        (target, source) => *target = source,
    }
}

fn merge_named_sequence(target: &mut Vec<Value>, source: Vec<Value>) {
    for value in source {
        let name = item_name(&value);
        if let Some(index) = name.and_then(|name| {
            target
                .iter()
                .position(|existing| item_name(existing) == Some(name))
        }) {
            target[index] = value;
        } else {
            target.push(value);
        }
    }
}

fn item_name(value: &Value) -> Option<&str> {
    value
        .as_mapping()?
        .get(Value::String("name".into()))?
        .as_str()
}

fn insert_dashboard(root: &mut Mapping, dashboard: &MihomoDashboardSpec) {
    insert_string(
        root,
        "external-controller",
        format!("127.0.0.1:{}", dashboard.listen_port),
    );
    insert_string(root, "external-ui", MANAGED_UI_DIRECTORY);
    match &dashboard.download_url {
        Some(url) => insert_string(root, "external-ui-url", url),
        None => {
            root.remove(Value::String("external-ui-url".into()));
        }
    }
}

fn remove_dashboard(root: &mut Mapping) {
    let managed = string_value(root, "external-ui") == Some(MANAGED_UI_DIRECTORY)
        && string_value(root, "external-controller")
            .is_some_and(|controller| controller.starts_with("127.0.0.1:"));
    if !managed {
        return;
    }
    for key in ["external-controller", "external-ui", "external-ui-url"] {
        root.remove(Value::String(key.into()));
    }
}

fn insert_string(root: &mut Mapping, key: &str, value: impl Into<String>) {
    root.insert(Value::String(key.into()), Value::String(value.into()));
}

fn string_value<'a>(root: &'a Mapping, key: &str) -> Option<&'a str> {
    root.get(Value::String(key.into())).and_then(Value::as_str)
}

#[cfg(test)]
pub(crate) fn managed_ui_directory() -> &'static str {
    MANAGED_UI_DIRECTORY
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(yaml: &str) -> Mapping {
        serde_yaml_ng::from_str::<Value>(yaml)
            .expect("yaml")
            .as_mapping()
            .cloned()
            .expect("mapping")
    }

    #[test]
    fn named_sections_replace_in_place_and_rules_append() {
        let mut target = mapping(
            "proxies:\n  - name: edge\n    type: direct\nrules:\n  - DOMAIN,first.test,DIRECT\n",
        );
        merge_mapping(
            &mut target,
            mapping(
                "proxies:\n  - name: edge\n    type: socks5\n  - name: backup\n    type: direct\nrules:\n  - MATCH,edge\n",
            ),
        );
        let value = Value::Mapping(target);
        assert_eq!(value["proxies"][0]["type"].as_str(), Some("socks5"));
        assert_eq!(value["proxies"][1]["name"].as_str(), Some("backup"));
        assert_eq!(value["rules"][0].as_str(), Some("DOMAIN,first.test,DIRECT"));
        assert_eq!(value["rules"][1].as_str(), Some("MATCH,edge"));
    }

    #[test]
    fn managed_dashboard_is_idempotent_and_preserves_secret() {
        let mut root = mapping("secret: keep-me\nexternal-ui-name: custom\n");
        let managed = ManagedConfigSpec {
            mihomo_dashboard: Some(MihomoDashboardSpec {
                listen_port: 9092,
                download_url: Some("https://example.test/ui.zip".into()),
            }),
            ..ManagedConfigSpec::default()
        };
        assert!(apply_features(&mut root, &managed).expect("apply"));
        assert!(!apply_features(&mut root, &managed).expect("apply again"));
        assert_eq!(string_value(&root, "secret"), Some("keep-me"));
        assert_eq!(string_value(&root, "external-ui-name"), Some("custom"));
        assert_eq!(
            string_value(&root, "external-controller"),
            Some("127.0.0.1:9092")
        );
    }

    #[test]
    fn disabling_only_removes_a_managed_dashboard() {
        let mut root = mapping(
            "external-controller: 127.0.0.1:9092\nexternal-ui: camellia-nexus-mihomo-dashboard\nsecret: keep-me\n",
        );
        assert!(apply_features(&mut root, &ManagedConfigSpec::default()).expect("remove"));
        assert_eq!(string_value(&root, "secret"), Some("keep-me"));
        assert!(string_value(&root, "external-controller").is_none());

        let mut user = mapping("external-controller: 0.0.0.0:9090\nexternal-ui: user-ui\n");
        assert!(!apply_features(&mut user, &ManagedConfigSpec::default()).expect("preserve"));
        assert_eq!(string_value(&user, "external-ui"), Some("user-ui"));
    }
}
