import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loopbackPort } from './loopback.mjs';

test('normalizes the actual ChromeDriver hostname to IPv4 without DNS', () => {
  assert.equal(loopbackPort('localhost:53124'), 'http://127.0.0.1:53124');
  assert.equal(loopbackPort('127.0.0.1:65535'), 'http://127.0.0.1:65535');
});

test('rejects non-loopback or malformed debugger addresses', () => {
  for (const address of [undefined, '0.0.0.0:53124', 'localhost.example:80', 'localhost:65536',
    'localhost:0', 'localhost:53124\n', 'http://localhost:53124', '127.0.0.1:80@external.test']) {
    assert.throws(() => loopbackPort(address), /Expected a local Chromium automation endpoint/);
  }
});
