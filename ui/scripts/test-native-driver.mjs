import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const nativeUtils = await import('@wdio/native-utils');
assert.equal(
  typeof nativeUtils.installMockSyncOverride,
  'function',
  '@wdio/native-utils must expose the synchronization hook required by the Tauri service'
);

await import('@wdio/tauri-service');
// This import is also the executable contract for the temporary
// GHSA-mh99-v99m-4gvg / GHSA-rgw5-rvv9-x895 bridge in
// docs/dependency-management.md. It must remain
// until the WebdriverIO graph natively consumes the current brace-expansion API.
await import('@wdio/cli');

const mochaRequire = createRequire(import.meta.resolve('mocha'));
const diff = mochaRequire('diff');
const patch = diff.createPatch('fixture', 'before\n', 'after\n');
assert.equal(diff.applyPatch('before\n', patch), 'after\n');
assert.equal(diff.diffLines('before\n', 'after\n')[0].removed, true);

const { adapterFactory } = await import('@wdio/mocha-framework');
const reporter = new EventEmitter();
let passed = 0;
let pending = 0;
reporter.on('test:pass', () => passed++);
reporter.on('test:pending', () => pending++);
const adapter = await adapterFactory.init(
  '0-0',
  { rootDir: process.cwd(), mochaOpts: { ui: 'bdd', timeout: 2000 } },
  [fileURLToPath(new URL('./fixtures/mocha-contract.mjs', import.meta.url))],
  {},
  reporter
);
assert.equal(adapter.hasTests(), true);
assert.equal(await adapter.run(), 0);
assert.equal(passed, 1);
assert.equal(pending, 1);

console.log('Native WebdriverIO dependency contract passed.');
