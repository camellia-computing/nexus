#!/usr/bin/env node

import { access, copyFile, mkdir } from 'node:fs/promises';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { spawnSync } from 'node:child_process';

const TARGET_TRIPLE = /^[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,4}$/u;

export function validateTargetTriple(value) {
  if (value.length > 128 || !TARGET_TRIPLE.test(value)) {
    throw new Error(`invalid TAURI_ENV_TARGET_TRIPLE: ${JSON.stringify(value)}`);
  }
  return value;
}

function resolveWithin(root, ...parts) {
  const path = resolve(root, ...parts);
  const relation = relative(root, path);
  if (
    relation === '..'
    || relation.startsWith(`..${sep}`)
    || isAbsolute(relation)
  ) {
    throw new Error(`resolved privilege-broker path escapes its root: ${path}`);
  }
  return path;
}

async function main() {
  const configuredTarget = process.env.TAURI_ENV_TARGET_TRIPLE;
  if (!configuredTarget) return;
  const targetTriple = validateTargetTriple(configuredTarget);

  const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
  const debug = process.env.TAURI_ENV_DEBUG === 'true';
  const profile = debug ? 'debug' : 'release';
  const windows = targetTriple.split('-').includes('windows');
  const extension = windows ? '.exe' : '';
  const destinationDirectory = resolveWithin(root, 'src-tauri', 'binaries');
  const destination = resolveWithin(
    destinationDirectory,
    `camellia-nexus-privilege-broker-${targetTriple}${extension}`,
  );
  const runtimeDirectory = resolveWithin(root, 'target', profile);
  const runtimeDestination = resolveWithin(
    runtimeDirectory,
    `camellia-nexus-privilege-broker${extension}`,
  );
  if (process.env.CAMELLIA_NEXUS_PRIVILEGE_BROKER_PREPARED === '1') {
    await Promise.all([access(destination), access(runtimeDestination)]);
    return;
  }
  const cargoArguments = [
    'build',
    '--locked',
    '--package',
    'camellia-nexus-privilege-broker',
    '--target',
    targetTriple,
  ];
  if (!debug) cargoArguments.push('--release');

  const result = spawnSync('cargo', cargoArguments, { cwd: root, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);

  const source = resolveWithin(
    root,
    'target',
    targetTriple,
    profile,
    `camellia-nexus-privilege-broker${extension}`,
  );
  await mkdir(destinationDirectory, { recursive: true });
  await copyFile(source, destination);
  await mkdir(runtimeDirectory, { recursive: true });
  await copyFile(source, runtimeDestination);
}

if (
  process.argv[1]
  && import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  await main();
}
