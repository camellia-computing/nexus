use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceUpdateKind {
    UserEdit,
    Refresh,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "surface", content = "settingId", rename_all = "camelCase")]
enum UpstreamOwner {
    Sources,
    Intent(String),
    Details(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpstreamPathWrite {
    owner: UpstreamOwner,
    path: SemanticPath,
    value: SemanticValue,
    sequence: u64,
    #[serde(default)]
    list_edit: Option<IntentListEdit>,
}

/// Current contributions and their explicit write order, not an event history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamState {
    source: Value,
    intent: GuidedIntent,
    details: ManagedIntegrationIntent,
    sequence: u64,
    writes: Vec<UpstreamPathWrite>,
}

impl UpstreamState {
    pub(super) fn new(source: Value) -> Self {
        Self {
            source,
            intent: GuidedIntent::default(),
            details: ManagedIntegrationIntent::default(),
            sequence: 0,
            writes: Vec::new(),
        }
    }

    pub(super) fn reconcile(
        &mut self,
        kind: ProgramKind,
        source: &Value,
        intent: &GuidedIntent,
        details: &ManagedIntegrationIntent,
        source_update: SourceUpdateKind,
    ) -> Result<Value> {
        if source_update == SourceUpdateKind::UserEdit {
            for operation in diff_intent_operations(&self.source, source) {
                if matches!(operation, IntentOperation::ReorderIdentities { .. }) {
                    continue;
                }
                let path = intent_operation_path(&operation);
                let value = value_at(source, &path)?;
                if value_at(&self.source, &path)? == SemanticValue::Missing
                    && let SemanticValue::Present(value) = value
                {
                    self.record_source_addition(path, value);
                } else {
                    self.record(UpstreamOwner::Sources, path, value);
                }
            }
        }
        // A refresh changes the Source contribution, but not its write order.
        self.source = source.clone();
        let settings = self
            .intent
            .values
            .keys()
            .chain(intent.values.keys())
            .cloned()
            .collect::<BTreeSet<_>>();
        for setting in settings {
            if self.intent.values.get(&setting) == intent.values.get(&setting) {
                continue;
            }
            let owner = UpstreamOwner::Intent(setting.clone());
            match intent.values.get(&setting) {
                Some(value) => {
                    validate_guided_value(kind, &setting, value)?;
                    let path = guided_path(kind, &setting).ok_or_else(|| {
                        CamelliaNexusError::invalid_spec("Intent setting has no registered path")
                    })?;
                    self.record(owner, key_path(path), SemanticValue::Present(value.clone()));
                }
                None => self.writes.retain(|write| write.owner != owner),
            }
        }
        for id in self
            .intent
            .path_edits
            .keys()
            .chain(intent.path_edits.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            if self.intent.path_edits.get(&id) == intent.path_edits.get(&id) {
                continue;
            }
            let owner = UpstreamOwner::Intent(id.clone());
            match intent.path_edits.get(&id) {
                Some(edit) => {
                    self.record(owner.clone(), edit.path.clone(), edit.value.clone());
                    if let Some(write) = self
                        .writes
                        .iter_mut()
                        .find(|write| write.owner == owner && write.path == edit.path)
                    {
                        write.list_edit = edit.list_edit.clone();
                    }
                }
                None => self.writes.retain(|write| write.owner != owner),
            }
        }
        self.intent = intent.clone();

        if self.details != *details {
            let mut rendered = json!({});
            let mut previous = json!({});
            apply_dashboard_intent(kind, &mut previous, &self.details)?;
            apply_dashboard_intent(kind, &mut rendered, details)?;
            for (setting, _, mut path) in managed_ownership_targets(kind) {
                if self.details.values.get(setting) == details.values.get(setting) {
                    continue;
                }
                let mut value = value_at(&rendered, &path)?;
                if semantic_values_equal_at(&value_at(&previous, &path)?, &value, &path) {
                    continue;
                }
                // Removing an owned identity removes that element, not its
                // siblings or the entire Source-provided array.
                if value == SemanticValue::Missing
                    && let Some(index) = path
                        .iter()
                        .position(|segment| matches!(segment, SemanticPathSegment::Identity { .. }))
                    && value_at(&rendered, &path[..=index])? == SemanticValue::Missing
                {
                    path.truncate(index + 1);
                    value = SemanticValue::Missing;
                }
                self.record(UpstreamOwner::Details(setting.into()), path, value);
            }
            self.details = details.clone();
        }
        self.document()
    }

    pub(super) fn claim_intent(&mut self, kind: ProgramKind, setting: &str) -> Result<()> {
        let Some(value) = self.intent.values.get(setting).cloned() else {
            return Ok(());
        };
        let path = key_path(guided_path(kind, setting).ok_or_else(|| {
            CamelliaNexusError::invalid_spec("Intent setting has no registered path")
        })?);
        if !semantic_values_equal_at(
            &value_at(&self.document()?, &path)?,
            &SemanticValue::Present(value.clone()),
            &path,
        ) {
            self.record(
                UpstreamOwner::Intent(setting.into()),
                path,
                SemanticValue::Present(value),
            );
        }
        Ok(())
    }

