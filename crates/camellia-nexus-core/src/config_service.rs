use std::{collections::HashMap, path::PathBuf, sync::Arc};

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::{
    ActionContext, ActionPlan, ActionResult, AdapterRegistry, CamelliaNexusError, CommandOutput,
    ConfigDocument, ConfigurationSchemaDocument, CoreBinaryFingerprint, DetectedBinary,
    DynConfigStore, DynProgramStore, DynToolRunner, ErrorCode, ExecutableMetadata,
    JsonSchemaDialect, MAX_CONFIG_BYTES, MAX_CONFIGURATION_SCHEMA_BYTES, NativeDiagnosticReport,
    ProgramId, ProgramSpec, Result, StagedConfig, ValidationResult,
};

const JSON_SCHEMA_2020_12_URI: &str = "https://json-schema.org/draft/2020-12/schema";

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
    spec: ProgramSpec,
    _guard: OwnedMutexGuard<()>,
}

pub struct CommittedConfigGuard {
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
        if let Some(recorded) = spec
            .executable
            .metadata()
            .filter(|recorded| recorded.fingerprint.sha256 == metadata.fingerprint.sha256)
        {
            metadata.probe.clone_from(&recorded.probe);
            metadata.core_target.clone_from(&recorded.core_target);
        }
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
            .with_details(serde_json::to_string(
                &NativeDiagnosticReport::from_output(&output),
            )?));
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

    pub async fn probe_binary(&self, spec: &ProgramSpec) -> Result<ExecutableMetadata> {
        let workspace = self.program_store.workspace(&spec.id).await?;
        let executable = spec.executable_path(&workspace);
        let mut metadata = self.program_store.executable_metadata(spec).await?;
        let detected = self
            .probe_executable(spec, executable, workspace, &metadata.fingerprint)
            .await?;
        metadata.probe = detected.probe;
        metadata.core_target = detected.core_target;
        Ok(metadata)
    }

    pub async fn probe_executable(
        &self,
        spec: &ProgramSpec,
        executable: std::path::PathBuf,
        workspace: std::path::PathBuf,
        fingerprint: &CoreBinaryFingerprint,
    ) -> Result<DetectedBinary> {
        let adapter = self.adapters.get(spec.program_type.kind());
        let plans = adapter.probe_plans(&executable, &workspace);
        let mut outputs = Vec::with_capacity(plans.len());
        for plan in plans {
            outputs.push(self.tool_runner.run(plan).await?);
        }
        let mut detected = adapter.verify_probe(&outputs)?;
        if let Some(probe) = &detected.probe {
            crate::assess_core_probe(spec.program_type.kind(), probe)?.require_admitted()?;
        }
        detected.core_target = match &detected.probe {
            Some(probe) => Some(crate::CoreTargetIdentity::from_probe(
                spec.program_type.kind(),
                probe,
                Some(fingerprint.sha256.clone()),
            )?),
            None => None,
        };
        Ok(detected)
    }

    pub async fn activation_preflight(
        &self,
        spec: &ProgramSpec,
        validated_config_hash: Option<&str>,
    ) -> Result<()> {
        if spec.program_type.main_config().is_none() {
            return Ok(());
        }
        crate::require_program_admission(spec)?;
        let recorded = spec.executable.metadata().ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Core activation requires an exact binary fingerprint",
            )
        })?;
        let current = self.program_store.executable_metadata(spec).await?;
        if current.fingerprint.sha256 != recorded.fingerprint.sha256 {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Core executable changed after configuration validation",
            )
            .with_message_key("CORE_TARGET_CHANGED"));
        }
        let target = recorded.core_target.as_ref().ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Core activation requires a compatibility target",
            )
        })?;
        if target.fingerprint_sha256.as_deref() != Some(recorded.fingerprint.sha256.as_str()) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Core compatibility target does not match the executable fingerprint",
            )
            .with_message_key("CORE_PROFILE_MISMATCH"));
        }
        let profile = crate::CoreCompatibilityProfile::resolve(target)?;
        let config_hash = self.store.current_hash(spec).await?;
        // A configuration commit that is still inside the controller's
        // stabilization transaction already carries an exact native-validator
        // result in its committed guard.  Accept that one-shot hash here; the
        // durable state/evidence is updated by the desktop coordinator after
        // the runtime confirmation succeeds.  Ordinary starts (and retries
        // after rollback) must continue to use persisted candidate evidence.
        if let Some(validated_config_hash) = validated_config_hash {
            if config_hash != validated_config_hash {
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigConflict,
                    "Active configuration changed after native validation",
                )
                .with_message_key("CORE_VALIDATION_EVIDENCE_STALE"));
            }
            return Ok(());
        }
        let evidence = self
            .program_store
            .configuration_validation_evidence(spec)
            .await?
            .ok_or_else(|| {
                CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Active configuration has no native validation evidence",
                )
                .with_message_key("CORE_VALIDATION_EVIDENCE_STALE")
            })?;
        if !evidence.validates(
            &recorded.fingerprint.sha256,
            &profile.profile_hash,
            &config_hash,
        ) {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Active configuration validation evidence is stale",
            )
            .with_message_key("CORE_VALIDATION_EVIDENCE_STALE"));
        }
        Ok(())
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
        self.ensure_validation_binary(spec).await?;
        self.ensure_content_size(&content)?;
        self.ensure_hash(spec, &base_hash).await?;
        self.assess_content(spec, &content).await?;
        let staged = self.store.stage(spec, content.as_bytes()).await?;
        let output = self.run_validation(spec, &staged).await;
        let identity = self.ensure_validation_binary(spec).await;
        let content_check = self
            .ensure_staged_content(&staged, &hash_bytes(content.as_bytes()))
            .await;
        let discard_result = self.store.discard_staged(staged).await;
        let output = output?;
        discard_result?;
        identity?;
        content_check?;
        Ok(ValidationResult {
            valid: output.success,
            report: NativeDiagnosticReport::from_output(&output),
        })
    }

    pub async fn prepare_apply(
        &self,
        spec: &ProgramSpec,
        content: String,
        base_hash: String,
    ) -> Result<PreparedConfigGuard> {
        let guard = self.lock(&spec.id).await;
        self.ensure_validation_binary(spec).await?;
        self.ensure_content_size(&content)?;
        self.ensure_hash(spec, &base_hash).await?;
        self.assess_content(spec, &content).await?;
        let staged = self.store.stage(spec, content.as_bytes()).await?;
        let validation = self.run_validation(spec, &staged).await;
        match validation {
            Ok(output) if output.success => {}
            Ok(output) => {
                self.store.discard_staged(staged.clone()).await?;
                let report = NativeDiagnosticReport::from_output(&output);
                return Err(CamelliaNexusError::new(
                    ErrorCode::ConfigInvalid,
                    "Configuration validation failed",
                )
                .with_message_key(&report.message_key)
                .with_details(serde_json::to_string(&report)?));
            }
            Err(error) => {
                let _ = self.store.discard_staged(staged.clone()).await;
                return Err(error);
            }
        }
        let recheck = async {
            self.ensure_hash(spec, &base_hash).await?;
            self.ensure_validation_binary(spec).await?;
            self.ensure_staged_content(&staged, &hash_bytes(content.as_bytes()))
                .await
        }
        .await;
        if let Err(error) = recheck {
            let _ = self.store.discard_staged(staged.clone()).await;
            return Err(error);
        }
        Ok(PreparedConfigGuard {
            new_hash: hash_bytes(content.as_bytes()),
            base_hash,
            spec: spec.clone(),
            staged,
            _guard: guard,
        })
    }

    pub async fn commit(&self, prepared: PreparedConfigGuard) -> Result<CommittedConfigGuard> {
        if let Err(error) = self.verify_prepared(&prepared.spec, &prepared).await {
            let _ = self.discard(prepared).await;
            return Err(error);
        }
        let PreparedConfigGuard {
            staged,
            new_hash,
            base_hash,
            _guard,
            ..
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

    pub async fn discard(&self, prepared: PreparedConfigGuard) -> Result<()> {
        self.store.discard_staged(prepared.staged).await
    }

    pub(crate) async fn verify_prepared(
        &self,
        spec: &ProgramSpec,
        prepared: &PreparedConfigGuard,
    ) -> Result<()> {
        if spec != &prepared.spec {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Prepared configuration target changed",
            )
            .with_message_key("CORE_TARGET_CHANGED"));
        }
        self.ensure_validation_binary(spec).await?;
        self.ensure_staged_content(&prepared.staged, &prepared.new_hash)
            .await
    }

    async fn ensure_staged_content(
        &self,
        staged: &StagedConfig,
        expected_hash: &str,
    ) -> Result<()> {
        let content = self.store.read_staged(staged).await?;
        if hash_bytes(content.as_bytes()) != expected_hash {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Staged configuration changed after preparation",
            )
            .with_message_key("CORE_VALIDATION_EVIDENCE_STALE"));
        }
        Ok(())
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
                ActionPlan::Format {
                    command,
                    validate_after,
                    ..
                } => {
                    let formatted = self.tool_runner.run(command).await?;
                    if !formatted.success {
                        action_failed(&formatted)
                    } else {
                        let validation = self.tool_runner.run(validate_after).await?;
                        if !validation.success {
                            action_failed(&validation)
                        } else {
                            let preview = self.store.read_staged(&staged).await?;
                            Ok(ActionResult {
                                report: NativeDiagnosticReport::from_output(&formatted),
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

    pub async fn assess_configuration(
        &self,
        spec: &ProgramSpec,
        content: &str,
    ) -> Result<Option<crate::ConfigurationAssessment>> {
        self.ensure_content_size(content)?;
        self.ensure_validation_binary(spec).await?;
        let Some(mut assessment) = crate::assess_program_configuration(spec, content)? else {
            return Ok(None);
        };
        if !assessment.issues.is_empty() {
            return Ok(Some(assessment));
        }
        let metadata = spec.executable.metadata().ok_or_else(|| {
            CamelliaNexusError::invalid_spec("Configuration assessment requires binary identity")
        })?;
        let probe = metadata.probe.as_ref().ok_or_else(|| {
            CamelliaNexusError::invalid_spec("Configuration assessment requires a binary probe")
        })?;
        let profile = crate::CoreCapabilityProfile::resolve(
            spec.program_type.kind(),
            probe,
            &metadata.fingerprint,
        )?;
        let format =
            crate::ConfigurationFormat::for_kind(spec.program_type.kind()).ok_or_else(|| {
                CamelliaNexusError::invalid_spec("Configuration format is unavailable")
            })?;
        let document = crate::parse_semantic_document(format, content.as_bytes())?;
        let entries = crate::configuration_field_evidence::undeclared_entries(&profile, &document)?;
        if !entries.is_empty() {
            let schema = self
                .load_configuration_schema(spec)
                .await
                .map_err(|error| {
                    CamelliaNexusError::new(
                        ErrorCode::ConfigurationSchemaInvalid,
                        "Configuration field evidence could not be obtained",
                    )
                    .with_message_key("CORE_CONFIGURATION_SCHEMA_UNCONFIRMED")
                    .with_details(format!("code={:?}", error.code))
                })?;
            assessment
                .issues
                .extend(crate::configuration_field_evidence::assess_entry_evidence(
                    entries,
                    &document,
                    schema.as_ref(),
                )?);
            self.ensure_validation_binary(spec).await?;
        }
        Ok(Some(assessment))
    }

    async fn assess_content(&self, spec: &ProgramSpec, content: &str) -> Result<()> {
        let Some(assessment) = self.assess_configuration(spec, content).await? else {
            return Ok(());
        };
        if let Some(issue) = assessment.issues.first() {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigInvalid,
                "A configuration value is not supported",
            )
            .with_message_key(&issue.message_key)
            .with_details(serde_json::to_string(&assessment.issues)?));
        }
        Ok(())
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

    async fn ensure_validation_binary(&self, spec: &ProgramSpec) -> Result<()> {
        crate::require_program_admission(spec)?;
        let recorded = spec.executable.metadata().ok_or_else(|| {
            CamelliaNexusError::new(
                ErrorCode::InvalidState,
                "Core validation requires an exact binary fingerprint",
            )
        })?;
        let current = self.program_store.executable_metadata(spec).await?;
        if current.fingerprint.sha256 != recorded.fingerprint.sha256 {
            return Err(CamelliaNexusError::new(
                ErrorCode::ConfigConflict,
                "Core executable identity no longer matches validation",
            )
            .with_message_key("CORE_TARGET_CHANGED"));
        }
        Ok(())
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
        .with_details(format!("line={}; column={}", error.line(), error.column()))
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
    fn invalid() -> CamelliaNexusError {
        CamelliaNexusError::new(
            ErrorCode::ConfigurationSchemaInvalid,
            "Configuration schema exceeds the local resource contract",
        )
        .with_message_key("CORE_CONFIGURATION_SCHEMA_UNCONFIRMED")
    }
    fn visit(value: &serde_json::Value, depth: usize, visits: &mut usize) -> Result<()> {
        *visits += 1;
        if depth > 64 || *visits > 65_536 {
            return Err(invalid());
        }
        if value.is_boolean() {
            return Ok(());
        }
        let object = value.as_object().ok_or_else(invalid)?;
        if depth != 0 && object.contains_key("$id") {
            return Err(invalid());
        }
        if object
            .get("$schema")
            .is_some_and(|dialect| dialect.as_str() != Some(JSON_SCHEMA_2020_12_URI))
        {
            return Err(invalid());
        }
        for keyword in ["$ref", "$dynamicRef"] {
            if let Some(reference) = object.get(keyword)
                && !reference
                    .as_str()
                    .is_some_and(|reference| reference.starts_with('#'))
            {
                return Err(invalid());
            }
        }
        // Visit schema locations, not examples/defaults/enum values that can contain literal keys.
        for keyword in [
            "$defs",
            "definitions",
            "properties",
            "patternProperties",
            "dependentSchemas",
        ] {
            if let Some(entries) = object.get(keyword) {
                for value in entries.as_object().ok_or_else(invalid)?.values() {
                    visit(value, depth + 1, visits)?;
                }
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            if let Some(entries) = object.get(keyword) {
                for value in entries.as_array().ok_or_else(invalid)? {
                    visit(value, depth + 1, visits)?;
                }
            }
        }
        for keyword in [
            "not",
            "if",
            "then",
            "else",
            "items",
            "contains",
            "additionalProperties",
            "unevaluatedProperties",
            "unevaluatedItems",
            "propertyNames",
            "contentSchema",
        ] {
            if let Some(value) = object.get(keyword) {
                visit(value, depth + 1, visits)?;
            }
        }
        Ok(())
    }
    visit(value, 0, &mut 0)
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn action_failed(output: &CommandOutput) -> Result<ActionResult> {
    let report = NativeDiagnosticReport::from_output(output);
    Err(
        CamelliaNexusError::new(ErrorCode::ConfigInvalid, "Program action failed")
            .with_message_key(&report.message_key)
            .with_details(serde_json::to_string(&report)?),
    )
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
    fn schema_data_is_not_a_reference_and_nested_resources_cannot_rebase_it() {
        let value = serde_json::json!({
            "$schema": JSON_SCHEMA_2020_12_URI,
            "$id": "https://example.test/program-schema",
            "default": {"$ref": "literal-user-data", "$id": "literal-value"},
            "examples": [{"$ref": 123}],
            "properties": {"$ref": {"type": "string"}},
        });
        parse_configuration_schema(&value.to_string(), draft_2020_12_program_schema()).unwrap();
        for schema in [
            serde_json::json!({"properties":{"field":{"$id":"https://example.test/other", "$ref":"#"}}}),
            serde_json::json!({"$defs":{"field":{"$schema":"https://example.test/dialect"}}}),
            serde_json::json!({"properties":[]}),
            serde_json::json!({"items":"not-a-schema"}),
        ] {
            assert!(validate_local_schema_references(&schema).is_err());
        }
        let mut deep = serde_json::json!({});
        for _ in 0..66 {
            deep = serde_json::json!({"items": deep});
        }
        assert!(validate_local_schema_references(&deep).is_err());
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
    fn invalid_schema_reports_position_without_echoing_generated_content() {
        let error = parse_configuration_schema(
            "{\"private-token\": invalid}",
            draft_2020_12_program_schema(),
        )
        .unwrap_err();
        let details = error.details.unwrap();
        assert!(details.starts_with("line=1; column="));
        assert!(!details.contains("private-token"));
    }

    #[test]
    fn failed_action_reports_category_without_echoing_native_values() {
        let output = CommandOutput {
            code: Some(1),
            success: false,
            stdout: "fixture-private-key".into(),
            stderr: "invalid port: fixture-private-token".into(),
        };
        let error = action_failed(&output).unwrap_err();
        assert_eq!(
            error.message_key.as_deref(),
            Some("CORE_NATIVE_PORT_REJECTED")
        );
        let details: NativeDiagnosticReport =
            serde_json::from_str(error.details.as_deref().unwrap()).unwrap();
        assert_eq!(details, NativeDiagnosticReport::from_output(&output));
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("fixture-private")
        );
    }
}
