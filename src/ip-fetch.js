// Workers' fetch() refuses bare-IP URLs (error 1003), and evonodes are only reachable by IP.
// Their certificates carry the IP as a SAN, so a TLS socket to the IP verifies fine; this sends
// HTTPS requests to IPv4 hosts over such a socket as HTTP/1.1 and hands back an ordinary
// Response. Those requests skip fetch's redirects and `cf` options; everything else is untouched.
import { connect } from 'cloudflare:sockets';
import { encodeRequest, readResponse } from './http1.js';

const IPV4 = /^\d{1,3}(?:\.\d{1,3}){3}$/;
// The wasm build of the DAPI client enforces no deadline itself, so a node that accepts the
// connection and then stalls would hang the request. Unary calls get the deadline the client
// put in `grpc-timeout` plus a margin (DAPI itself waits up to 30 s for a state transition's
// result). Streams (DAPI's subscribe* methods) only get a deadline for their response head,
// then stay open until the caller drops them.
const DEFAULT_TIMEOUT_MS = 35_000;
const MARGIN_MS = 5_000;
const STREAM_HEAD_TIMEOUT_MS = 10_000;
const GRPC_UNITS_MS = { H: 3_600_000, M: 60_000, S: 1_000, m: 1, u: 0.001, n: 0.000001 };

function deadline(req) {
  const match = /^(\d{1,8})([HMSmun])$/.exec(req.headers.get('grpc-timeout') ?? '');
  return match ? Number(match[1]) * GRPC_UNITS_MS[match[2]] + MARGIN_MS : DEFAULT_TIMEOUT_MS;
}

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
  const streaming = /\/subscribe/i.test(url.pathname);
  timer = setTimeout(close, streaming ? STREAM_HEAD_TIMEOUT_MS : deadline(req));
  try {
    const writer = socket.writable.getWriter();
    await writer.write(encodeRequest(req, url, body));
    writer.releaseLock();
    const response = await readResponse(socket.readable, close);
    if (streaming) clearTimeout(timer);
    return response;
  } catch (e) {
    close();
    throw e;
  }
}
