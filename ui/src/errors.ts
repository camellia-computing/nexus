export interface ErrorInfo {
  code?: string;
  messageKey?: string;
  title: string;
  message: string;
  fallbackMessage: string;
  details: string;
  suggestion: string;
}

export type ConfigurationErrorContext =
  | 'details-save'
  | 'configuration-load'
  | 'sources-save'
  | 'sources-refresh'
  | 'guided-change'
  | 'final-editor-draft'
  | 'configuration-save'
  | 'configuration-apply'
  | 'configuration-rebase';

export const TRANSIENT_ERROR_DISMISS_MS = 12_000;

export function isTransientErrorInfo(error: ErrorInfo | null | undefined) {
  return error?.code === 'TIMEOUT' || error?.code === 'NETWORK' || error?.code === 'RATE_LIMITED';
}

export function publicErrorInfo(error: ErrorInfo | null | undefined): ErrorInfo | null {
  return error ? { ...error, details: '' } : null;
}

export function sameUserFacingError(
  left: ErrorInfo | null | undefined,
  right: ErrorInfo | null | undefined,
) {
  return !!left && !!right
    && left.code === right.code
    && left.title === right.title
    && left.message === right.message
    && left.suggestion === right.suggestion;
}

const presentations: Record<string, { title: string; message: string }> = {
  INVALID_SPEC: { title: 'Invalid input', message: 'The request contains invalid values.' },
  INVALID_PATH: { title: 'Invalid path', message: 'The selected path cannot be used.' },
  NOT_FOUND: { title: 'Item not found', message: 'The requested item is no longer available.' },
  ALREADY_EXISTS: { title: 'Program already exists', message: 'A program with this ID already exists.' },
  PROGRAM_BUSY: { title: 'Program busy', message: 'Another operation is still in progress.' },
  INVALID_STATE: { title: 'Action unavailable', message: 'The program is not ready for this action.' },
  SPAWN_FAILED: { title: 'Program could not start', message: 'The program process could not be started.' },
  STOP_FAILED: { title: 'Program could not stop', message: 'The program process could not be stopped.' },
  CONFIG_INVALID: { title: 'Configuration error', message: 'The configuration could not be applied.' },
  CONFIG_CONFLICT: { title: 'Configuration error', message: 'The configuration could not be applied.' },
  CONFIGURATION_SCHEMA_INVALID: { title: 'Suggestions unavailable', message: 'You can still edit the configuration. Try loading suggestions again.' },
  UNSUPPORTED_BINARY: { title: 'Unsupported program', message: 'The selected executable does not match this program type.' },
  OUTPUT_LIMIT_EXCEEDED: { title: 'Output limit reached', message: 'The operation produced too much output.' },
  TIMEOUT: { title: 'Operation timed out', message: 'The operation did not finish in time.' },
  RATE_LIMITED: { title: 'Too many requests', message: 'The service is temporarily limiting requests.' },
  NETWORK: { title: 'Network error', message: 'The network request could not be completed.' },
  STORAGE: { title: 'Storage error', message: 'Application data could not be accessed.' },
  SYSTEM_INTEGRATION: { title: 'System integration error', message: 'The operating-system integration request failed.' },
  PRIVILEGE_REQUIRED: { title: 'Administrator access required', message: 'This program needs administrator authorization before it can start.' },
  PRIVILEGE_AUTHORIZATION_CANCELED: { title: 'Administrator authorization canceled', message: 'The program was not started because authorization was canceled.' },
  PRIVILEGE_BROKER_UNAVAILABLE: { title: 'Privilege broker unavailable', message: 'The installed privilege broker is unavailable, so Camellia Nexus cannot start programs that require administrator access.' },
  PRIVILEGE_CONFIG_UNSAFE: { title: 'Privilege assessment failed', message: 'The program configuration could not be assessed safely.' },
  PRIVILEGE_BROKER_FAILED: { title: 'Privilege broker request failed', message: 'The privilege broker could not complete the administrator operation.' },
  PRIVILEGE_BROKER_CONNECTION_LOST: { title: 'Privilege broker connection lost', message: 'Camellia Nexus lost its secure lifecycle connection to the privilege broker.' },
  LICENSE_REQUIRED: { title: 'License required', message: 'An active license is required for this action.' },
  LICENSE_IDENTITY_ALREADY_REGISTERED: { title: 'License identity already registered', message: 'This installation is still linked to its existing license identity.' },
  LICENSE_ACTIVATION_PENDING: { title: 'Completing activation', message: 'This device is still completing its secure activation.' },
  LICENSE_ACTIVATION_PENDING_EXPIRED: { title: 'Activation session expired', message: 'This device did not finish activation before the secure session expired.' },
  LICENSE_PLAN_REQUIRED: { title: 'License plan needed', message: 'The current license does not include this feature.' },
  LICENSE_PERMISSION_DENIED: { title: 'Permission required', message: 'Your workspace role does not allow this action.' },
  LICENSE_TEAM_INVITATION_INVALID: { title: 'Invitation token not accepted', message: 'This Team invitation token is invalid, expired, or no longer available.' },
  LICENSE_TEAM_DEVICE_ENROLLMENT_INVALID: { title: 'Device enrollment token not accepted', message: 'This Team device enrollment token is invalid, expired, or no longer available.' },
  LICENSE_WORKSPACE_CONFLICT: { title: 'Workspace changed', message: 'The team workspace was updated by another session.' },
  LICENSE_OPERATION_CONFLICT: { title: 'Request changed', message: 'This operation ID was already used for a different request.' },
  LICENSE_WORKSPACE_QUOTA_EXCEEDED: { title: 'Workspace storage full', message: 'This revision would exceed the Team workspace storage quota.' },
  LICENSE_WORKSPACE_DOCUMENT_LIMIT_REACHED: { title: 'Shared configuration limit reached', message: 'The Team workspace has reached its active shared-configuration limit.' },
  LICENSE_WORKSPACE_ALERT_RULE_LIMIT_REACHED: { title: 'Alert rule limit reached', message: 'The Team workspace has reached its 50-rule limit.' },
  LICENSE_WORKSPACE_RETENTION_ACTIVE: { title: 'Recovery period still active', message: 'This shared configuration cannot be permanently removed during its 30-day recovery period.' },
  LICENSE_WORKSPACE_NOT_FOUND: { title: 'Workspace item not found', message: 'The requested Team workspace item no longer exists or is unavailable to this role.' },
  LICENSE_WORKSPACE_INTEGRITY_FAILED: { title: 'Workspace integrity check failed', message: 'Encrypted workspace content did not pass its integrity check.' },
  LICENSE_WORKSPACE_KEY_UNAVAILABLE: { title: 'Workspace key unavailable', message: 'The service cannot currently open encrypted workspace content.' },
  LICENSE_WEBHOOK_INVALID_URL: { title: 'Webhook URL rejected', message: 'The endpoint is not an allowed public HTTPS destination.' },
  LICENSE_WEBHOOK_ENDPOINT_LIMIT_REACHED: { title: 'Webhook endpoint limit reached', message: 'The Team workspace cannot create another webhook endpoint.' },
  LICENSE_WEBHOOK_NOT_FOUND: { title: 'Webhook endpoint not found', message: 'The requested webhook endpoint no longer exists.' },
  LICENSE_WEBHOOK_KEY_UNAVAILABLE: { title: 'Webhook key unavailable', message: 'The service cannot securely create or rotate webhook secrets.' },
  REQUEST_TOO_LARGE: { title: 'Request too large', message: 'The submitted content exceeds the service request-size limit.' },
  LICENSE_EXPIRED: { title: 'License expired', message: 'The license is no longer valid for protected features.' },
  LICENSE_ACCOUNT_SUSPENDED: { title: 'License account suspended', message: 'This license account is temporarily suspended.' },
  LICENSE_ACCOUNT_DENYLISTED: { title: 'License account disabled', message: 'This license account is no longer permitted to use the service.' },
  LICENSE_PAYMENT_PAST_DUE: { title: 'License payment past due', message: 'This license is unavailable because its payment is past due.' },
  LICENSE_CANCELED: { title: 'License canceled', message: 'This license has been canceled.' },
  LICENSE_DEVICE_DENIED: { title: 'Device authorization revoked', message: 'This device is not authorized for the current license.' },
  LICENSE_DEVICE_REMOVAL_INCOMPLETE: { title: 'Device could not be removed', message: 'The license service did not confirm removal. Local access was kept unchanged.' },
  LICENSE_REMOTE_SIGNOUT_INCOMPLETE: { title: 'Signed out locally', message: 'Local access was removed, but the license service could not revoke the remote device sessions.' },
  LICENSE_REVALIDATION_REQUIRED: { title: 'License revalidation required', message: 'The license must be revalidated online before protected features can continue.' },
  LICENSE_CLIENT_UPGRADE_REQUIRED: { title: 'Camellia Nexus update required', message: 'This client version no longer meets the signed minimum version policy, so protected features are unavailable.' },
  LICENSE_LIMIT_EXCEEDED: { title: 'License limit reached', message: 'The current license limit would be exceeded.' },
  LICENSE_ACTIVATION_CODE_INVALID: { title: 'Invalid activation code', message: 'The activation code was not recognized.' },
  LICENSE_ACTIVATION_CODE_EXPIRED: { title: 'Activation code expired', message: 'This activation code has expired.' },
  LICENSE_ACTIVATION_CODE_CONSUMED: { title: 'Activation code already used', message: 'This activation code has already been used.' },
  LICENSE_ACTIVATION_CODE_REVOKED: { title: 'Activation code revoked', message: 'This activation code is no longer valid.' },
  INTERNAL: { title: 'Unexpected error', message: 'The operation could not be completed.' },
};

