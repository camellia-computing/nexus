import type { ProgramState } from './types';

const runtimeActiveStatuses = new Set<ProgramState['status']>([
  'starting',
  'running',
  'stopping',
  'backoff',
  'stopFailed',
]);

export function isRuntimeActive(state: ProgramState | undefined): boolean {
  return !!state && runtimeActiveStatuses.has(state.status);
}
