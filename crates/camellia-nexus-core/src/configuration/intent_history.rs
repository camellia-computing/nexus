use super::*;

pub const MAX_INTENT_HISTORY_RECORDS: usize = 64;
pub const MAX_INTENT_HISTORY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntentHistoryDirection {
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IntentHistoryChoice {
    KeepCurrent,
    Restore,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentHistory {
    pub records: Vec<IntentHistoryRecord>,
    pub redo_order: Vec<String>,
}

/// Values and inverse operations live only in the protected payload store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentHistoryRecord {
    pub record_id: String,
    pub payload_ref: String,
    pub payload_hash: String,
    pub payload_bytes: u64,
    pub label_key: String,
    pub subject_key: String,
    pub created_unix_ms: u64,
    pub undone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentHistorySummary {
    pub record_id: String,
    pub label_key: String,
    pub subject_key: String,
    pub created_unix_ms: u64,
    pub can_undo: bool,
    pub can_redo: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentHistoryConflict {
    pub conflict_id: String,
    pub semantic_paths: Vec<String>,
    pub protected: bool,
    pub current: Vec<Option<SemanticValue>>,
    pub restoring: Vec<Option<SemanticValue>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentHistoryPayload {
    pub program: ProgramKind,
    contributions: Vec<ContributionDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContributionDelta {
    id: String,
    before_scalar: Option<Value>,
    after_scalar: Option<Value>,
    before_edit: Option<IntentPathEdit>,
    after_edit: Option<IntentPathEdit>,
    before_writes: Vec<upstream::UpstreamPathWrite>,
    after_writes: Vec<upstream::UpstreamPathWrite>,
    touched: Vec<HistoryPathDelta>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HistoryPathDelta {
    path: SemanticPath,
    before: SemanticValue,
    after: SemanticValue,
}

impl IntentHistory {
    pub fn record(&self, id: &str) -> Result<&IntentHistoryRecord> {
        self.records
            .iter()
            .find(|record| record.record_id == id)
            .ok_or_else(|| history_error("INTENT_HISTORY_UNAVAILABLE"))
    }

    pub fn summaries(&self) -> Vec<IntentHistorySummary> {
        self.records
            .iter()
            .rev()
            .map(|record| IntentHistorySummary {
                record_id: record.record_id.clone(),
                label_key: record.label_key.clone(),
                subject_key: record.subject_key.clone(),
                created_unix_ms: record.created_unix_ms,
                can_undo: !record.undone,
                can_redo: record.undone && self.redo_order.last() == Some(&record.record_id),
            })
            .collect()
    }

    pub fn append(&mut self, record: IntentHistoryRecord) -> Result<()> {
        if record.payload_bytes > MAX_INTENT_HISTORY_BYTES
            || self
                .records
                .iter()
                .any(|item| item.record_id == record.record_id)
        {
            return Err(history_error("INTENT_HISTORY_LIMIT"));
        }
        self.records.retain(|record| !record.undone);
        self.redo_order.clear();
        self.records.push(record);
        while self.records.len() > MAX_INTENT_HISTORY_RECORDS
            || self
                .records
                .iter()
                .map(|item| u128::from(item.payload_bytes))
                .sum::<u128>()
                > u128::from(MAX_INTENT_HISTORY_BYTES)
        {
            self.records.remove(0);
        }
        Ok(())
    }

    fn check_direction(&self, id: &str, direction: IntentHistoryDirection) -> Result<()> {
        let record = self.record(id)?;
        if match direction {
            IntentHistoryDirection::Undo => record.undone,
            IntentHistoryDirection::Redo => {
                !record.undone || self.redo_order.last().is_none_or(|last| last != id)
            }
        } {
            return Err(history_error("INTENT_HISTORY_UNAVAILABLE"));
        }
        Ok(())
    }

    fn complete(&mut self, id: &str, direction: IntentHistoryDirection) {
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.record_id == id)
        {
            record.undone = direction == IntentHistoryDirection::Undo;
        }
        match direction {
            IntentHistoryDirection::Undo => self.redo_order.push(id.into()),
            IntentHistoryDirection::Redo => {
                self.redo_order.pop();
            }
        }
    }
}

impl IntentHistoryPayload {
    pub fn between(
        before: &ConfigurationState,
        after: &ConfigurationState,
    ) -> Result<Option<Self>> {
        if before.kind != after.kind {
            return Err(history_error("INTENT_HISTORY_UNAVAILABLE"));
        }
        let original = before.upstream_document()?;
        let updated = after.upstream_document()?;
        let operations = diff_intent_operations(&original, &updated);
        let ids = before
            .guided_intent
            .values
            .keys()
            .chain(after.guided_intent.values.keys())
            .chain(before.guided_intent.path_edits.keys())
            .chain(after.guided_intent.path_edits.keys())
            .cloned()
            .chain(before.upstream.changed_intent_owners(&after.upstream))
            .collect::<BTreeSet<_>>();
        let mut contributions = Vec::new();
        for id in ids {
            let mut delta = ContributionDelta {
                before_scalar: before.guided_intent.values.get(&id).cloned(),
                after_scalar: after.guided_intent.values.get(&id).cloned(),
                before_edit: before.guided_intent.path_edits.get(&id).cloned(),
                after_edit: after.guided_intent.path_edits.get(&id).cloned(),
                before_writes: before.upstream.intent_writes(&id),
                after_writes: after.upstream.intent_writes(&id),
                id,
                touched: Vec::new(),
            };
            if delta.before_scalar == delta.after_scalar
                && delta.before_edit == delta.after_edit
                && delta.before_writes == delta.after_writes
            {
                continue;
            }
            let paths = delta
                .before_writes
                .iter()
                .chain(&delta.after_writes)
                .map(|write| &write.path)
                .chain(
                    delta
                        .before_edit
                        .iter()
                        .chain(&delta.after_edit)
                        .map(|edit| &edit.path),
                )
                .collect::<Vec<_>>();
            for operation in &operations {
                let path = intent_operation_path(operation);
                if paths.iter().any(|owned| paths_overlap(owned, &path)) {
                    delta.touched.push(HistoryPathDelta {
                        before: upstream::value_at(&original, &path)?,
                        after: upstream::value_at(&updated, &path)?,
                        path,
                    });
                }
            }
            contributions.push(delta);
        }
        Ok((!contributions.is_empty()).then_some(Self {
            program: before.kind,
            contributions,
        }))
    }

    pub fn prepare(
        &self,
        state: &ConfigurationState,
        record_id: &str,
        direction: IntentHistoryDirection,
        choices: &BTreeMap<String, IntentHistoryChoice>,
        now: u64,
    ) -> Result<(ConfigurationState, Vec<IntentHistoryConflict>)> {
        state.intent_history.check_direction(record_id, direction)?;
        let record = state.intent_history.record(record_id)?;
        if self.program != state.kind
            || record.payload_hash != hash_bytes(&serde_json::to_vec(self)?)
        {
            return Err(history_error("INTENT_HISTORY_UNAVAILABLE"));
        }
        let undo = direction == IntentHistoryDirection::Undo;
        let document = state.upstream_document()?;
        let mut next = state.clone();
        let mut conflicts = Vec::new();
        let mut known_choices = BTreeSet::new();
        for delta in &self.contributions {
            let (
                expected_scalar,
                target_scalar,
                expected_edit,
                target_edit,
                expected_writes,
                target_writes,
            ) = if undo {
                (
                    &delta.after_scalar,
                    &delta.before_scalar,
                    &delta.after_edit,
                    &delta.before_edit,
                    &delta.after_writes,
                    &delta.before_writes,
                )
            } else {
                (
                    &delta.before_scalar,
                    &delta.after_scalar,
                    &delta.before_edit,
                    &delta.after_edit,
                    &delta.before_writes,
                    &delta.after_writes,
                )
            };
            let current_values = delta
                .touched
                .iter()
                .map(|entry| upstream::value_at(&document, &entry.path))
                .collect::<Result<Vec<_>>>()?;
            let overlap = state.guided_intent.values.get(&delta.id) != expected_scalar.as_ref()
                || state.guided_intent.path_edits.get(&delta.id) != expected_edit.as_ref()
                || !state
                    .upstream
                    .intent_writes_match(&delta.id, expected_writes)
                || delta
                    .touched
                    .iter()
                    .zip(&current_values)
                    .any(|(entry, current)| {
                        !semantic_values_equal_at(
                            current,
                            if undo { &entry.after } else { &entry.before },
                            &entry.path,
                        )
                    });
            if overlap {
                let mut restoring = state.upstream.clone();
                restoring.restore_intent_owner(
                    &delta.id,
                    target_writes,
                    target_scalar.as_ref(),
                    target_edit.as_ref(),
                    true,
                );
                let restoring_document = restoring.document()?;
                let restoring_values = delta
                    .touched
                    .iter()
                    .map(|entry| upstream::value_at(&restoring_document, &entry.path))
                    .collect::<Result<Vec<_>>>()?;
                let conflict_id = hash_bytes(&serde_json::to_vec(&(
                    record_id,
                    direction,
                    &delta.id,
                    &current_values,
                    &restoring_values,
                ))?);
                known_choices.insert(conflict_id.clone());
                let protected = delta.touched.iter().any(|entry| {
                    protected_semantic(&entry.path, &entry.before)
                        || protected_semantic(&entry.path, &entry.after)
                }) || delta
                    .touched
                    .iter()
                    .zip(&current_values)
                    .any(|(entry, value)| protected_semantic(&entry.path, value))
                    || delta
                        .touched
                        .iter()
                        .zip(&restoring_values)
                        .any(|(entry, value)| protected_semantic(&entry.path, value));
                conflicts.push(IntentHistoryConflict {
                    conflict_id: conflict_id.clone(),
                    semantic_paths: delta
                        .touched
                        .iter()
                        .map(|entry| display_semantic_path(&entry.path))
                        .collect(),
                    protected,
                    current: current_values
                        .into_iter()
                        .map(|value| (!protected).then_some(value))
                        .collect(),
                    restoring: restoring_values
                        .into_iter()
                        .map(|value| (!protected).then_some(value))
                        .collect(),
                });
                match choices.get(&conflict_id) {
                    None => continue,
                    Some(IntentHistoryChoice::KeepCurrent) => continue,
                    Some(IntentHistoryChoice::Restore) => {}
                }
            }
            set_entry(
                &mut next.guided_intent.values,
                &delta.id,
                target_scalar.as_ref(),
            );
            set_entry(
                &mut next.guided_intent.path_edits,
                &delta.id,
                target_edit.as_ref(),
            );
            next.upstream.restore_intent_owner(
                &delta.id,
                target_writes,
                target_scalar.as_ref(),
                target_edit.as_ref(),
                overlap,
            );
        }
        if choices.keys().any(|id| !known_choices.contains(id)) {
            return Err(history_error("INTENT_PLAN_STALE"));
        }
        if conflicts
            .iter()
            .any(|conflict| !choices.contains_key(&conflict.conflict_id))
        {
            return Ok((state.clone(), conflicts));
        }
        next.intent_history.complete(record_id, direction);
        let revision = next.state_revision;
        next.rebuild_desired(now)?;
        if next.state_revision == revision {
            next.state_revision = next.state_revision.saturating_add(1);
        }
        Ok((next, conflicts))
    }
}

fn set_entry<T: Clone>(values: &mut BTreeMap<String, T>, id: &str, value: Option<&T>) {
    match value {
        Some(value) => {
            values.insert(id.into(), value.clone());
        }
        None => {
            values.remove(id);
        }
    }
}

fn paths_overlap(a: &SemanticPath, b: &SemanticPath) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

fn protected_semantic(path: &SemanticPath, value: &SemanticValue) -> bool {
    path.iter().any(
        |part| matches!(part, SemanticPathSegment::Key {key} if intent_catalog::protected_key(key)),
    ) || matches!(value, SemanticValue::Present(value) if intent_catalog::contains_protected_value(value))
}

fn history_error(key: &str) -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::ConfigConflict,
        "Review this change before restoring it",
    )
    .with_message_key(key)
}

#[cfg(test)]
mod tests;
