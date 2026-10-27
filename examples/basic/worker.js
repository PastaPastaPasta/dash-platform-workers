// A small JSON API over Dash Platform. Reads are public; POST /rpc runs any SDK method, writes
// included, for callers holding WRITE_TOKEN (unset: /rpc is refused).
import { DashPlatform } from '../../src/index.js';

const json = (body, status = 200) =>
  new Response(JSON.stringify(body, (_, v) => (typeof v === 'bigint' ? v.toString() : v)), {
    status,
    headers: { 'content-type': 'application/json' },
  });

class BadRequest extends Error {}

const param = (q, key, parse = (v) => v) => {
  if (!q.has(key)) return undefined;
  try {
    return parse(q.get(key));
  } catch {
    throw new BadRequest(`invalid ${key}`);
  }
};

const integer = (v) => {
  if (!/^\d+$/.test(v)) throw new Error('not an integer');
  return Number(v);
};

const routes = [
  ['GET', /^\/identities\/(\w+)$/, ({ dash, m }) => dash.identities.fetch(m[0])],
  ['GET', /^\/identities\/(\w+)\/balance$/, ({ dash, m }) => dash.identities.balance(m[0])],
  ['GET', /^\/identities\/(\w+)\/names$/, ({ dash, m }) => dash.dpns.usernames(m[0])],
  ['GET', /^\/identities\/(\w+)\/tokens$/, ({ dash, m, q }) =>
    dash.tokens.identityBalances(m[0], (q.get('ids') ?? '').split(',').filter(Boolean))],
  ['GET', /^\/names\/([\w.-]+)$/, ({ dash, m }) => dash.dpns.resolveName(m[0])],
  ['GET', /^\/contracts\/(\w+)$/, ({ dash, m }) => dash.contracts.fetch(m[0])],
  ['GET', /^\/contracts\/(\w+)\/documents\/(\w+)$/, ({ dash, m, q }) =>
    dash.documents.query({
      dataContractId: m[0],
      documentTypeName: m[1],
      where: param(q, 'where', JSON.parse),
      orderBy: param(q, 'orderBy', JSON.parse),
      limit: param(q, 'limit', integer),
      startAfter: param(q, 'startAfter'),
    })],
  ['GET', /^\/epoch$/, ({ dash }) => dash.epoch.current()],
  // {"method": "identities.fetch", "params": {...}}: any SDK method by name.
  ['POST', /^\/rpc$/, async ({ dash, req }) => {
    const { method, params } = await req.json().catch(() => { throw new BadRequest('body must be JSON'); });
    return dash.call(method, params);
  }, { write: true }],
];

const enc = new TextEncoder();

/** Bearer-token check in constant time. */
function authorized(req, env) {
  if (!env.WRITE_TOKEN) return false;
  const given = enc.encode(req.headers.get('authorization') ?? '');
  const expected = enc.encode(`Bearer ${env.WRITE_TOKEN}`);
  return given.byteLength === expected.byteLength && crypto.subtle.timingSafeEqual(given, expected);
}

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    for (const [method, pattern, handler, { write = false } = {}] of routes) {
      const match = req.method === method && url.pathname.match(pattern);
      if (!match) continue;
      if (write && !authorized(req, env)) return json({ error: 'unauthorized' }, 401);
      try {
        const dash = await DashPlatform.connect({ network: env.NETWORK });
        return json(await handler({ dash, m: match.slice(1), q: url.searchParams, req }));
      } catch (e) {
        return json({ error: e?.message ?? String(e) }, e instanceof BadRequest ? 400 : 502);
      }
    }
    return json({ error: 'not found' }, 404);
  },
};