const suggestions: Record<string, string> = {
  INVALID_SPEC: 'Review the highlighted values and try again.',
  INVALID_PATH: 'Use a path valid for the current operating system.',
  NOT_FOUND: 'Confirm that the file or program still exists.',
  ALREADY_EXISTS: 'Choose a different program ID.',
  PROGRAM_BUSY: 'Wait for the current operation to finish.',
  INVALID_STATE: 'Stop the program before changing runtime settings.',
  UNSUPPORTED_BINARY: 'Check that the selected binary matches the chosen program type.',
  CONFIG_INVALID: 'Correct the configuration reported by the validator.',
  CONFIG_CONFLICT: 'Reload the configuration before applying your changes.',
  CONFIGURATION_SCHEMA_INVALID: 'Try again, or keep editing without suggestions.',
  RATE_LIMITED: 'Wait briefly before trying again.',
  NETWORK: 'Check the network connection, proxy settings and source URL.',
  STORAGE: 'Check file permissions and available disk space.',
  SYSTEM_INTEGRATION: 'Check operating-system permissions and try again.',
  PRIVILEGE_REQUIRED: 'Start the program manually and approve the operating-system authorization request.',
  PRIVILEGE_AUTHORIZATION_CANCELED: 'Start the program again when you are ready to authorize it.',
  PRIVILEGE_BROKER_UNAVAILABLE: 'Reinstall a complete, trusted Camellia Nexus package or use standard access when the program supports it.',
  PRIVILEGE_CONFIG_UNSAFE: 'Validate the configuration and keep managed files inside the program workspace.',
  PRIVILEGE_BROKER_FAILED: 'Retry once; if it continues, reinstall the privilege broker from a trusted package.',
  PRIVILEGE_BROKER_CONNECTION_LOST: 'Stop any remaining program process before retrying.',
  LICENSE_REQUIRED: 'Open License settings to refresh or activate your license.',
  LICENSE_IDENTITY_ALREADY_REGISTERED: 'Reconnect the existing device, or choose Use another license before entering a code for a different license.',
  LICENSE_ACTIVATION_PENDING: 'Keep the app online; activation will resume automatically.',
  LICENSE_ACTIVATION_PENDING_EXPIRED: 'Start device activation again and use a new activation code.',
  LICENSE_PLAN_REQUIRED: 'Use a license plan that includes this feature.',
  LICENSE_PERMISSION_DENIED: 'Ask a workspace owner or administrator to grant the required role.',
  LICENSE_TEAM_INVITATION_INVALID: 'Paste a current member invitation token. Device enrollment tokens belong in Link device.',
  LICENSE_TEAM_DEVICE_ENROLLMENT_INVALID: 'Create a new device enrollment token on an already linked device, then paste it in Link device.',
  LICENSE_WORKSPACE_CONFLICT: 'Reload the team workspace, review the latest values, and retry.',
  LICENSE_OPERATION_CONFLICT: 'Refresh the current feature data, review the existing result, then retry the intended request with a new operation ID.',
  LICENSE_WORKSPACE_QUOTA_EXCEEDED: 'Export and delete unused configurations, then have an owner purge eligible deleted data, or increase the workspace limit.',
  LICENSE_WORKSPACE_DOCUMENT_LIMIT_REACHED: 'Delete an unused active shared configuration, then retry.',
  LICENSE_WORKSPACE_ALERT_RULE_LIMIT_REACHED: 'Delete an unused alert rule before creating another one.',
  LICENSE_WORKSPACE_RETENTION_ACTIVE: 'Restore the configuration if needed, or wait until 30 days after deletion before permanently removing it.',
  LICENSE_WORKSPACE_NOT_FOUND: 'Reload the Team workspace and review the current list before continuing.',
  LICENSE_WORKSPACE_INTEGRITY_FAILED: 'Stop editing this item and contact the workspace administrator. Do not overwrite or recreate it from this response.',
  LICENSE_WORKSPACE_KEY_UNAVAILABLE: 'Stop editing encrypted content and ask the service administrator to restore the workspace keyring.',
  LICENSE_WEBHOOK_INVALID_URL: 'Use a public HTTPS URL that does not redirect to a local, private, or reserved network address.',
  LICENSE_WEBHOOK_ENDPOINT_LIMIT_REACHED: 'Delete an unused endpoint or increase the signed workspace endpoint limit.',
  LICENSE_WEBHOOK_NOT_FOUND: 'Reload webhook endpoints before continuing.',
  LICENSE_WEBHOOK_KEY_UNAVAILABLE: 'Ask the service administrator to restore the webhook keyring before creating or rotating secrets.',
  REQUEST_TOO_LARGE: 'Reduce the configuration or request payload size, then submit it again as a new action.',
  LICENSE_EXPIRED: 'Refresh or reactivate the license.',
  LICENSE_ACCOUNT_SUSPENDED: 'Contact the license administrator or support before retrying.',
  LICENSE_ACCOUNT_DENYLISTED: 'Contact support if you believe this is an error.',
  LICENSE_PAYMENT_PAST_DUE: 'Update billing details, then refresh the license.',
  LICENSE_CANCELED: 'Renew the license or activate this device with another valid license.',
  LICENSE_DEVICE_DENIED: 'Open License settings and activate this device again if it should still have access.',
  LICENSE_DEVICE_REMOVAL_INCOMPLETE: 'When online, remove this device from another authorized installation or ask the license administrator to remove it.',
  LICENSE_REMOTE_SIGNOUT_INCOMPLETE: 'Reconnect briefly and sign out again when online, or remove this device from another authorized installation.',
  LICENSE_REVALIDATION_REQUIRED: 'Reconnect and refresh the license.',
  LICENSE_CLIENT_UPGRADE_REQUIRED: 'Install a supported Camellia Nexus version, reopen the app, and refresh the license.',
  LICENSE_LIMIT_EXCEEDED: 'Review the license limits or remove unused devices/programs.',
  LICENSE_ACTIVATION_CODE_INVALID: 'Check the activation code and try again.',
  LICENSE_ACTIVATION_CODE_EXPIRED: 'Use a new activation code or contact your license administrator.',
  LICENSE_ACTIVATION_CODE_CONSUMED: 'Use a new activation code or contact your license administrator.',
  LICENSE_ACTIVATION_CODE_REVOKED: 'Contact your license administrator for a replacement activation code.',
};