    pub(super) fn claim_intent_path(&mut self, id: &str, edit: &IntentPathEdit) -> Result<()> {
        if let Some(delta) = &edit.list_edit {
            let values = match value_at(&self.document()?, &edit.path)? {
                SemanticValue::Present(Value::Array(items)) => items,
                SemanticValue::Missing => Vec::new(),
                _ => {
                    return Err(CamelliaNexusError::invalid_spec(
                        "Intent list requires a list",
                    ));
                }
            };
            let mut updated = values.clone();
            delta.apply(&mut updated)?;
            if values != updated {
                self.record(
                    UpstreamOwner::Intent(id.into()),
                    edit.path.clone(),
                    edit.value.clone(),
                );
                if let Some(write) = self
                    .writes
                    .iter_mut()
                    .find(|write| write.owner == UpstreamOwner::Intent(id.into()))
                {
                    write.list_edit = Some(delta.clone());
                }
            }
            return Ok(());
        }
        if edit.list_edit.is_none()
            && !semantic_values_equal_at(
                &value_at(&self.document()?, &edit.path)?,
                &edit.value,
                &edit.path,
            )
        {
            self.record(
                UpstreamOwner::Intent(id.into()),
                edit.path.clone(),
                edit.value.clone(),
            );
        }
        Ok(())
    }

    pub(super) fn claim_details(&mut self, kind: ProgramKind, setting: &str) -> Result<()> {
        let primary = managed_setting_path(kind, setting).ok_or_else(|| {
            CamelliaNexusError::invalid_spec("Details setting has no registered path")
        })?;
        if !self.details.values.contains_key(setting) {
            return Err(CamelliaNexusError::invalid_spec(
                "Details setting has no saved value",
            ));
        }
        let current = self.document()?;
        let mut rendered = json!({});
        apply_dashboard_intent(kind, &mut rendered, &self.details)?;
        for (owner, _, path) in managed_ownership_targets(kind) {
            if owner != setting {
                continue;
            }
            let previous = value_at(&current, &path)?;
            let value = value_at(&rendered, &path)?;
            if (path == primary || previous == SemanticValue::Missing)
                && !semantic_values_equal_at(&previous, &value, &path)
            {
                self.record(UpstreamOwner::Details(setting.into()), path, value);
            }
        }
        Ok(())
    }

    fn record_source_addition(&mut self, path: SemanticPath, value: Value) {
        match value {
            Value::Object(fields) if !fields.is_empty() => {
                for (key, value) in fields {
                    let mut child = path.clone();
                    child.push(SemanticPathSegment::Key { key });
                    self.record_source_addition(child, value);
                }
            }
            Value::Array(values)
                if !values.is_empty() && sequence_identities(&values).is_some() =>
            {
                for value in values {
                    let (field, identity) =
                        semantic_identity(&value).expect("Array identities were checked");
                    let mut child = path.clone();
                    child.push(SemanticPathSegment::Identity {
                        field,
                        value: identity,
                    });
                    self.record_source_addition(child, value);
                }
            }
            value => self.record(UpstreamOwner::Sources, path, SemanticValue::Present(value)),
        }
    }

    fn record(&mut self, owner: UpstreamOwner, path: SemanticPath, value: SemanticValue) {
        // Replace only contributions covered by this same owner's new write.
        self.writes
            .retain(|write| write.owner != owner || !write.path.starts_with(&path));
        self.sequence = self.sequence.saturating_add(1);
        self.writes.push(UpstreamPathWrite {
            owner,
            path,
            value,
            sequence: self.sequence,
            list_edit: None,
        });
    }

    pub(super) fn document(&self) -> Result<Value> {
        let mut document = self.source.clone();
        let mut ordered = self.writes.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|write| write.sequence);
        for write in ordered {
            if let Some(edit) = &write.list_edit {
                let mut values = match value_at(&document, &write.path)? {
                    SemanticValue::Present(Value::Array(values)) => values,
                    SemanticValue::Missing => Vec::new(),
                    _ => {
                        return Err(CamelliaNexusError::invalid_spec(
                            "An Intent list crosses a non-list value",
                        ));
                    }
                };
                edit.apply(&mut values)?;
                write_value(
                    &mut document,
                    &write.path,
                    &SemanticValue::Present(json!(values)),
                )?;
                continue;
            }
            let value = if write.owner == UpstreamOwner::Sources {
                value_at(&self.source, &write.path)?
            } else {
                write.value.clone()
            };
            write_value(&mut document, &write.path, &value)?;
        }
        Ok(document)
    }
}

fn value_at(root: &Value, path: &[SemanticPathSegment]) -> Result<SemanticValue> {
    semantic_value_at(root, path)
        .map_err(|conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason))
}

fn write_value(
    root: &mut Value,
    path: &[SemanticPathSegment],
    value: &SemanticValue,
) -> Result<()> {
    if matches!(value, SemanticValue::Missing) {
        if matches!(value_at(root, path)?, SemanticValue::Missing) {
            return Ok(());
        }
        return delete_semantic_path(root, path).map_err(|conflict| {
            CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason)
        });
    }
    // A later explicit descendant write may reconstruct the necessary chain
    // after a parent was removed or replaced by an earlier operation.
    for index in 0..path.len() {
        let expected_array = matches!(path[index], SemanticPathSegment::Identity { .. });
        let ancestor = value_at(root, &path[..index])?;
        let valid = matches!(&ancestor, SemanticValue::Present(value) if
            if expected_array { value.is_array() } else { value.is_object() });
        if !valid {
            let container = if expected_array { json!([]) } else { json!({}) };
            set_semantic_path_creating_anchors(root, &path[..index], container).map_err(
                |conflict| CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason),
            )?;
        }
    }
    if let SemanticValue::Present(value) = value {
        set_semantic_path_creating_anchors(root, path, value.clone()).map_err(|conflict| {
            CamelliaNexusError::new(ErrorCode::ConfigConflict, conflict.reason)
        })?;
    }
    Ok(())
}
