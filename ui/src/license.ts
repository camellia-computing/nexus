import type {
  ClientVersionPolicy,
  EntitlementSnapshot,
  EntitlementState,
  LicenseStateChangedEvent,
  VerifiedEntitlement,
} from './types';

export type LicenseRuntimeImpact = 'active' | 'restrictedOffline' | 'hardInactive';
type DeviceDeniedState = Extract<EntitlementState, { status: 'deviceDenied' }>['state'];
type RevalidationReason = Extract<EntitlementState, { status: 'revalidationRequired' }>['reason'];

export interface LicenseNotice {
  title: string;
  message: string;
  suggestion: string;
  additionalMessages?: string[];
}

export interface SignedLicenseStatusPresentation {
  label: string;
  tone: 'success' | 'warning' | 'danger';
}

export function isNewerEntitlementSnapshot(
  snapshot: EntitlementSnapshot,
  currentGeneration: number,
) {
  return (
    Number.isSafeInteger(snapshot.generation) &&
    snapshot.generation > currentGeneration
  );
}

export type ClientVersionAdvisory =
  | { kind: 'required'; policy: ClientVersionPolicy }
  | { kind: 'requiredBefore'; policy: ClientVersionPolicy }
  | { kind: 'recommended'; policy: ClientVersionPolicy };

interface ParsedSemVer {
  core: [string, string, string];
  prerelease: string[];
}

const canonicalSemVerPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

export function compareCanonicalSemVer(left: string, right: string): -1 | 0 | 1 | null {
  const parsedLeft = parseCanonicalSemVer(left);
  const parsedRight = parseCanonicalSemVer(right);
  if (!parsedLeft || !parsedRight) return null;
  for (let index = 0; index < parsedLeft.core.length; index += 1) {
    const comparison = compareNumericIdentifier(parsedLeft.core[index], parsedRight.core[index]);
    if (comparison) return comparison;
  }
  if (!parsedLeft.prerelease.length && !parsedRight.prerelease.length) return 0;
  if (!parsedLeft.prerelease.length) return 1;
  if (!parsedRight.prerelease.length) return -1;
  const length = Math.max(parsedLeft.prerelease.length, parsedRight.prerelease.length);
  for (let index = 0; index < length; index += 1) {
    const leftIdentifier = parsedLeft.prerelease[index];
    const rightIdentifier = parsedRight.prerelease[index];
    if (leftIdentifier === undefined) return -1;
    if (rightIdentifier === undefined) return 1;
    if (leftIdentifier === rightIdentifier) continue;
    const leftNumeric = /^\d+$/.test(leftIdentifier);
    const rightNumeric = /^\d+$/.test(rightIdentifier);
    if (leftNumeric && rightNumeric) {
      return compareNumericIdentifier(leftIdentifier, rightIdentifier);
    }
    if (leftNumeric) return -1;
    if (rightNumeric) return 1;
    return leftIdentifier < rightIdentifier ? -1 : 1;
  }
  return 0;
}

export function clientVersionAdvisory(
  state: EntitlementState | null,
  currentVersion: string,
): ClientVersionAdvisory | null {
  if (!state) return null;
  if (state.status === 'clientUpgradeRequired') {
    return { kind: 'required', policy: state.policy };
  }
  const policy = clientVersionPolicyOf(state);
  if (!policy || !currentVersion) return null;
  const minimumComparison = compareCanonicalSemVer(currentVersion, policy.minimumVersion);
  if (minimumComparison === null) return null;
  if (minimumComparison < 0) {
    // Enforcement time is evaluated by Rust against trusted time. An Active state is therefore
    // necessarily still before enforcement even if the mutable browser wall clock says otherwise.
    return { kind: 'requiredBefore', policy };
  }
  const recommendedComparison = compareCanonicalSemVer(currentVersion, policy.recommendedVersion);
  return recommendedComparison !== null && recommendedComparison < 0
    ? { kind: 'recommended', policy }
    : null;
}

export function clientVersionNotice(
  state: EntitlementState | null,
  currentVersion: string,
): LicenseNotice | null {
  const advisory = clientVersionAdvisory(state, currentVersion);
  if (!advisory) return null;
  switch (advisory.kind) {
    case 'required':
      return {
        title: 'Camellia Nexus update required',
        message: 'This client version no longer meets the signed minimum version policy, so protected features are unavailable.',
        suggestion: 'Install a supported Camellia Nexus version, reopen the app, and refresh the license.',
      };
    case 'requiredBefore':
      return {
        title: 'Camellia Nexus update required soon',
        message: 'This client version is below the signed minimum version and will stop accessing protected features when enforcement begins.',
        suggestion: 'Update Camellia Nexus before the enforcement time shown in License settings.',
      };
    case 'recommended':
      return {
        title: 'Camellia Nexus update available',
        message: 'A newer Camellia Nexus version is recommended by the signed license policy.',
        suggestion: 'Update when convenient to receive the latest fixes and support.',
      };
  }
}