const defaultSuggestion = 'Retry the operation. If it continues, inspect the program logs.';

const admissionMessages: Record<string, string> = {
  CORE_VERSION_TOO_OLD: 'This version is outside the supported range. Choose a newer program.',
  CORE_VERSION_NOT_MAINTAINED: 'This program version is not supported yet.',
  CORE_PRERELEASE_NOT_SUPPORTED: 'Choose a supported stable release instead of a prerelease.',
  CORE_VERSION_UNRECOGNIZED: 'The program version could not be identified. Choose a recognizable build.',
  CORE_BINARY_IDENTITY_MISMATCH: 'The program identity is inconsistent. Choose another build.',
  CORE_KNOWLEDGE_INVALID: 'Program support information could not be loaded.',
  CORE_PROGRAM_CHECK_UNAVAILABLE: 'This program cannot perform the required checks. Choose another build.',
};

const assessmentMessages: Record<string, string> = {
  CORE_CHECK_COMPLETED: 'Check completed.',
  CORE_NATIVE_REJECTED: 'The current program rejected this candidate.',
  CORE_NATIVE_FIELD_REJECTED: 'The program reported an unsupported field. Review this configuration.',
  CORE_NATIVE_TYPE_REJECTED: 'The program reported a value with the wrong type.',
  CORE_NATIVE_SYNTAX_REJECTED: 'The program could not parse this configuration.',
  CORE_NATIVE_PORT_REJECTED: 'The program reported an invalid port.',
  CORE_NATIVE_RESOURCE_UNAVAILABLE: 'The program could not read a required file.',
  CORE_CONFIGURATION_FIELD_UNCONFIRMED: 'This program has not declared a configuration field. Remove it or choose a build that supports it.',
  CORE_CONFIGURATION_SCHEMA_UNCONFIRMED: 'This program could not provide usable field information. Retry or choose another build.',
  CONFIGURATION_VALUE_NOT_ALLOWED: 'A value is not supported by this program. Review Final configuration.',
  CONFIGURATION_ASSESSMENT_LIMIT: 'This configuration is too complex to check. Reduce it and try again.',
  CORE_BUILD_CAPABILITY_UNAVAILABLE: 'This build cannot use a configured feature. Change the setting or choose another build.',
  CORE_BUILD_CAPABILITY_UNCONFIRMED: 'A configured feature could not be confirmed. Choose a build with identifiable capabilities.',
};

