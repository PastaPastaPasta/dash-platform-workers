// Minimal HTTP/1.1 framing for one request per connection (`Connection: close`).
const enc = new TextEncoder();
const dec = new TextDecoder();
const CRLF = enc.encode('\r\n');
const HEADER_END = enc.encode('\r\n\r\n');

/** Serializes `req` (a Request whose body is already read into `body`) for a socket. */
export function encodeRequest(req, url, body) {
  let head = `${req.method} ${url.pathname}${url.search} HTTP/1.1\r\nHost: ${url.host}\r\n`;
  for (const [name, value] of req.headers) {
    if (name !== 'host' && name !== 'content-length' && name !== 'connection') head += `${name}: ${value}\r\n`;
  }
  head += `Content-Length: ${body.length}\r\nConnection: close\r\n\r\n`;
  return concat([enc.encode(head), body]);
}

/** Parses a complete response read to EOF into a Response. */
export function decodeResponse(raw) {
  const end = indexOf(raw, HEADER_END);
  if (end < 0) throw new Error('truncated HTTP response');
  const [statusLine, ...lines] = dec.decode(raw.subarray(0, end)).split('\r\n');
  const status = Number(statusLine.split(' ')[1]);
  if (!Number.isInteger(status)) throw new Error(`malformed status line: ${statusLine}`);
  const headers = new Headers();
  for (const line of lines) {
    const colon = line.indexOf(':');
    if (colon > 0) headers.append(line.slice(0, colon).trim(), line.slice(colon + 1).trim());
  }
  let body = raw.subarray(end + HEADER_END.length);
  if (/chunked/i.test(headers.get('transfer-encoding') ?? '')) {
    body = dechunk(body);
    headers.delete('transfer-encoding');
  } else if (headers.has('content-length')) {
    body = body.subarray(0, Number(headers.get('content-length')));
  }
  return new Response(body, { status, headers });
}

function dechunk(buf) {
  const parts = [];
  let pos = 0;
  for (;;) {
    const lineEnd = indexOf(buf, CRLF, pos);
    if (lineEnd < 0) throw new Error('truncated chunked body');
    const size = parseInt(dec.decode(buf.subarray(pos, lineEnd)), 16);
    if (Number.isNaN(size)) throw new Error('malformed chunk size');
    if (size === 0) return concat(parts);
    const start = lineEnd + CRLF.length;
    if (start + size > buf.length) throw new Error('truncated chunked body');
    parts.push(buf.subarray(start, start + size));
    pos = start + size + CRLF.length;
  }
}

function indexOf(buf, needle, from = 0) {
  outer: for (let i = from; i <= buf.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) if (buf[i + j] !== needle[j]) continue outer;
    return i;
  }
  return -1;
}

export function concat(parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}