export function signedLicenseStatusPresentation(
  status: VerifiedEntitlement['claims']['licenseStatus'],
): SignedLicenseStatusPresentation {
  switch (status) {
    case 'active':
      return { label: 'In good standing', tone: 'success' };
    case 'past_due':
      return { label: 'Payment past due', tone: 'warning' };
    case 'canceled':
      return { label: 'Canceled', tone: 'danger' };
  }
}

export function hasRefreshableLicenseSession(state: EntitlementState | null) {
  return !!state && (
    state.status === 'activationPending'
    || state.status === 'active'
    || state.status === 'restrictedOffline'
    || state.status === 'expired'
    || state.status === 'revalidationRequired'
    || state.status === 'clientUpgradeRequired'
    || state.status === 'licenseInactive'
  );
}

export function licenseRuntimeImpact(state: EntitlementState | null): LicenseRuntimeImpact {
  if (state?.status === 'active') return 'active';
  if (state?.status === 'restrictedOffline') return 'restrictedOffline';
  return 'hardInactive';
}

export function licenseStateNotice(
  state: EntitlementState | null,
  currentVersion = '',
): LicenseNotice | null {
  if (!state) return null;
  const versionNotice = clientVersionNotice(state, currentVersion);
  if (state.status === 'clientUpgradeRequired') return versionNotice;
  if (state.status === 'active') {
    if (state.entitlement.claims.licenseStatus === 'past_due') {
      return {
        title: 'License payment past due',
        message: 'Access remains available only through the signed commercial grace term shown in License settings.',
        suggestion: 'Update billing details before the grace term ends, then refresh the license.',
      };
    }
    if (state.entitlement.claims.licenseStatus === 'canceled') {
      return {
        title: 'License canceled',
        message: 'Access remains available only through the signed cancellation grace term shown in License settings.',
        suggestion: 'Renew the license or switch licenses before the grace term ends.',
      };
    }
    return versionNotice;
  }
  if (state.status === 'activationPending') {
    return {
      title: 'Device activation is completing',
      message: 'The device registration is saved, but a signed entitlement has not been installed yet.',
      suggestion: 'Reconnect and refresh the license; the authorization code does not need to be entered again.',
    };
  }
  if (state.status === 'restrictedOffline') {
    return {
      title: 'License offline grace period',
      message: 'Existing programs can keep running for up to 24 hours, but protected changes and remote control are unavailable until the license is refreshed.',
      suggestion: 'Reconnect and refresh the license.',
    };
  }
  if (state.status === 'expired') {
    return {
      title: 'Offline credential expired',
      message: 'The cached entitlement and its 24-hour offline safety window have ended. The commercial license term may still be valid.',
      suggestion: 'Refresh or reactivate the license.',
    };
  }
  if (state.status === 'deviceDenied') {
    return {
      title: 'Device authorization revoked',
      message: deviceDeniedMessage(state.state),
      suggestion: deviceDeniedSuggestion(state.state),
    };
  }
  if (state.status === 'licenseInactive') {
    return licenseInactiveNotice(state.reason);
  }
  if (state.status === 'revalidationRequired') {
    return {
      title: 'License revalidation required',
      message: revalidationMessage(state.reason),
      suggestion: 'Reconnect and refresh the license.',
    };
  }
  return {
    title: 'License required',
    message: 'Activate this device with a valid license to use protected features.',
    suggestion: 'Open License settings to activate this device.',
  };
}

export function licenseRuntimeNotice(
  event: LicenseStateChangedEvent,
  currentVersion = '',
): LicenseNotice | null {
  const notice = licenseStateNotice(event.entitlementState, currentVersion);
  if (!notice) return null;
  const runtimeMessages = [
    event.stoppedPrograms > 0 ? stoppedProgramsMessage(event.stoppedPrograms) : '',
    event.failedPrograms > 0
      ? failedProgramsMessage(event.failedPrograms, event.failedProgramIds)
      : '',
  ].filter(Boolean);
  if (!runtimeMessages.length) return notice;
  return {
    ...notice,
    additionalMessages: runtimeMessages,
    suggestion: event.failedPrograms > 0
      ? 'Stop remaining programs manually and inspect the program logs.'
      : notice.suggestion,
  };
}

export function licenseNoticeRequiresPersistentAttention(event: LicenseStateChangedEvent) {
  return event.failedPrograms > 0;
}

