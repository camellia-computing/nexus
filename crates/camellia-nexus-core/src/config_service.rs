use std::{collections::HashMap, path::PathBuf, sync::Arc};

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::{
    ActionContext, ActionPlan, ActionResult, AdapterRegistry, CamelliaNexusError, CommandOutput,
    ConfigDocument, ConfigurationSchemaDocument, DynConfigStore, DynProgramStore, DynToolRunner,
    ErrorCode, ExecutableMetadata, JsonSchemaDialect, MAX_CONFIG_BYTES,
    MAX_CONFIGURATION_SCHEMA_BYTES, ProgramConfigTransaction, ProgramId, ProgramSpec, Result,
    StagedConfig, ValidationResult,
};

const JSON_SCHEMA_2020_12_URI: &str = "https://json-schema.org/draft/2020-12/schema";
const SCHEMA_ERROR_DETAILS_LIMIT: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigurationSchemaCacheKey {
    executable: PathBuf,
    metadata: ExecutableMetadata,
}

#[derive(Debug, Clone)]
struct ConfigurationSchemaCacheEntry {
    key: ConfigurationSchemaCacheKey,
    document: ConfigurationSchemaDocument,
}

#[derive(Debug)]
pub struct PreparedConfigGuard {
    pub staged: StagedConfig,
    pub new_hash: String,
    base_hash: String,
    _guard: OwnedMutexGuard<()>,
}

pub struct CommittedConfigGuard {
    new_hash: String,
    _guard: OwnedMutexGuard<()>,
}

pub struct CommittedProgramConfigGuard {
    transaction: ProgramConfigTransaction,
    new_hash: String,
    _guard: OwnedMutexGuard<()>,
}

impl PreparedConfigGuard {
    pub fn new_hash(&self) -> &str {
        &self.new_hash
    }
}

impl CommittedConfigGuard {
    pub fn new_hash(&self) -> &str {
        &self.new_hash
    }
}

#[derive(Clone)]
pub struct ConfigService {
    store: DynConfigStore,
    program_store: DynProgramStore,
    tool_runner: DynToolRunner,
    adapters: AdapterRegistry,
    locks: Arc<RwLock<HashMap<ProgramId, Arc<Mutex<()>>>>>,
    configuration_schema_locks: Arc<RwLock<HashMap<ProgramId, Arc<Mutex<()>>>>>,
    configuration_schema_cache: Arc<RwLock<HashMap<ProgramId, ConfigurationSchemaCacheEntry>>>,
}

