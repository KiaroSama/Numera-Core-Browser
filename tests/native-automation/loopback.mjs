import assert from 'node:assert/strict';

export function loopbackPort(address) {
  // ChromeDriver uses localhost in debuggerAddress even for its IPv4 listener.
  const match = /^(?:localhost|127\.0\.0\.1):([1-9][0-9]{0,4})$/.exec(address);
  assert.ok(match && match[0] === address && Number(match[1]) <= 65535,
    'Expected a local Chromium automation endpoint');
  return `http://127.0.0.1:${match[1]}`;
}
