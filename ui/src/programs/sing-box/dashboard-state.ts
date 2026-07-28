import type { SingBoxDashboardChange, SingBoxDashboardOptions } from './index';

export function applySingBoxDashboardChange(
  current: SingBoxDashboardOptions,
  change: SingBoxDashboardChange,
): SingBoxDashboardOptions {
  return change.kind === 'native'
    ? { ...current, native: change.value }
    : { ...current, clash: change.value };
}