impl ConfigService {
    pub fn new(
        store: DynConfigStore,
        program_store: DynProgramStore,
        tool_runner: DynToolRunner,
        adapters: AdapterRegistry,
    ) -> Self {
        Self {
            store,
            program_store,
            tool_runner,
            adapters,
            locks: Arc::new(RwLock::new(HashMap::new())),
            configuration_schema_locks: Arc::new(RwLock::new(HashMap::new())),
            configuration_schema_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    async fn lock(&self, id: &ProgramId) -> OwnedMutexGuard<()> {
        acquire_program_lock(&self.locks, id).await
    }

    async fn configuration_schema_lock(&self, id: &ProgramId) -> OwnedMutexGuard<()> {
        acquire_program_lock(&self.configuration_schema_locks, id).await
    }

    pub async fn forget_program(&self, id: &ProgramId) {
        self.locks.write().await.remove(id);
        self.configuration_schema_locks.write().await.remove(id);
        self.configuration_schema_cache.write().await.remove(id);
    }

    async fn configuration_schema_cache_key(
        &self,
        spec: &ProgramSpec,
    ) -> Result<ConfigurationSchemaCacheKey> {
        let workspace = self.program_store.workspace(&spec.id).await?;
        let mut metadata = self.program_store.executable_metadata(spec).await?;
        metadata.detected_version = spec
            .executable
            .metadata()
            .and_then(|metadata| metadata.detected_version.clone());
        Ok(ConfigurationSchemaCacheKey {
            executable: spec.executable_path(&workspace),
            metadata,
        })
    }

    async fn cached_configuration_schema(
        &self,
        id: &ProgramId,
        key: &ConfigurationSchemaCacheKey,
    ) -> Option<ConfigurationSchemaDocument> {
        self.configuration_schema_cache
            .read()
            .await
            .get(id)
            .filter(|entry| &entry.key == key)
            .map(|entry| entry.document.clone())
    }

    pub async fn load_configuration_schema(
        &self,
        spec: &ProgramSpec,
    ) -> Result<Option<ConfigurationSchemaDocument>> {
        let workspace = self.program_store.workspace(&spec.id).await?;
        let adapter = self.adapters.get(spec.program_type.kind());
        let Some(plan) = adapter.configuration_schema_plan(spec, &workspace) else {
            return Ok(None);
        };

        let initial_key = self.configuration_schema_cache_key(spec).await?;
        if let Some(document) = self
            .cached_configuration_schema(&spec.id, &initial_key)
            .await
        {
            return Ok(Some(document));
        }

        let _guard = self.configuration_schema_lock(&spec.id).await;
        let key = self.configuration_schema_cache_key(spec).await?;
        if let Some(document) = self.cached_configuration_schema(&spec.id, &key).await {
            return Ok(Some(document));
        }

        let output = self.tool_runner.run(plan.command).await?;
        if !output.success {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigurationSchemaInvalid,
                "Program could not generate a configuration schema",
            )
            .with_details(truncate_details(&combined_output(&output))));
        }
        let document = parse_configuration_schema(&output.stdout, plan.descriptor)?;
        if self.configuration_schema_cache_key(spec).await? != key {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Program executable changed while its configuration schema was generated",
            ));
        }
        self.configuration_schema_cache.write().await.insert(
            spec.id.clone(),
            ConfigurationSchemaCacheEntry {
                key,
                document: document.clone(),
            },
        );
        Ok(Some(document))
    }

    pub async fn probe_binary(&self, spec: &ProgramSpec) -> Result<Option<String>> {
        let workspace = self.program_store.workspace(&spec.id).await?;
        let executable = spec.executable_path(&workspace);
        self.probe_executable(spec, executable, workspace).await
    }

    pub async fn probe_executable(
        &self,
        spec: &ProgramSpec,
        executable: std::path::PathBuf,
        workspace: std::path::PathBuf,
    ) -> Result<Option<String>> {
        let adapter = self.adapters.get(spec.program_type.kind());
        let plans = adapter.probe_plans(&executable, &workspace);
        let mut outputs = Vec::with_capacity(plans.len());
        for plan in plans {
            outputs.push(self.tool_runner.run(plan).await?);
        }
        Ok(adapter.verify_probe(&outputs)?.version)
    }

    pub async fn load(&self, spec: &ProgramSpec) -> Result<ConfigDocument> {
        let _guard = self.lock(&spec.id).await;
        let adapter = self.adapters.get(spec.program_type.kind());
        let editor = adapter.editor(spec).ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Program has no managed config editor",
            )
        })?;
        let raw = self.store.load(spec).await?;
        Ok(ConfigDocument {
            content: raw.content,
            base_hash: raw.base_hash,
            language: editor.language,
            documentation_url: editor.documentation_url,
            configuration_schema: editor.configuration_schema,
        })
    }

    pub async fn validate(
        &self,
        spec: &ProgramSpec,
        content: String,
        base_hash: String,
    ) -> Result<ValidationResult> {
        let _guard = self.lock(&spec.id).await;
        self.ensure_content_size(&content)?;
        self.ensure_hash(spec, &base_hash).await?;
        let staged = self.store.stage(spec, content.as_bytes()).await?;
        let output = self.run_validation(spec, &staged).await;
        let discard_result = self.store.discard_staged(staged).await;
        let output = output?;
        discard_result?;
        Ok(ValidationResult {
            valid: output.success,
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    pub async fn prepare_apply(
        &self,
        spec: &ProgramSpec,
        content: String,
        base_hash: String,
    ) -> Result<PreparedConfigGuard> {
        let guard = self.lock(&spec.id).await;
        self.ensure_content_size(&content)?;
        self.ensure_hash(spec, &base_hash).await?;
        let staged = self.store.stage(spec, content.as_bytes()).await?;
        let validation = self.run_validation(spec, &staged).await;
        match validation {
            Ok(output) if output.success => {}
            Ok(output) => {
                self.store.discard_staged(staged.clone()).await?;
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Configuration validation failed",
                )
                .with_details(combined_output(&output)));
            }
            Err(error) => {
                let _ = self.store.discard_staged(staged.clone()).await;
                return Err(error);
            }
        }
        if let Err(error) = self.ensure_hash(spec, &base_hash).await {
            let _ = self.store.discard_staged(staged.clone()).await;
            return Err(error);
        }
        Ok(PreparedConfigGuard {
            new_hash: hash_bytes(content.as_bytes()),
            base_hash,
            staged,
            _guard: guard,
        })
    }

    pub async fn commit(&self, prepared: PreparedConfigGuard) -> Result<CommittedConfigGuard> {
        let PreparedConfigGuard {
            staged,
            new_hash,
            base_hash,
            _guard,
        } = prepared;
        let discard = staged.clone();
        if let Err(error) = self
            .store
            .atomic_replace_with_backup(staged, &base_hash)
            .await
        {
            let _ = self.store.discard_staged(discard).await;
            return Err(error);
        }
        Ok(CommittedConfigGuard { new_hash, _guard })
    }

    pub async fn commit_program_update(
        &self,
        expected_spec: &ProgramSpec,
        next_spec: &ProgramSpec,
        prepared: PreparedConfigGuard,
    ) -> Result<CommittedProgramConfigGuard> {
        let PreparedConfigGuard {
            staged,
            new_hash,
            base_hash,
            _guard,
        } = prepared;
        let discard = staged.clone();
        let transaction = match self
            .program_store
            .begin_program_config_update(expected_spec, next_spec, staged, &base_hash)
            .await
        {
            Ok(transaction) => transaction,
            Err(error) => {
                let _ = self.store.discard_staged(discard).await;
                return Err(error);
            }
        };
        Ok(CommittedProgramConfigGuard {
            transaction,
            new_hash,
            _guard,
        })
    }

    pub async fn discard(&self, prepared: PreparedConfigGuard) -> Result<()> {
        self.store.discard_staged(prepared.staged).await
    }

    pub async fn restore_backup(&self, spec: &ProgramSpec) -> Result<()> {
        self.store.restore_backup(spec).await
    }

    pub async fn finalize(
        &self,
        spec: &ProgramSpec,
        committed: CommittedConfigGuard,
    ) -> Result<String> {
        self.store.finalize_replace(spec).await?;
        Ok(committed.new_hash)
    }

    pub async fn finalize_program_update(
        &self,
        committed: &CommittedProgramConfigGuard,
    ) -> Result<String> {
        self.program_store
            .finalize_program_config_update(committed.transaction.clone())
            .await?;
        Ok(committed.new_hash.clone())
    }

    pub async fn rollback_program_update(
        &self,
        committed: CommittedProgramConfigGuard,
    ) -> Result<()> {
        self.program_store
            .rollback_program_config_update(committed.transaction)
            .await
    }

    pub async fn run_action(
        &self,
        spec: &ProgramSpec,
        action_id: String,
        content: String,
        base_hash: String,
    ) -> Result<ActionResult> {
        let _guard = self.lock(&spec.id).await;
        self.ensure_content_size(&content)?;
        self.ensure_hash(spec, &base_hash).await?;
        let staged = self.store.stage(spec, content.as_bytes()).await?;
        let workspace = self.program_store.workspace(&spec.id).await?;
        let adapter = self.adapters.get(spec.program_type.kind());
        let context = ActionContext {
            spec: spec.clone(),
            workspace,
            staged_config: staged.path.clone(),
        };
        let result: Result<ActionResult> = async {
            let plan = adapter.action_plan(&action_id, &context)?;
            match plan {
                ActionPlan::Run(command) => {
                    let output = self.tool_runner.run(command).await?;
                    if !output.success {
                        Err(action_failed(&output))
                    } else {
                        Ok(ActionResult {
                            stdout: output.stdout,
                            stderr: output.stderr,
                            preview_content: None,
                        })
                    }
                }
                ActionPlan::Format {
                    command,
                    validate_after,
                    ..
                } => {
                    let formatted = self.tool_runner.run(command).await?;
                    if !formatted.success {
                        Err(action_failed(&formatted))
                    } else {
                        let validation = self.tool_runner.run(validate_after).await?;
                        if !validation.success {
                            Err(action_failed(&validation))
                        } else {
                            let preview = self.store.read_staged(&staged).await?;
                            Ok(ActionResult {
                                stdout: formatted.stdout,
                                stderr: formatted.stderr,
                                preview_content: Some(preview),
                            })
                        }
                    }
                }
            }
        }
        .await;
        let discard_result = self.store.discard_staged(staged).await;
        match result {
            Ok(result) => {
                discard_result?;
                Ok(result)
            }
            Err(error) => {
                let _ = discard_result;
                Err(error)
            }
        }
    }

    async fn run_validation(
        &self,
        spec: &ProgramSpec,
        staged: &StagedConfig,
    ) -> Result<CommandOutput> {
        let workspace = self.program_store.workspace(&spec.id).await?;
        let adapter = self.adapters.get(spec.program_type.kind());
        let context = ActionContext {
            spec: spec.clone(),
            workspace,
            staged_config: staged.path.clone(),
        };
        let plan = adapter.validate_plan(&context).ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Program has no configuration validator",
            )
        })?;
        self.tool_runner.run(plan).await
    }

    async fn ensure_hash(&self, spec: &ProgramSpec, expected: &str) -> Result<()> {
        let actual = self.store.current_hash(spec).await?;
        if actual == expected {
            Ok(())
        } else {
            Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Configuration changed since it was loaded",
            ))
        }
    }

    fn ensure_content_size(&self, content: &str) -> Result<()> {
        if content.len() <= MAX_CONFIG_BYTES {
            Ok(())
        } else {
            Err(CamelliaNexusError::invalid_spec(
                "Configuration exceeds the 4 MiB limit",
            ))
        }
    }
}

