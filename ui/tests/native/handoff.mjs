import { readFile, unlink, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const retryableReadErrors = new Set(['EACCES', 'EBUSY', 'ENOENT', 'EPERM']);

function handoffPath(name) {
  if (!/^[a-z][a-z0-9-]{0,63}\.(?:json|token)$/u.test(name)) {
    throw new Error('invalid native E2E handoff name');
  }
  const root = process.env.CAMELLIA_NEXUS_E2E_HANDOFF_DIR;
  if (!root) throw new Error('CAMELLIA_NEXUS_E2E_HANDOFF_DIR is required');
  return path.join(root, name);
}

export async function writeHandoff(name, value) {
  await writeFile(handoffPath(name), value, { encoding: 'utf8', mode: 0o600, flag: 'wx' });
}

export async function readHandoff(name, consume = false) {
  const target = handoffPath(name);
  const value = await readFile(target, 'utf8');
  if (consume) await unlink(target);
  return value;
}

export async function waitForHandoff(name, consume = false, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (true) {
    try {
      return await readHandoff(name, consume);
    } catch (error) {
      if (!retryableReadErrors.has(error?.code)) throw error;
      if (Date.now() >= deadline) {
        throw new Error(`native E2E handoff ${name} was not published in time`, { cause: error });
      }
      await delay(100);
    }
  }
}