export function licenseNoticeKey(event: LicenseStateChangedEvent) {
  const entitlement = 'entitlement' in event.entitlementState
    ? event.entitlementState.entitlement ?? undefined
    : undefined;
  const versionPolicy = event.entitlementState.status === 'clientUpgradeRequired'
    ? event.entitlementState.policy
    : entitlement?.claims.clientVersionPolicy;
  return [
    event.runtimeImpact,
    event.entitlementState.status,
    'reason' in event.entitlementState ? event.entitlementState.reason : '',
    'state' in event.entitlementState ? event.entitlementState.state : '',
    entitlement?.claims.licenseStatus ?? '',
    entitlement?.claims.licenseExpiresAt ?? '',
    versionPolicy?.minimumVersion ?? '',
    versionPolicy?.recommendedVersion ?? '',
    versionPolicy?.enforceAfter ?? '',
    event.stoppedPrograms > 0 ? 'stopped' : 'none-stopped',
    event.failedPrograms > 0 ? 'stop-failed' : 'none-failed',
    event.failedProgramIds.join(','),
  ].join(':');
}

function parseCanonicalSemVer(value: string): ParsedSemVer | null {
  const match = canonicalSemVerPattern.exec(value);
  if (!match || match[0] !== value) return null;
  return {
    core: [match[1], match[2], match[3]],
    prerelease: match[4]?.split('.') ?? [],
  };
}

function compareNumericIdentifier(left: string, right: string): -1 | 0 | 1 {
  if (left.length !== right.length) return left.length < right.length ? -1 : 1;
  if (left === right) return 0;
  return left < right ? -1 : 1;
}

function clientVersionPolicyOf(state: EntitlementState): ClientVersionPolicy | null {
  if (!('entitlement' in state) || !state.entitlement) return null;
  return state.entitlement.claims.clientVersionPolicy;
}

function stoppedProgramsMessage(count: number) {
  return count === 1
    ? 'One managed program was stopped.'
    : 'Managed programs were stopped.';
}

function failedProgramsMessage(count: number, programIds: string[]) {
  const visibleIds = programIds.slice(0, 3);
  if (visibleIds.length) {
    const remainder = count - visibleIds.length;
    return remainder > 0
      ? `Could not stop ${visibleIds.join(', ')} and ${remainder} other managed program${remainder === 1 ? '' : 's'}.`
      : `Could not stop: ${visibleIds.join(', ')}.`;
  }
  return count === 1
    ? 'One managed program could not be stopped automatically.'
    : 'Some managed programs could not be stopped automatically.';
}

function deviceDeniedMessage(state: DeviceDeniedState) {
  switch (state) {
    case 'removed':
      return 'This device was removed from the license.';
    case 'revoked':
      return 'This device was revoked by the license service.';
    case 'suspicious':
      return 'This license session was rejected by the license service.';
    default:
      return 'This device is not authorized for the current license.';
  }
}

function deviceDeniedSuggestion(state: DeviceDeniedState) {
  switch (state) {
    case 'removed':
      return 'Open License settings and activate this device again.';
    case 'revoked':
      return 'Contact the license administrator before attempting to activate this device again.';
    case 'suspicious':
      return 'Contact support and review the device security before attempting another activation.';
    default:
      return 'Contact the license administrator or support.';
  }
}

function revalidationMessage(reason: RevalidationReason) {
  switch (reason) {
    case 'clock_rollback':
      return 'The local clock changed unexpectedly and the license must be revalidated online.';
    case 'obsolete_epoch':
      return 'The license policy changed and this device must refresh its entitlement.';
    case 'corrupt_secure_store':
      return 'The local license data could not be read safely.';
    case 'invalid_server_proof':
      return 'The license service response could not be authenticated by this build.';
    default:
      return 'The license must be revalidated online.';
  }
}

function licenseInactiveNotice(
  reason: Extract<EntitlementState, { status: 'licenseInactive' }>['reason'],
): LicenseNotice {
  switch (reason) {
    case 'account_suspended':
      return {
        title: 'License account suspended',
        message: 'The license account is temporarily suspended, so protected features are unavailable.',
        suggestion: 'Contact the license administrator or support before retrying.',
      };
    case 'account_denylisted':
      return {
        title: 'License account disabled',
        message: 'The license account is no longer permitted to use protected features.',
        suggestion: 'Contact support if you believe this is an error.',
      };
    case 'license_past_due':
      return {
        title: 'License payment past due',
        message: 'Protected features are unavailable because the license payment is past due.',
        suggestion: 'Update billing details, then refresh the license.',
      };
    case 'license_canceled':
      return {
        title: 'License canceled',
        message: 'The license has been canceled and protected features are unavailable.',
        suggestion: 'Renew the license or activate this device with another valid license.',
      };
    case 'license_expired':
      return {
        title: 'License expired',
        message: 'The license term has ended and protected features are unavailable.',
        suggestion: 'Renew the license or activate this device with another valid license.',
      };
    case 'license_unavailable':
      return {
        title: 'License unavailable',
        message: 'No usable license is currently assigned to this account.',
        suggestion: 'Contact the license administrator or activate another valid license.',
      };
  }
}
