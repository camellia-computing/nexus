use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationConflictOrigin {
    Candidate,
    Draft,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationConflictReference {
    pub origin: ConfigurationConflictOrigin,
    pub conflict_id: String,
    pub fingerprint: String,
}

impl ConfigurationConflictReference {
    pub fn new(origin: ConfigurationConflictOrigin, conflict: &FinalMergeConflict) -> Self {
        Self {
            origin,
            conflict_id: conflict.conflict_id.clone(),
            fingerprint: hash_bytes(&serde_json::to_vec(conflict).expect("serializable conflict")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConfigurationConflictAction {
    Resolve {
        reference: ConfigurationConflictReference,
        resolution: FinalConflictResolution,
    },
    Undo {
        #[serde(rename = "resolutionOperationId")]
        resolution_operation_id: String,
    },
    Redo {
        #[serde(rename = "resolutionOperationId")]
        resolution_operation_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveConfigurationConflictRequest {
    pub operation_id: String,
    pub expected_state_revision: u64,
    pub editor_session_id: Option<String>,
    pub expected_draft_revision: Option<u64>,
    pub action: ConfigurationConflictAction,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationConflictReceipt {
    pub request: ResolveConfigurationConflictRequest,
    pub conflict: FinalMergeConflict,
    pub origin: ConfigurationConflictOrigin,
    pub resolution: FinalConflictResolution,
    pub undone: bool,
    pub generation: u64,
}

pub fn project_configuration_conflict(
    origin: ConfigurationConflictOrigin,
    conflict: &FinalMergeConflict,
) -> FinalConflictProjection {
    FinalConflictProjection {
        reference: ConfigurationConflictReference::new(origin, conflict),
        conflict_id: format!(
            "{}:{}",
            match origin {
                ConfigurationConflictOrigin::Candidate => "candidate",
                ConfigurationConflictOrigin::Draft => "draft",
            },
            conflict.conflict_id
        ),
        semantic_path: conflict.semantic_path.clone(),
        segments: conflict.path.clone(),
        kind: conflict.kind,
        base_value: conflict.base_value.clone(),
        upstream_value: conflict.upstream_value.clone(),
        user_value: conflict.user_value.clone(),
        can_merge: conflict.can_merge,
    }
}

fn conflict_changed() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::ConfigConflict,
        "Conflict changed; review the latest choices",
    )
    .with_message_key("CONFIGURATION_CONFLICT_STALE")
}

fn chosen_value(
    conflict: &FinalMergeConflict,
    resolution: &FinalConflictResolution,
) -> SemanticValue {
    match resolution {
        FinalConflictResolution::AcceptUpstream => conflict.upstream_value.clone(),
        FinalConflictResolution::KeepMine => conflict.user_value.clone(),
        FinalConflictResolution::ManualEdit { value } => value.clone(),
    }
}

impl ConfigurationState {
    pub fn resolve_configuration_conflict(
        &mut self,
        request: ResolveConfigurationConflictRequest,
        now: u64,
    ) -> Result<bool> {
        if let Some(receipt) = self
            .conflict_operations
            .iter()
            .find(|item| item.request.operation_id == request.operation_id)
        {
            if receipt.request != request {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Operation identity belongs to a different request",
                )
                .with_message_key("CONFIGURATION_OPERATION_MISMATCH"));
            }
            return Ok(false);
        }
        if uuid::Uuid::parse_str(&request.operation_id).is_err() {
            return Err(CamelliaNexusError::invalid_spec(
                "Invalid configuration operation identity",
            ));
        }
        if request.expected_state_revision != self.state_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration state changed",
            )
            .with_message_key("CONFIGURATION_STATE_STALE"));
        }
        match &self.editor_session {
            Some(draft)
                if request.editor_session_id.as_deref() != Some(&draft.session_id)
                    || request.expected_draft_revision != Some(draft.draft_revision) =>
            {
                return Err(conflict_changed());
            }
            None if request
                .expected_draft_revision
                .is_some_and(|revision| revision != 0) =>
            {
                return Err(conflict_changed());
            }
            _ => {}
        }
        let mut next = self.clone();
        let (origin, conflict, resolution, undo) = match &request.action {
            ConfigurationConflictAction::Resolve {
                reference,
                resolution,
            } => {
                let conflict = match reference.origin {
                    ConfigurationConflictOrigin::Candidate => next
                        .final_edit
                        .conflicts
                        .iter()
                        .find(|item| item.conflict_id == reference.conflict_id),
                    ConfigurationConflictOrigin::Draft => {
                        next.editor_session.as_ref().and_then(|draft| {
                            draft.conflicts.iter().find(|item| {
                                item.conflict_id == reference.conflict_id
                                    && draft.unresolved_conflict_ids.contains(&item.conflict_id)
                            })
                        })
                    }
                }
                .ok_or_else(conflict_changed)?;
                if ConfigurationConflictReference::new(reference.origin, conflict) != *reference {
                    return Err(conflict_changed());
                }
                (
                    reference.origin,
                    conflict.clone(),
                    resolution.clone(),
                    false,
                )
            }
            ConfigurationConflictAction::Undo {
                resolution_operation_id,
            }
            | ConfigurationConflictAction::Redo {
                resolution_operation_id,
            } => {
                let receipt = next
                    .conflict_operations
                    .iter_mut()
                    .find(|item| {
                        item.request.operation_id == *resolution_operation_id
                            && matches!(
                                item.request.action,
                                ConfigurationConflictAction::Resolve { .. }
                            )
                    })
                    .ok_or_else(conflict_changed)?;
                let undo = matches!(request.action, ConfigurationConflictAction::Undo { .. });
                if receipt.undone == undo {
                    return Err(conflict_changed());
                }
                receipt.undone = undo;
                (
                    receipt.origin,
                    receipt.conflict.clone(),
                    receipt.resolution.clone(),
                    undo,
                )
            }
        };
        let previous_draft = next.editor_session.clone();
        match origin {
            ConfigurationConflictOrigin::Candidate => {
                if undo {
                    let upstream = next.upstream_document()?;
                    let mut restored = conflict.clone();
                    restored.upstream_value = semantic_value_at(&upstream, &conflict.path)
                        .map_err(|_| conflict_changed())?;
                    let mut document = parse_semantic_document(
                        next.format,
                        next.final_edit.edited_content.as_bytes(),
                    )?;
                    if semantic_value_at(&document, &conflict.path)
                        .map_err(|_| conflict_changed())?
                        != chosen_value(&conflict, &resolution)
                    {
                        return Err(conflict_changed());
                    }
                    apply_semantic_value(&mut document, &conflict.path, &restored.upstream_value)
                        .map_err(|_| conflict_changed())?;
                    next.final_edit.edited_content =
                        serialize_semantic_document(next.format, &document)?;
                    next.final_edit
                        .conflicts
                        .retain(|item| item.path != conflict.path);
                    next.final_edit.conflicts.push(restored);
                    next.rebuild_desired(now)?;
                } else {
                    let current = next
                        .final_edit
                        .conflicts
                        .iter()
                        .find(|item| item.conflict_id == conflict.conflict_id)
                        .ok_or_else(conflict_changed)?;
                    if current.upstream_value != conflict.upstream_value {
                        return Err(conflict_changed());
                    }
                    next.resolve_final_conflict(&conflict.conflict_id, resolution.clone(), now)?;
                }
                if let Some(draft) = &mut next.editor_session {
                    rebase_final_editor_session(
                        draft,
                        next.format,
                        &next.desired.content,
                        next.state_revision,
                        next.generation,
                    )?;
                }
            }
            ConfigurationConflictOrigin::Draft => {
                let draft = next.editor_session.as_mut().ok_or_else(conflict_changed)?;
                if draft.rebase_required {
                    return Err(conflict_changed());
                }
                if undo {
                    let mut restored = conflict.clone();
                    let upstream =
                        parse_semantic_document(next.format, draft.base_content.as_bytes())?;
                    restored.upstream_value = semantic_value_at(&upstream, &conflict.path)
                        .map_err(|_| conflict_changed())?;
                    let mut document =
                        parse_semantic_document(next.format, draft.working_content.as_bytes())?;
                    if semantic_value_at(&document, &conflict.path)
                        .map_err(|_| conflict_changed())?
                        != chosen_value(&conflict, &resolution)
                    {
                        return Err(conflict_changed());
                    }
                    apply_semantic_value(&mut document, &conflict.path, &restored.upstream_value)
                        .map_err(|_| conflict_changed())?;
                    draft.working_content = serialize_semantic_document(next.format, &document)?;
                    draft.resolutions.remove(&conflict.conflict_id);
                    draft.conflicts.retain(|item| item.path != conflict.path);
                    draft.conflicts.push(restored);
                    refresh_final_editor_conflicts(draft, next.format);
                } else {
                    let current = draft
                        .conflicts
                        .iter()
                        .find(|item| item.conflict_id == conflict.conflict_id)
                        .ok_or_else(conflict_changed)?;
                    if current.upstream_value != conflict.upstream_value {
                        return Err(conflict_changed());
                    }
                    resolve_final_editor_conflict(
                        draft,
                        next.format,
                        &conflict.conflict_id,
                        resolution.clone(),
                    )?;
                }
            }
        }
        next.state_revision = self.state_revision.saturating_add(1);
        if let Some(draft) = &mut next.editor_session {
            if previous_draft.as_ref() != Some(draft) {
                draft.draft_revision = previous_draft
                    .as_ref()
                    .map_or(1, |item| item.draft_revision.saturating_add(1));
                draft.updated_unix_ms = now;
            }
            draft.based_on_state_revision = next.state_revision;
        }
        let referenced_resolution = match &request.action {
            ConfigurationConflictAction::Resolve { .. } => None,
            ConfigurationConflictAction::Undo {
                resolution_operation_id,
            }
            | ConfigurationConflictAction::Redo {
                resolution_operation_id,
            } => Some(resolution_operation_id.clone()),
        };
        next.conflict_operations.push(ConfigurationConflictReceipt {
            request,
            conflict,
            origin,
            resolution,
            undone: undo,
            generation: next.generation,
        });
        const RECEIPT_LIMIT: usize = 64;
        const RECEIPT_BYTES_LIMIT: usize = 4 * 1024 * 1024;
        while next.conflict_operations.len() > 2
            && (next.conflict_operations.len() > RECEIPT_LIMIT
                || serde_json::to_vec(&next.conflict_operations)
                    .expect("serializable receipts")
                    .len()
                    > RECEIPT_BYTES_LIMIT)
        {
            let oldest_removable = next
                .conflict_operations
                .iter()
                .position(|item| {
                    referenced_resolution.as_deref() != Some(&item.request.operation_id)
                        && item.request.operation_id
                            != next
                                .conflict_operations
                                .last()
                                .unwrap()
                                .request
                                .operation_id
                })
                .expect("more than two receipts leaves a removable entry");
            next.conflict_operations.remove(oldest_removable);
        }
        *self = next;
        Ok(true)
    }
}