const sourceMessages: Record<string, string> = {
  SOURCE_URL_INVALID: 'Use an HTTPS source address without embedded credentials.',
  SOURCE_FILE_NOT_FOUND: 'Source file not found. Select an existing file.',
  SOURCE_READ_FAILED: 'Could not read this source. Check file access and retry.',
  SOURCE_CHANGED: 'This source changed during reading. Retry the update.',
  SOURCE_TOO_LARGE: 'This source exceeds the size limit. Use a smaller configuration.',
  SOURCE_ACCESS_DENIED: 'The source denied access. Check its credentials.',
  SOURCE_TIMEOUT: 'The source did not respond in time. Retry the update.',
  SOURCE_DOWNLOAD_FAILED: 'Could not download this source. Check the connection and retry.',
  SOURCE_CREDENTIALS_UNAVAILABLE: 'Enter the source password again.',
  SOURCE_INVALID: 'This source contains invalid configuration. Edit or disable it.',
  SOURCE_REFRESH_FAILED: 'Could not update this source. Retry the update.',
  CORE_TARGET_SOURCE_REJECTED: 'This source has no items supported by the current program.',
};

export function sourceIssueMessage(messageKey: string): string {
  return sourceMessages[messageKey] ?? sourceMessages.SOURCE_REFRESH_FAILED;
}

