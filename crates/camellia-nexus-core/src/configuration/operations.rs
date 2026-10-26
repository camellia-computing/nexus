use super::*;

/// Identifies one user request independently of its delivery attempts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationMutationContext {
    pub operation_id: String,
    pub kind: ConfigurationOperationKind,
    pub expected_state_revision: u64,
    pub editor_session_id: Option<String>,
    pub expected_draft_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationOperationKind {
    Save,
    Apply,
    Intent,
}

impl ConfigurationMutationContext {
    pub fn ensure_kind(&self, expected: ConfigurationOperationKind) -> Result<()> {
        if self.kind != expected {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration operation does not match this action",
            )
            .with_message_key("CONFIGURATION_OPERATION_MISMATCH"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConfigurationOperationStatus {
    Pending,
    Saved,
    Applied,
    Rejected,
    Interrupted,
    Updated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationOperationResult {
    pub operation_id: String,
    pub status: ConfigurationOperationStatus,
    pub candidate_generation: u64,
    pub saved_candidate: Option<ConfigurationRevision>,
    pub message_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationOperationReceipt {
    pub request: ConfigurationMutationContext,
    pub result: ConfigurationOperationResult,
}

impl ConfigurationState {
    pub fn operation_receipt(
        &self,
        request: &ConfigurationMutationContext,
    ) -> Result<Option<&ConfigurationOperationReceipt>> {
        let receipt = self
            .operation_receipts
            .iter()
            .find(|receipt| receipt.request.operation_id == request.operation_id);
        if receipt.is_some_and(|receipt| receipt.request != *request) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Operation identity belongs to a different request",
            )
            .with_message_key("CONFIGURATION_OPERATION_MISMATCH"));
        }
        Ok(receipt)
    }

    pub fn begin_operation(&mut self, request: ConfigurationMutationContext) -> Result<()> {
        if uuid::Uuid::parse_str(&request.operation_id).is_err() {
            return Err(CamelliaNexusError::invalid_spec(
                "Invalid configuration operation identity",
            ));
        }
        if self.operation_receipt(&request)?.is_some() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Operation already recorded",
            )
            .with_message_key("CONFIGURATION_OPERATION_MISMATCH"));
        }
        if self
            .operation_receipts
            .iter()
            .any(|receipt| receipt.result.status == ConfigurationOperationStatus::Pending)
        {
            return Err(CamelliaNexusError::new(
                ErrorCode::Storage,
                "Configuration operation needs recovery",
            )
            .with_message_key("CONFIGURATION_RECOVERY_REQUIRED"));
        }
        if request.expected_state_revision != self.state_revision {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration state changed",
            )
            .with_message_key("CONFIGURATION_STATE_STALE"));
        }
        let draft_matches = match &self.editor_session {
            Some(draft) => {
                request.editor_session_id.as_deref() == Some(&draft.session_id)
                    && request.expected_draft_revision == Some(draft.draft_revision)
            }
            None => request
                .expected_draft_revision
                .is_none_or(|revision| revision == 0),
        };
        if !draft_matches {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration draft changed",
            )
            .with_message_key("CONFIGURATION_DRAFT_STALE"));
        }
        // An evicted identity cannot be reused: its original state revision is stale.
        const RECEIPT_LIMIT: usize = 64;
        if self.operation_receipts.len() >= RECEIPT_LIMIT {
            self.operation_receipts
                .drain(..self.operation_receipts.len() - RECEIPT_LIMIT + 1);
        }
        self.operation_receipts.push(ConfigurationOperationReceipt {
            result: ConfigurationOperationResult {
                operation_id: request.operation_id.clone(),
                status: ConfigurationOperationStatus::Pending,
                candidate_generation: self.generation,
                saved_candidate: None,
                message_key: None,
            },
            request,
        });
        self.state_revision = self.state_revision.saturating_add(1);
        Ok(())
    }

    pub fn record_operation_candidate(&mut self, operation_id: &str) -> Result<()> {
        if !self.candidate_is_saved() {
            return Err(CamelliaNexusError::invalid_spec(
                "Operation candidate is not saved",
            ));
        }
        let receipt = self
            .operation_receipts
            .iter_mut()
            .find(|receipt| {
                receipt.request.operation_id == operation_id
                    && receipt.result.status == ConfigurationOperationStatus::Pending
            })
            .ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Pending operation was not found",
                )
                .with_message_key("CONFIGURATION_OPERATION_MISMATCH")
            })?;
        let revision = Some(self.desired.revision.clone());
        if receipt.result.saved_candidate != revision {
            receipt.result.saved_candidate = revision;
            self.state_revision = self.state_revision.saturating_add(1);
        }
        Ok(())
    }

    pub fn finish_operation(
        &mut self,
        operation_id: &str,
        status: ConfigurationOperationStatus,
        candidate_generation: u64,
        message_key: Option<&str>,
    ) -> bool {
        let Some(receipt) = self
            .operation_receipts
            .iter_mut()
            .find(|receipt| receipt.request.operation_id == operation_id)
        else {
            return false;
        };
        if receipt.result.status != ConfigurationOperationStatus::Pending {
            return false;
        }
        receipt.result.status = status;
        receipt.result.candidate_generation = candidate_generation;
        receipt.result.message_key = message_key.map(str::to_owned);
        self.state_revision = self.state_revision.saturating_add(1);
        true
    }
}
