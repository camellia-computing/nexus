import assert from 'node:assert/strict';

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

console.log('Native WebdriverIO dependency contract passed.');