export function coreAssessmentMessage(messageKey: string | undefined): string | undefined {
  return messageKey ? assessmentMessages[messageKey] : undefined;
}

export function coreAdmissionMessage(messageKey: string | undefined): string | undefined {
  return messageKey ? admissionMessages[messageKey] : undefined;
}

function isLicenseTrustConfigurationError(details: string) {
  return [
    'entitlement signature is invalid',
    'entitlement signing key is not trusted',
    'entitlement issuer is not trusted',
    'entitlement audience is not trusted',
    'entitlement device key does not match this installation',
    'entitlement belongs to a different device',
    'entitlement epoch is obsolete',
    'entitlement claim values are invalid',
    'entitlement is malformed',
  ].some((marker) => details.includes(marker));
}

export function errorInfoOf(error: unknown): ErrorInfo {
  const normalized = normalizeErrorRecord(error);
  if (normalized) {
    const value = normalized;
    const code = typeof value.code === 'string' ? value.code : '';
    const messageKey = typeof value.messageKey === 'string'
      ? value.messageKey
      : typeof value.message_key === 'string'
        ? value.message_key
        : undefined;
    const presentation = presentations[code] ?? {
      title: 'Operation failed',
      message: 'The operation could not be completed.',
    };
    const rawMessage = typeof value.message === 'string' ? value.message : '';
    const message = rawMessage === 'License service operation failed'
      ? presentation.message
      : rawMessage || presentation.message;
    const rawDetails = typeof value.details === 'string' ? value.details : '';
    const details = rawDetails.length > 16_000
      ? `${rawDetails.slice(0, 16_000)}\n… output truncated in the interface`
      : rawDetails;
    if (messageKey && sourceMessages[messageKey]) {
      return { code, messageKey, title: 'Configuration source needs attention',
        message: sourceIssueMessage(messageKey), fallbackMessage: sourceIssueMessage(messageKey), details, suggestion: '' };
    }
    if (messageKey && admissionMessages[messageKey]) {
      return {
        code,
        messageKey,
        title: 'Unsupported program',
        message: admissionMessages[messageKey],
        fallbackMessage: admissionMessages[messageKey],
        details,
        suggestion: '',
      };
    }
    if (messageKey === 'PROGRAM_PACKAGE_RECOVERY_REQUIRED') {
      return {
        code, messageKey,
        title: 'Program needs recovery',
        message: 'The program replacement could not be restored automatically.',
        fallbackMessage: '程序更换未能自动恢复。',
        details,
        suggestion: 'Reopen the app to recover before making more changes.',
      };
    }
    if (isLicenseTrustConfigurationError(details)) {
      return {
        code,
        messageKey,
        title: 'License configuration error',
        message: 'The license service is not trusted by this build.',
        fallbackMessage: 'The operation could not be completed.',
        details,
        suggestion: 'Check the license service signing configuration, issuer and key ID.',
      };
    }
    return {
      code,
      messageKey,
      title: presentation.title,
      message,
      fallbackMessage: presentation.message,
      details: code.startsWith('LICENSE_') ? '' : details,
      suggestion: suggestions[code] ?? defaultSuggestion,
    };
  }
  const message = String(error).replace(/^Error:\s*/, '') || 'Operation failed';
  return {
    title: 'Operation failed',
    message,
    fallbackMessage: 'The operation could not be completed.',
    details: message,
    suggestion: defaultSuggestion,
  };
}

