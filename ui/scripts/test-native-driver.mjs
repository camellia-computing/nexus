import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

const webdriverRequire = createRequire(import.meta.resolve('webdriverio'));
const archiverRequire = createRequire(webdriverRequire.resolve('archiver'));
const directoryRequire = createRequire(archiverRequire.resolve('readdir-glob'));
const matcherRequire = createRequire(directoryRequire.resolve('minimatch'));
const expansion = await import(pathToFileURL(join(dirname(matcherRequire.resolve('brace-expansion/package.json')), 'dist/esm/index.js')).href);
const commonExpansion = matcherRequire('brace-expansion');
assert.equal(typeof commonExpansion, 'function');
assert.equal(expansion.default, expansion.expand);
for (const limit of ['EXPANSION_MAX', 'EXPANSION_MAX_LENGTH', 'EXPANSION_MAX_DEPTH', 'EXPANSION_MAX_REWRITES']) {
  assert.equal(commonExpansion[limit], expansion[limit]);
  assert.equal(typeof expansion[limit], 'number');
}
assert.deepEqual(commonExpansion('{a,b}'), expansion.expand('{a,b}'));
assert.equal(commonExpansion('{a,b}', { max: 1 }).length, 1);

const nativeUtils = await import('@wdio/native-utils');
assert.equal(
  typeof nativeUtils.installMockSyncOverride,
  'function',
  '@wdio/native-utils must expose the synchronization hook required by the Tauri service'
);

await import('@wdio/tauri-service');
// Import the complete consumer graph, including callable/default expansion consumers.
await import('@wdio/cli');

console.log('Native WebdriverIO dependency contract passed.');
