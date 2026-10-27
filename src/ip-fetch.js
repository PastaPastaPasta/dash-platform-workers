// Workers' fetch() refuses bare-IP URLs (error 1003), and evonodes are only reachable by IP.
// Their certificates carry the IP as a SAN, so a TLS socket to the IP verifies fine; this sends
// HTTPS requests to IPv4 hosts over such a socket as HTTP/1.1 and hands back an ordinary
// Response. Those requests skip fetch's redirects and `cf` options; everything else is untouched.
import { connect } from 'cloudflare:sockets';
import { encodeRequest, readResponse } from './http1.js';

const IPV4 = /^\d{1,3}(?:\.\d{1,3}){3}$/;
// The wasm build of the DAPI client sets no deadline of its own, so a node that accepts the
// connection and then stalls would hang the request; unary calls give up after this. Streams
// (DAPI's subscribe* methods) stay open until the caller drops them.
const UNARY_TIMEOUT_MS = 15_000;

const nativeFetch = globalThis.fetch;
globalThis.fetch = (input, init) => {
  const req = new Request(input, init);
  const url = new URL(req.url);
  return url.protocol === 'https:' && IPV4.test(url.hostname) ? socketFetch(req, url) : nativeFetch(req);
};

async function socketFetch(req, url) {
  if (req.signal?.aborted) throw req.signal.reason;
  const body = new Uint8Array(await req.arrayBuffer());
  const socket = connect({ hostname: url.hostname, port: Number(url.port || 443) }, { secureTransport: 'on' });
  let timer;
  const close = () => {
    clearTimeout(timer);
    req.signal?.removeEventListener('abort', close);
    socket.close().catch(() => {});
  };
  req.signal?.addEventListener('abort', close);
  if (!/\/subscribe/i.test(url.pathname)) timer = setTimeout(close, UNARY_TIMEOUT_MS);
  try {
    const writer = socket.writable.getWriter();
    await writer.write(encodeRequest(req, url, body));
    writer.releaseLock();
    return await readResponse(socket.readable, close);
  } catch (e) {
    close();
    throw e;
  }
}
