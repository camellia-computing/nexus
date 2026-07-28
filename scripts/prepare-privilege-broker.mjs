#!/usr/bin/env node

import { access, copyFile, mkdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const targetTriple = process.env.TAURI_ENV_TARGET_TRIPLE;
if (!targetTriple) process.exit(0);

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const debug = process.env.TAURI_ENV_DEBUG === 'true';
const profile = debug ? 'debug' : 'release';
const windows = targetTriple.includes('windows');
const extension = windows ? '.exe' : '';
const destinationDirectory = join(root, 'src-tauri', 'binaries');
const destination = join(
  destinationDirectory,
  `camellia-nexus-privilege-broker-${targetTriple}${extension}`,
);
const runtimeDestination = join(
  root,
  'target',
  profile,
  `camellia-nexus-privilege-broker${extension}`,
);
if (process.env.CAMELLIA_NEXUS_PRIVILEGE_BROKER_PREPARED === '1') {
  await Promise.all([access(destination), access(runtimeDestination)]);
  process.exit(0);
}
const cargo = process.env.CARGO || 'cargo';
const cargoArguments = [
  'build',
  '--locked',
  '--package',
  'camellia-nexus-privilege-broker',
  '--target',
  targetTriple,
];
if (!debug) cargoArguments.push('--release');

const result = spawnSync(cargo, cargoArguments, { cwd: root, stdio: 'inherit' });
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

const source = join(
  root,
  'target',
  targetTriple,
  profile,
  `camellia-nexus-privilege-broker${extension}`,
);
await mkdir(destinationDirectory, { recursive: true });
await copyFile(source, destination);
await mkdir(join(root, 'target', profile), { recursive: true });
await copyFile(source, runtimeDestination);
