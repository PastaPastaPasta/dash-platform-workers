// Minimal HTTP/1.1 client framing for one request per connection (`Connection: close`).
// Responses stream, so DAPI's server-streaming calls deliver each message as it arrives.
const enc = new TextEncoder();
const dec = new TextDecoder();
const CRLF = enc.encode('\r\n');
const HEADER_END = enc.encode('\r\n\r\n');
const SKIPPED_HEADERS = new Set(['host', 'content-length', 'connection', 'transfer-encoding']);

/** Serializes `req` (whose body is already read into `body`) for a socket. */
export function encodeRequest(req, url, body) {
  let head = `${req.method} ${url.pathname}${url.search} HTTP/1.1\r\nHost: ${url.host}\r\n`;
  for (const [name, value] of req.headers) {
    if (!SKIPPED_HEADERS.has(name)) head += `${name}: ${value}\r\n`;
  }
  head += `Content-Length: ${body.length}\r\nConnection: close\r\n\r\n`;
  return concat([enc.encode(head), body]);
}

/**
 * Reads a response's head from `readable` and returns a Response whose body streams the rest.
 * `release` runs once the body is finished, failed or cancelled, so the socket can close.
 */
export async function readResponse(readable, release = () => {}) {
  const reader = readable.getReader();
  try {
    let buf = new Uint8Array(0);
    let end;
    while ((end = indexOf(buf, HEADER_END)) < 0) {
      const { value, done } = await reader.read();
      if (done) throw new Error('truncated HTTP response');
      buf = concat([buf, value]);
    }
    const [statusLine, ...lines] = dec.decode(buf.subarray(0, end)).split('\r\n');
    const status = Number(statusLine.split(' ')[1]);
    if (!/^HTTP\/1\.[01]$/.test(statusLine.split(' ')[0]) || !Number.isInteger(status)) {
      throw new Error(`malformed status line: ${statusLine}`);
    }
    const headers = new Headers();
    for (const line of lines) {
      const colon = line.indexOf(':');
      if (colon > 0) headers.append(line.slice(0, colon).trim(), line.slice(colon + 1).trim());
    }
    const rest = buf.subarray(end + HEADER_END.length);
    const chunked = /chunked/i.test(headers.get('transfer-encoding') ?? '');
    headers.delete('transfer-encoding');
    const length = headers.has('content-length') ? Number(headers.get('content-length')) : undefined;
    const source = chunked ? chunkedBody(reader, rest) : plainBody(reader, rest, length);
    return new Response(stream(source, reader, release), { status, headers });
  } catch (e) {
    reader.cancel().catch(() => {});
    release();
    throw e;
  }
}

/** Adapts an async generator of byte chunks into a ReadableStream that releases the socket. */
function stream(source, reader, release) {
  let released = false;
  const finish = () => {
    if (!released) { released = true; reader.cancel().catch(() => {}); release(); }
  };
  return new ReadableStream({
    async pull(controller) {
      try {
        const { value, done } = await source.next();
        if (done) { controller.close(); finish(); } else controller.enqueue(value);
      } catch (e) {
        controller.error(e);
        finish();
      }
    },
    cancel() { finish(); },
  });
}

/** A body delimited by Content-Length, or by the end of the connection without one. */
async function* plainBody(reader, rest, length) {
  let remaining = length ?? Infinity;
  if (rest.length) {
    const part = rest.subarray(0, Math.min(rest.length, remaining));
    remaining -= part.length;
    if (part.length) yield part;
  }
  while (remaining > 0) {
    const { value, done } = await reader.read();
    if (done) {
      if (length !== undefined) throw new Error('truncated HTTP body');
      return;
    }
    const part = value.subarray(0, Math.min(value.length, remaining));
    remaining -= part.length;
    if (part.length) yield part;
  }
}

/** A chunked body, yielding each chunk's data as soon as it is complete. */
async function* chunkedBody(reader, rest) {
  let buf = rest;
  const fill = async () => {
    const { value, done } = await reader.read();
    if (done) throw new Error('truncated chunked body');
    buf = concat([buf, value]);
  };
  for (;;) {
    let lineEnd;
    while ((lineEnd = indexOf(buf, CRLF)) < 0) await fill();
    const size = parseInt(dec.decode(buf.subarray(0, lineEnd)), 16);
    if (Number.isNaN(size)) throw new Error('malformed chunk size');
    if (size === 0) return;
    const start = lineEnd + CRLF.length;
    while (buf.length < start + size + CRLF.length) await fill();
    yield buf.slice(start, start + size);
    buf = buf.subarray(start + size + CRLF.length);
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
