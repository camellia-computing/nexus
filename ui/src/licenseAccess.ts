import type { Capability, EntitlementState, LicenseLimits, VerifiedEntitlement } from './types';

export interface LicenseAccess {
  entitlement: VerifiedEntitlement | null;
  active: boolean;
  capabilities: ReadonlySet<Capability>;
  limits: LicenseLimits | null;
  canUseLocalPrograms: boolean;
  canUseManagedSources: boolean;
  canRunAdvancedDiagnostics: boolean;
  canOpenRemoteDashboard: boolean;
  canUseManagedPackages: boolean;
  canAdministerTeam: boolean;
  canCreateProgram: boolean;
  programLimitReached: boolean;
  configurationValid: boolean;
}

export type ProgramLifecycleAction = 'start' | 'stop' | 'restart';

export function canUseProgramLifecycleAction(
  access: Pick<LicenseAccess, 'canUseLocalPrograms'>,
  action: ProgramLifecycleAction,
): boolean {
  return action === 'stop' || access.canUseLocalPrograms;
}

const requiredLimits: (keyof LicenseLimits)[] = [
  'max_programs',
  'max_config_sources_per_program',
  'max_team_members',
  'max_remote_monitors',
  'max_shared_programs',
  'max_webhook_endpoints',
  'max_workspace_storage_bytes',
  'max_alert_rules',
  'max_audit_export_events',
];

export function deriveLicenseAccess(
  state: EntitlementState | null,
  programCount: number,
): LicenseAccess {
  const entitlement = state?.status === 'active' ? state.entitlement : null;
  const capabilities = new Set(entitlement?.claims.capabilities ?? []);
  const limits = entitlement?.claims.limits ?? null;
  const configurationValid = !!limits && requiredLimits.every((key) => (
    Number.isSafeInteger(limits[key]) && limits[key] >= 0
  ));
  const active = !!entitlement && configurationValid;
  const maxPrograms = limits?.max_programs;
  const programLimitReached = active && typeof maxPrograms === 'number'
    ? programCount >= maxPrograms
    : true;

  return {
    entitlement,
    active,
    capabilities,
    limits,
    canUseLocalPrograms: active,
    canUseManagedSources: active && capabilities.has('managed_config_sources'),
    canRunAdvancedDiagnostics: active && capabilities.has('advanced_diagnostics'),
    canOpenRemoteDashboard: active && capabilities.has('remote_dashboard'),
    canUseManagedPackages: active && capabilities.has('managed_program_packages'),
    canAdministerTeam: active && capabilities.has('team_administration'),
    canCreateProgram: active && !programLimitReached,
    programLimitReached,
    configurationValid,
  };
}
