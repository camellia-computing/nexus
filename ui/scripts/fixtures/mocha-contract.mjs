import assert from 'node:assert/strict';

describe('native framework contract', function () {
  let ready = false;
  before(function () {
    ready = true;
  });
  it('runs asynchronous tests after setup', async function () {
    await Promise.resolve();
    assert.equal(ready, true);
  });
  it.skip('does not execute a skipped test', function () {
    assert.fail('skipped test executed');
  });
});
