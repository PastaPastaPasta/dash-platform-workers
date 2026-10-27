import assert from 'node:assert/strict';
import { test } from 'node:test';
import { decodeResponse, encodeRequest } from '../src/http1.js';

const enc = new TextEncoder();
const bytes = (...parts) => {
  const arrays = parts.map((p) => (typeof p === 'string' ? enc.encode(p) : p));
  const out = new Uint8Array(arrays.reduce((n, a) => n + a.length, 0));
  let o = 0;
  for (const a of arrays) { out.set(a, o); o += a.length; }
  return out;
};

test('should frame a gRPC-web POST with its body and close the connection', () => {
  const url = new URL('https://1.2.3.4:1443/org.dash.platform.dapi.v0.Platform/getStatus?x=1');
  const req = new Request(url, { method: 'POST', headers: { 'content-type': 'application/grpc-web+proto' } });
  const body = new Uint8Array([0, 0, 0, 0, 1, 7]);
  const text = new TextDecoder().decode(encodeRequest(req, url, body));
  assert.match(text, /^POST \/org\.dash\.platform\.dapi\.v0\.Platform\/getStatus\?x=1 HTTP\/1\.1\r\n/);
  assert.match(text, /\r\nHost: 1\.2\.3\.4:1443\r\n/);
  assert.match(text, /\r\ncontent-type: application\/grpc-web\+proto\r\n/);
  assert.match(text, /\r\nContent-Length: 6\r\nConnection: close\r\n\r\n/);
  assert.deepEqual([...encodeRequest(req, url, body).slice(-6)], [...body]);
});

test('should decode a content-length response and ignore trailing bytes', async () => {
  const raw = bytes('HTTP/1.1 200 OK\r\ncontent-type: application/grpc-web+proto\r\ncontent-length: 3\r\n\r\n', new Uint8Array([1, 2, 3, 9]));
  const res = decodeResponse(raw);
  assert.equal(res.status, 200);
  assert.equal(res.headers.get('content-type'), 'application/grpc-web+proto');
  assert.deepEqual([...new Uint8Array(await res.arrayBuffer())], [1, 2, 3]);
});

test('should reassemble a chunked body', async () => {
  const raw = bytes('HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n', '3\r\nabc\r\n', 'a\r\n0123456789\r\n', '0\r\n\r\n');
  const res = decodeResponse(raw);
  assert.equal(await res.text(), 'abc0123456789');
  assert.equal(res.headers.get('transfer-encoding'), null);
});

test('should reject a response cut off before its headers end', () => {
  assert.throws(() => decodeResponse(bytes('HTTP/1.1 200 OK\r\ncontent-type: x\r\n')), /truncated/);
});

test('should reject a chunked body cut off mid-chunk', () => {
  const raw = bytes('HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n', 'a\r\n01234');
  assert.throws(() => decodeResponse(raw), /truncated/);
});