/** Tauri normally rejects with the serialized Rust error object. Some native
 * WebView versions wrap that payload in an Error or JSON string, so normalize
 * those transport shapes before classifying the operation. */
function normalizeErrorRecord(error: unknown): Record<string, unknown> | null {
  if (error instanceof Error) {
    return parseErrorText(error.message) ?? {
      message: error.message || 'Operation failed',
      details: error.message || 'Operation failed',
    };
  }
  if (typeof error === 'string') {
    return parseErrorText(error) ?? {
      message: error.replace(/^Error:\s*/, '') || 'Operation failed',
      details: error.replace(/^Error:\s*/, '') || 'Operation failed',
    };
  }
  return error && typeof error === 'object'
    ? error as Record<string, unknown>
    : null;
}

function parseErrorText(value: string): Record<string, unknown> | null {
  const text = value.trim().replace(/^Error:\s*/, '');
  if (!text.startsWith('{') || !text.endsWith('}')) return null;
  try {
    const parsed = JSON.parse(text) as unknown;
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : null;
  } catch {
    return null;
  }
}

const configurationContextLabels: Record<ConfigurationErrorContext, { title: string }> = {
  'details-save': { title: 'Program details could not be saved' },
  'configuration-load': { title: 'Configuration workspace could not be loaded' },
  'sources-save': { title: 'Configuration sources could not be saved' },
  'sources-refresh': { title: 'Configuration sources could not be updated' },
  'guided-change': { title: 'Guided setting could not be applied' },
  'final-editor-draft': { title: 'Final configuration draft could not be saved' },
  'configuration-save': { title: 'Configuration candidate could not be saved' },
  'configuration-apply': { title: 'Configuration could not be applied' },
  'configuration-rebase': { title: 'Configuration draft could not be rebased' },
};

