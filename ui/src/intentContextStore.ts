import type { IntentContext } from './types';

/** A revision-bound, read-only cache. Pending reads are shared; rejected reads are not cached. */
export function createIntentContextStore(
  read: (programId: string, nodeId?: string) => Promise<IntentContext>,
  limit = 48,
) {
  const entries = new Map<string, Promise<IntentContext>>();
  return {
    load(programId: string, stateRevision: number, profileHash: string, nodeId?: string) {
      const key = JSON.stringify([programId, stateRevision, profileHash, nodeId ?? null]);
      const existing = entries.get(key);
      if (existing) {
        entries.delete(key);
        entries.set(key, existing);
        return existing;
      }
      const pending = read(programId, nodeId).then((context) => {
        if (context.stateRevision !== stateRevision) {
          throw { code: 'CONFIG_CONFLICT', messageKey: 'INTENT_PLAN_STALE' };
        }
        return context;
      });
      entries.set(key, pending);
      while (entries.size > limit) entries.delete(entries.keys().next().value!);
      void pending.catch(() => { if (entries.get(key) === pending) entries.delete(key); });
      return pending;
    },
    clear() { entries.clear(); },
  };
}
