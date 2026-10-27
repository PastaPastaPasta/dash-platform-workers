import assert from 'node:assert/strict';
import { test } from 'node:test';
import { encodeRequest, readResponse } from '../src/http1.js';

const enc = new TextEncoder();
const bytes = (s) => (typeof s === 'string' ? enc.encode(s) : s);

/** A socket-like stream that yields `parts` one read at a time, then optionally stays open. */
function socket(parts, { hang = false } = {}) {
  let i = 0;
  return new ReadableStream({
    pull(controller) {
      if (i < parts.length) controller.enqueue(bytes(parts[i++]));
      else if (!hang) controller.close();
    },
  });
}

/** Splits `s` into pieces of `n` bytes, so parsing must cope with any boundary. */
const pieces = (s, n) => {
  const b = bytes(s);
  const out = [];
  for (let i = 0; i < b.length; i += n) out.push(b.slice(i, i + n));
  return out;
};

test('should frame a gRPC-web POST with its body and close the connection', () => {
  const url = new URL('https://1.2.3.4:1443/org.dash.platform.dapi.v0.Platform/getStatus?x=1');
  const req = new Request(url, { method: 'POST', headers: { 'content-type': 'application/grpc-web+proto' } });
  const body = new Uint8Array([0, 0, 0, 0, 1, 7]);
  const framed = encodeRequest(req, url, body);
  const text = new TextDecoder().decode(framed);
  assert.match(text, /^POST \/org\.dash\.platform\.dapi\.v0\.Platform\/getStatus\?x=1 HTTP\/1\.1\r\n/);
  assert.match(text, /\r\nHost: 1\.2\.3\.4:1443\r\n/);
  assert.match(text, /\r\ncontent-type: application\/grpc-web\+proto\r\n/);
  assert.match(text, /\r\nContent-Length: 6\r\nConnection: close\r\n\r\n/);
  assert.deepEqual([...framed.slice(-6)], [...body]);
});

test('should read a content-length body split across reads and ignore trailing bytes', async () => {
  let released = 0;
  const res = await readResponse(
    socket(pieces('HTTP/1.1 200 OK\r\ncontent-type: application/grpc-web+proto\r\ncontent-length: 5\r\n\r\nhelloEXTRA', 3)),
    () => released++,
  );
  assert.equal(res.status, 200);
  assert.equal(res.headers.get('content-type'), 'application/grpc-web+proto');
  assert.equal(await res.text(), 'hello');
  assert.equal(released, 1);
});

test('should reassemble a chunked body split at every byte', async () => {
  const res = await readResponse(
    socket(pieces('HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabc\r\na\r\n0123456789\r\n0\r\n\r\n', 1)),
  );
  assert.equal(await res.text(), 'abc0123456789');
  assert.equal(res.headers.get('transfer-encoding'), null);
});

test('should deliver each chunk of an open stream before the stream ends', async () => {
  const res = await readResponse(
    socket(['HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n', '4\r\nmsg1\r\n', '4\r\nmsg2\r\n'], { hang: true }),
  );
  const reader = res.body.getReader();
  assert.equal(new TextDecoder().decode((await reader.read()).value), 'msg1');
  assert.equal(new TextDecoder().decode((await reader.read()).value), 'msg2');
  await reader.cancel();
});

test('should read a body without length until the connection closes', async () => {
  const res = await readResponse(socket(['HTTP/1.1 200 OK\r\n\r\npart1', 'part2']));
  assert.equal(await res.text(), 'part1part2');
});

test('should release the connection when the reader cancels', async () => {
  let released = 0;
  const res = await readResponse(
    socket(['HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n', '1\r\na\r\n'], { hang: true }),
    () => released++,
  );
  const reader = res.body.getReader();
  await reader.read();
  await reader.cancel();
  assert.equal(released, 1);
});

test('should reject a response cut off before its headers end', async () => {
  let released = 0;
  await assert.rejects(readResponse(socket(['HTTP/1.1 200 OK\r\ncontent-type: x\r\n']), () => released++), /truncated/);
  assert.equal(released, 1);
});

test('should fail the body when a chunk is cut off', async () => {
  const res = await readResponse(socket(['HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\na\r\n01234']));
  await assert.rejects(res.text(), /truncated/);
});

test('should fail the body when a content-length body is cut off', async () => {
  const res = await readResponse(socket(['HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\n01234']));
  await assert.rejects(res.text(), /truncated/);
});

test('should reject a response that is not HTTP/1.x', async () => {
  await assert.rejects(readResponse(socket(['SSH-2.0-OpenSSH\r\n\r\n'])), /malformed status line/);
});

test('should reject a negative chunk size instead of looping on it', async () => {
  const res = await readResponse(socket(['HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n-6\r\nabcdef\r\n0\r\n\r\n']));
  await assert.rejects(res.text(), /malformed chunk size/);
});

test('should accept chunk extensions after the size', async () => {
  const res = await readResponse(socket(['HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n3;name=v\r\nabc\r\n0\r\n\r\n']));
  assert.equal(await res.text(), 'abc');
});

test('should reject a chunk larger than the cap before buffering it', async () => {
  const res = await readResponse(socket(['HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\nffffffff\r\n'], { hang: true }));
  await assert.rejects(res.text(), /too large/);
});

test('should reject a malformed content-length', async () => {
  await assert.rejects(readResponse(socket(['HTTP/1.1 200 OK\r\ncontent-length: 12abc\r\n\r\n'])), /malformed content-length/);
});

test('should reject a response head that never ends', async () => {
  const filler = 'x-filler: ' + 'a'.repeat(1000) + '\r\n';
  await assert.rejects(readResponse(socket(['HTTP/1.1 200 OK\r\n', ...Array(70).fill(filler)], { hang: true })), /too large/);
});

test('should count multi-byte header values in the framed length', () => {
  const url = new URL('https://1.2.3.4/x');
  const req = new Request(url, { method: 'POST', headers: { 'x-note': 'ünïcode' } });
  const framed = encodeRequest(req, url, new Uint8Array([9, 9]));
  assert.deepEqual([...framed.slice(-2)], [9, 9]);
});
