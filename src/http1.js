// Minimal HTTP/1.1 client framing for one request per connection (`Connection: close`).
// Responses stream, so DAPI's server-streaming calls deliver each message as it arrives.
const enc = new TextEncoder();
const dec = new TextDecoder();
const CRLF = enc.encode('\r\n');
const HEADER_END = enc.encode('\r\n\r\n');
const SKIPPED_HEADERS = new Set(['host', 'content-length', 'connection', 'transfer-encoding']);
const MAX_HEAD = 64 * 1024;
const MAX_CHUNK = 16 * 1024 * 1024;

/** Serializes `req` (whose body is already read into `body`) for a socket. */
export function encodeRequest(req, url, body) {
  let head = `${req.method} ${url.pathname}${url.search} HTTP/1.1\r\nHost: ${url.host}\r\n`;
  for (const [name, value] of req.headers) {
    if (!SKIPPED_HEADERS.has(name)) head += `${name}: ${value}\r\n`;
  }
  head += `Content-Length: ${body.length}\r\nConnection: close\r\n\r\n`;
  const headBytes = enc.encode(head);
  const out = new Uint8Array(headBytes.length + body.length);
  out.set(headBytes);
  out.set(body, headBytes.length);
  return out;
}

/** Bytes read from the socket but not yet consumed, grown by doubling so reads stay linear. */
class ByteQueue {
  constructor(reader) {
    this.reader = reader;
    this.buf = new Uint8Array(4096);
    this.start = 0;
    this.end = 0;
  }

  get length() { return this.end - this.start; }

  view() { return this.buf.subarray(this.start, this.end); }

  /** Reads once more from the socket; false at EOF. */
  async fill() {
    const { value, done } = await this.reader.read();
    if (done) return false;
    if (this.end + value.length > this.buf.length) {
      if (this.length + value.length <= this.buf.length) {
        // Enough room once consumed bytes are dropped: slide the unread ones to the front.
        this.buf.copyWithin(0, this.start, this.end);
      } else {
        const next = new Uint8Array(Math.max(this.length + value.length, this.buf.length * 2));
        next.set(this.view());
        this.buf = next;
      }
      this.end = this.length;
      this.start = 0;
    }
    this.buf.set(value, this.end);
    this.end += value.length;
    return true;
  }

  /** Removes and returns the first `n` bytes (a copy, since the buffer is reused). */
  take(n) {
    const out = this.buf.slice(this.start, this.start + n);
    this.start += n;
    return out;
  }

  /** Index of `needle` in the unconsumed bytes, or -1. */
  find(needle) {
    const view = this.view();
    outer: for (let i = 0; i <= view.length - needle.length; i++) {
      for (let j = 0; j < needle.length; j++) if (view[i + j] !== needle[j]) continue outer;
      return i;
    }
    return -1;
  }
}

/**
 * Reads a response's head from `readable` and returns a Response whose body streams the rest.
 * `release` runs once the body is finished, failed or cancelled, so the socket can close.
 */
export async function readResponse(readable, release = () => {}) {
  const reader = readable.getReader();
  try {
    const queue = new ByteQueue(reader);
    let end;
    while ((end = queue.find(HEADER_END)) < 0) {
      if (queue.length > MAX_HEAD) throw new Error('HTTP response head too large');
      if (!(await queue.fill())) throw new Error('truncated HTTP response');
    }
    const [statusLine, ...lines] = dec.decode(queue.take(end)).split('\r\n');
    queue.take(HEADER_END.length);
    const [version, code] = statusLine.split(' ');
    const status = Number(code);
    if (!/^HTTP\/1\.[01]$/.test(version) || !Number.isInteger(status)) {
      throw new Error(`malformed status line: ${statusLine}`);
    }
    const headers = new Headers();
    for (const line of lines) {
      const colon = line.indexOf(':');
      if (colon > 0) headers.append(line.slice(0, colon).trim(), line.slice(colon + 1).trim());
    }
    const chunked = /chunked/i.test(headers.get('transfer-encoding') ?? '');
    headers.delete('transfer-encoding');
    let length;
    if (!chunked && headers.has('content-length')) {
      const value = headers.get('content-length');
      if (!/^\d+$/.test(value)) throw new Error(`malformed content-length: ${value}`);
      length = Number(value);
    }
    const source = chunked ? chunkedBody(queue) : plainBody(queue, length);
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
    if (!released) {
      released = true;
      reader.cancel().catch(() => {});
      release();
    }
  };
  return new ReadableStream({
    async pull(controller) {
      try {
        const { value, done } = await source.next();
        if (done) {
          controller.close();
          finish();
        } else {
          controller.enqueue(value);
        }
      } catch (e) {
        controller.error(e);
        finish();
      }
    },
    cancel() { finish(); },
  });
}

/** A body delimited by Content-Length, or by the end of the connection without one. */
async function* plainBody(queue, length) {
  let remaining = length ?? Infinity;
  while (remaining > 0) {
    if (queue.length === 0 && !(await queue.fill())) {
      if (length !== undefined) throw new Error('truncated HTTP body');
      return;
    }
    const part = queue.take(Math.min(queue.length, remaining));
    remaining -= part.length;
    yield part;
  }
}

/** A chunked body, yielding each chunk's data as soon as it is complete. */
async function* chunkedBody(queue) {
  for (;;) {
    let lineEnd;
    while ((lineEnd = queue.find(CRLF)) < 0) {
      if (queue.length > 1024) throw new Error('malformed chunk size');
      if (!(await queue.fill())) throw new Error('truncated chunked body');
    }
    const line = dec.decode(queue.take(lineEnd));
    queue.take(CRLF.length);
    const sizeText = line.split(';')[0].trim();
    if (!/^[0-9a-f]+$/i.test(sizeText)) throw new Error(`malformed chunk size: ${line}`);
    const size = parseInt(sizeText, 16);
    if (size > MAX_CHUNK) throw new Error(`chunk of ${size} bytes is too large`);
    if (size === 0) return;
    while (queue.length < size + CRLF.length) {
      if (!(await queue.fill())) throw new Error('truncated chunked body');
    }
    yield queue.take(size);
    queue.take(CRLF.length);
  }
}