async fn acquire_program_lock(
    locks: &RwLock<HashMap<ProgramId, Arc<Mutex<()>>>>,
    id: &ProgramId,
) -> OwnedMutexGuard<()> {
    if let Some(lock) = locks.read().await.get(id).cloned() {
        return lock.lock_owned().await;
    }
    let lock = {
        let mut locks = locks.write().await;
        locks
            .entry(id.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

fn parse_configuration_schema(
    content: &str,
    descriptor: crate::ConfigurationSchemaDescriptor,
) -> Result<ConfigurationSchemaDocument> {
    if content.len() > MAX_CONFIGURATION_SCHEMA_BYTES {
        return Err(CamelliaNexusError::new(
            ErrorCode::OutputLimitExceeded,
            "Configuration schema exceeds the 4 MiB limit",
        ));
    }
    let value: serde_json::Value = serde_json::from_str(content).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::ConfigurationSchemaInvalid,
            "Program generated an invalid configuration schema",
        )
        .with_details(error.to_string())
    })?;
    let Some(root) = value.as_object() else {
        return Err(CamelliaNexusError::new(
            ErrorCode::ConfigurationSchemaInvalid,
            "Program generated a configuration schema with an invalid root",
        ));
    };
    match descriptor.dialect {
        JsonSchemaDialect::Draft202012 => {
            if root.get("$schema").and_then(serde_json::Value::as_str)
                != Some(JSON_SCHEMA_2020_12_URI)
            {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigurationSchemaInvalid,
                    "Program generated an unsupported configuration schema dialect",
                ));
            }
        }
    }
    validate_local_schema_references(&value)?;
    Ok(ConfigurationSchemaDocument {
        source: descriptor.source,
        dialect: descriptor.dialect,
        content: content.to_owned(),
        content_hash: hash_bytes(content.as_bytes()),
    })
}

