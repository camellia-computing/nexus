#!/usr/bin/env node

import assert from 'node:assert/strict';
import { resolve } from 'node:path';

import { resolveCargoTargetDirectory, validateTargetTriple } from './prepare-privilege-broker.mjs';

const root = resolve('fixture', 'project');
assert.equal(resolveCargoTargetDirectory(root), resolve(root, 'target'));
assert.equal(resolveCargoTargetDirectory(root, ''), resolve(root, 'target'));
assert.equal(resolveCargoTargetDirectory(root, '../shared-cache'), resolve(root, '../shared-cache'));
const absoluteCache = resolve('fixture', 'shared-cache');
assert.equal(resolveCargoTargetDirectory(root, absoluteCache), absoluteCache);

for (const valid of [
  'x86_64-unknown-linux-gnu',
  'aarch64-apple-darwin',
  'x86_64-pc-windows-msvc',
]) {
  assert.equal(validateTargetTriple(valid), valid);
}

for (const invalid of [
  '',
  '../x86_64-unknown-linux-gnu',
  'x86_64/unknown/linux/gnu',
  'x86_64-unknown',
  'x86_64-unknown-linux-gnu/../../outside',
  `x86_64-unknown-linux-gnu${'a'.repeat(129)}`,
]) {
  assert.throws(() => validateTargetTriple(invalid), /invalid TAURI_ENV_TARGET_TRIPLE/u);
}

console.log('Privilege-broker target validation verified');
