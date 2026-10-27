// Workers' fetch() refuses bare-IP URLs (error 1003), and evonodes are only reachable by IP.
// Their certificates carry the IP as a SAN, so a TLS socket to the IP verifies fine; this sends
// the SDK's gRPC-web POST over that socket as HTTP/1.1 and hands back an ordinary Response.
import { connect } from 'cloudflare:sockets';
import { concat, decodeResponse, encodeRequest } from './http1.js';

const IPV4 = /^\d{1,3}(?:\.\d{1,3}){3}$/;

let installed = false;

export function installIpFetch() {
  if (installed) return;
  installed = true;
  const nativeFetch = globalThis.fetch;
  globalThis.fetch = (input, init) => {
    const req = new Request(input, init);
    const url = new URL(req.url);
    return IPV4.test(url.hostname) ? socketFetch(req, url) : nativeFetch(req);
  };
}

async function socketFetch(req, url) {
  const body = new Uint8Array(await req.arrayBuffer());
  const socket = connect({ hostname: url.hostname, port: Number(url.port || 443) }, { secureTransport: 'on' });
  const abort = () => socket.close();
  req.signal?.addEventListener('abort', abort);
  try {
    const writer = socket.writable.getWriter();
    await writer.write(encodeRequest(req, url, body));
    writer.releaseLock();
    const chunks = [];
    for await (const chunk of socket.readable) chunks.push(chunk);
    return decodeResponse(concat(chunks));
  } finally {
    req.signal?.removeEventListener('abort', abort);
    socket.close().catch(() => {});
  }
}