fn validate_local_schema_references(value: &serde_json::Value) -> Result<()> {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                validate_local_schema_references(value)?;
            }
        }
        serde_json::Value::Object(values) => {
            for keyword in ["$ref", "$dynamicRef"] {
                if let Some(reference) = values.get(keyword) {
                    let Some(reference) = reference.as_str() else {
                        return Err(CamelliaNexusError::new(
                            ErrorCode::ConfigurationSchemaInvalid,
                            "Configuration schema contains a non-string reference",
                        ));
                    };
                    if !reference.starts_with('#') {
                        return Err(CamelliaNexusError::new(
                            ErrorCode::ConfigurationSchemaInvalid,
                            "Configuration schema contains an external reference",
                        ));
                    }
                }
            }
            for value in values.values() {
                validate_local_schema_references(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn truncate_details(value: &str) -> String {
    if value.len() <= SCHEMA_ERROR_DETAILS_LIMIT {
        return value.to_owned();
    }
    let mut boundary = SCHEMA_ERROR_DETAILS_LIMIT;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}\n… output truncated", &value[..boundary])
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn combined_output(output: &CommandOutput) -> String {
    format!("{}\n{}", output.stdout, output.stderr)
        .trim()
        .to_owned()
}

fn action_failed(output: &CommandOutput) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Program action failed")
        .with_details(combined_output(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft_2020_12_program_schema() -> crate::ConfigurationSchemaDescriptor {
        crate::ConfigurationSchemaDescriptor {
            source: crate::ConfigurationSchemaSource::ProgramBinary,
            dialect: JsonSchemaDialect::Draft202012,
        }
    }

    #[test]
    fn accepts_bounded_draft_2020_12_schema_with_local_references() {
        let content = r##"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$ref": "#/$defs/root",
  "$defs": {
    "root": {
      "type": "object",
      "properties": {
        "enabled": { "type": "boolean" }
      },
      "additionalProperties": false
    }
  }
}"##;
        let document = parse_configuration_schema(content, draft_2020_12_program_schema())
            .expect("valid schema");
        assert_eq!(
            document.source,
            crate::ConfigurationSchemaSource::ProgramBinary
        );
        assert_eq!(document.dialect, JsonSchemaDialect::Draft202012);
        assert_eq!(document.content, content);
        assert_eq!(document.content_hash, hash_bytes(content.as_bytes()));
    }

    #[test]
    fn rejects_external_and_non_string_schema_references() {
        for content in [
            r#"{
              "$schema": "https://json-schema.org/draft/2020-12/schema",
              "$ref": "https://example.test/schema.json"
            }"#,
            r#"{
              "$schema": "https://json-schema.org/draft/2020-12/schema",
              "$ref": 7
            }"#,
            r#"{
              "$schema": "https://json-schema.org/draft/2020-12/schema",
              "$dynamicRef": "https://example.test/schema.json#node"
            }"#,
        ] {
            let error = parse_configuration_schema(content, draft_2020_12_program_schema())
                .expect_err("invalid reference");
            assert_eq!(error.code, ErrorCode::ConfigurationSchemaInvalid);
        }
    }

    #[test]
    fn rejects_other_schema_dialects_and_oversized_output() {
        let error = parse_configuration_schema(
            r#"{"$schema":"http://json-schema.org/draft-07/schema#"}"#,
            draft_2020_12_program_schema(),
        )
        .expect_err("unsupported dialect");
        assert_eq!(error.code, ErrorCode::ConfigurationSchemaInvalid);

        let oversized = " ".repeat(MAX_CONFIGURATION_SCHEMA_BYTES + 1);
        let error = parse_configuration_schema(&oversized, draft_2020_12_program_schema())
            .expect_err("oversized schema");
        assert_eq!(error.code, ErrorCode::OutputLimitExceeded);
    }

    #[test]
    fn truncates_schema_command_details_on_utf8_boundaries() {
        let value = "测".repeat(SCHEMA_ERROR_DETAILS_LIMIT);
        let truncated = truncate_details(&value);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.ends_with("… output truncated"));
        assert!(truncated.len() <= SCHEMA_ERROR_DETAILS_LIMIT + 32);
    }
}