/**
 * Turn a backend configuration error into a context-aware, actionable notice.
 * The backend message/details remain available under Technical details, while
 * the stable message and suggestion are safe to translate in the UI.
 */
export function configurationErrorInfo(
  error: unknown,
  context: ConfigurationErrorContext,
): ErrorInfo {
  const base = errorInfoOf(error);
  if (base.messageKey && admissionMessages[base.messageKey]) return base;
  const contextTitle = configurationContextLabels[context].title;
  if (base.messageKey === 'CONFIGURATION_CONFLICT_STALE') return {
    ...base,
    title: 'Setting changed',
    message: 'This setting changed. Review the latest values before choosing again.',
    fallbackMessage: '此设置已更新。请查看最新值后重新选择。',
    suggestion: '',
  };
  if (base.messageKey === 'CONFIGURATION_OPERATION_PENDING') return {
    ...base,
    title: 'Choice still pending',
    message: 'Retry the earlier choice to confirm its result.',
    fallbackMessage: '请重试先前的选择以确认结果。',
    suggestion: '',
  };
  if (base.messageKey && sourceMessages[base.messageKey]) return { ...base, title: contextTitle };
  const assessmentMessage = coreAssessmentMessage(base.messageKey);
  if (assessmentMessage) {
    return {
      ...base,
      title: 'Configuration needs attention',
      message: assessmentMessage,
      details: base.details,
      suggestion: '',
    };
  }
  if (base.messageKey === 'CONFIGURATION_WORKSPACE_COMMIT_RECOVERY_REQUIRED') {
    return {
      ...base,
      title: 'Configuration needs recovery',
      message: 'The configuration update needs confirmation.',
      fallbackMessage: '配置更新结果需要确认。',
      details: base.details || base.message,
      suggestion: 'Reload the workspace before making more changes.',
    };
  }
  if (['CONFIGURATION_OPERATION_INTERRUPTED', 'CONFIGURATION_OPERATION_REJECTED', 'CONFIGURATION_OPERATION_MISMATCH'].includes(base.messageKey ?? '')) {
    return {
      ...base,
      title: 'Review configuration',
      message: 'Review the current configuration before applying again.',
      fallbackMessage: '请检查当前配置，再次应用。',
      details: base.details || base.message,
      suggestion: '',
    };
  }
  if (base.messageKey === 'CONFIGURATION_COMMIT_RECOVERY_REQUIRED') {
    return {
      ...base,
      title: 'Configuration needs recovery',
      message: 'The apply result needs confirmation.',
      fallbackMessage: '应用结果需要确认。',
      details: base.details || base.message,
      suggestion: 'Reload the workspace before making more changes.',
    };
  }
  if (base.messageKey === 'CONFIGURATION_RECOVERY_REQUIRED') {
    return {
      ...base,
      title: 'Configuration needs recovery',
      message: 'The operation could not be restored automatically.',
      fallbackMessage: '操作未能自动恢复。',
      details: base.details || base.message,
      suggestion: 'Reload the workspace before making more changes.',
    };
  }
  if (base.messageKey === 'CORE_TARGET_CHANGED' || base.messageKey === 'CORE_VALIDATION_EVIDENCE_STALE') {
    return {
      ...base,
      title: 'Review configuration',
      message: base.messageKey === 'CORE_TARGET_CHANGED'
        ? 'The program changed. Review it before applying again.'
        : 'The configuration needs a fresh check. Review and apply again.',
      details: base.details || base.message,
      suggestion: '',
    };
  }
  if (['CONFIGURATION_STATE_STALE', 'CONFIGURATION_GENERATION_STALE', 'CONFIGURATION_DRAFT_STALE']
    .includes(base.messageKey ?? '')) {
    return {
      ...base,
      title: contextTitle,
      message: 'The configuration changed elsewhere before this request was committed.',
      fallbackMessage: '配置在本次操作提交前已在其他位置更新。当前草稿和有效配置均已保留。',
      details: base.details || base.message,
      suggestion: 'Reload the latest configuration state, review the draft, and retry the same request.',
    };
  }
  if (base.messageKey === 'CONFIGURATION_BLOCKING_CONFLICT') {
    return {
      ...base,
      title: contextTitle,
      message: 'The candidate still has blocking configuration conflicts and was not saved.',
      fallbackMessage: '候选配置仍有阻塞性冲突，因此未保存。',
      details: base.details || base.message,
      suggestion: 'Resolve each conflict in its owning section or Final configuration, then save again.',
    };
  }
  if (base.code === 'PROGRAM_BUSY') {
    return {
      ...base,
      title: contextTitle,
      message: 'This configuration operation is waiting for another program operation to finish.',
      fallbackMessage: '配置操作正在等待程序完成另一项操作。',
      suggestion: 'Wait for the current operation to finish, then retry the same request.',
    };
  }
  if (base.messageKey === 'FINAL_EDIT_CONFLICT') {
    return {
      ...base,
      title: contextTitle,
      message: 'Resolve conflicts before continuing.',
      fallbackMessage: '请先解决冲突。',
      details: base.details || base.message,
      suggestion: '',
    };
  }
  if (base.messageKey === 'CONFIGURATION_SYNTAX_INVALID') {
    return {
      ...base,
      title: contextTitle,
      message: 'Fix the highlighted configuration syntax.',
      fallbackMessage: '请修正标出的配置语法。',
      details: base.details || base.message,
      suggestion: '',
    };
  }
  if (base.messageKey === 'CONFIGURATION_STATIC_INVALID') {
    return {
      ...base,
      title: contextTitle,
      message: 'This configuration needs a correction.',
      fallbackMessage: '这份配置需要修正。',
      details: base.details || base.message,
      suggestion: 'Review the highlighted issue and apply again.',
    };
  }
  if (base.messageKey === 'CORE_INVALID' || base.messageKey === 'CORE_NATIVE_REJECTED') {
    return {
      ...base,
      title: contextTitle,
      message: 'The selected Core rejected this candidate. Applied and Last Known Good were retained.',
      fallbackMessage: '当前 Core 拒绝了这份候选配置；Applied 和 Last Known Good 已保留。',
      suggestion: 'Review the highlighted issue and apply again.',
    };
  }
  if (['configuration-apply', 'configuration-save'].includes(context)
    && (!base.code || ['TIMEOUT', 'NETWORK', 'INTERNAL', 'STORAGE'].includes(base.code))) {
    return {
      ...base,
      title: contextTitle,
      message: context === 'configuration-save' ? 'The save result needs confirmation.' : 'The apply result needs confirmation.',
      fallbackMessage: context === 'configuration-save' ? '保存结果需要确认。' : '应用结果需要确认。',
      details: base.details || base.message,
      suggestion: 'Retry to check the result of this request.',
    };
  }
  if (base.code === 'CONFIG_INVALID' || base.code === 'CONFIG_CONFLICT') {
    return {
      ...base,
      title: contextTitle,
      message: 'This configuration needs a correction.',
      fallbackMessage: '这份配置需要修正。',
      details: base.details || base.message,
      suggestion: 'Review the highlighted issue and apply again.',
    };
  }
  if (base.code === 'STORAGE') {
    return {
      ...base,
      title: contextTitle,
      message: 'The configuration could not be saved.',
      fallbackMessage: '配置未能保存。',
      details: base.details || base.message,
      suggestion: 'Retry once. If it continues, inspect the diagnostic logs and available disk space.',
    };
  }
  return {
    ...base,
    title: contextTitle,
    message: 'The configuration request could not be completed.',
    fallbackMessage: '配置请求未能完成。',
    details: base.details || base.message,
    suggestion: base.suggestion === defaultSuggestion
      ? 'Review the configuration details and retry the same request.'
      : base.suggestion,
  };
}
