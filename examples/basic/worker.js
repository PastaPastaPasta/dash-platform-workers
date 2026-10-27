// A small JSON API over Dash Platform. Reads are public; /transfer and /rpc need WRITE_TOKEN.
import { DashPlatform } from '../../src/index.js';

const json = (body, status = 200) =>
  new Response(JSON.stringify(body, (_, v) => (typeof v === 'bigint' ? v.toString() : v)), {
    status,
    headers: { 'content-type': 'application/json' },
  });

const routes = [
  ['GET', /^\/identities\/(\w+)$/, (dash, [id]) => dash.identities.fetch(id)],
  ['GET', /^\/identities\/(\w+)\/balance$/, (dash, [id]) => dash.identities.balance(id)],
  ['GET', /^\/identities\/(\w+)\/names$/, (dash, [id]) => dash.dpns.usernames(id)],
  ['GET', /^\/names\/([\w.-]+)$/, (dash, [name]) => dash.dpns.resolveName(name)],
  ['GET', /^\/contracts\/(\w+)$/, (dash, [id]) => dash.contracts.fetch(id)],
  ['GET', /^\/contracts\/(\w+)\/documents\/(\w+)$/, (dash, [dataContractId, documentTypeName], q) =>
    dash.documents.query({
      dataContractId,
      documentTypeName,
      where: q.has('where') ? JSON.parse(q.get('where')) : undefined,
      orderBy: q.has('orderBy') ? JSON.parse(q.get('orderBy')) : undefined,
      limit: q.has('limit') ? Number(q.get('limit')) : undefined,
      startAfter: q.get('startAfter') ?? undefined,
    })],
  ['GET', /^\/identities\/(\w+)\/tokens$/, (dash, [id], q) =>
    dash.tokens.identityBalances(id, (q.get('ids') ?? '').split(',').filter(Boolean))],
  ['GET', /^\/epoch$/, (dash) => dash.epoch.current()],
  ['POST', /^\/transfer$/, async (dash, _, __, req, env) => {
    if (!authorized(req, env)) return json({ error: 'unauthorized' }, 401);
    const { recipientId, amount } = await req.json();
    return dash.identities.creditTransfer({
      identityId: env.IDENTITY_ID,
      privateKey: env.TRANSFER_KEY,
      recipientId,
      amount,
    });
  }],
  // Any SDK method by name, for trusted callers: {"method": "identities.fetch", "params": {...}}.
  ['POST', /^\/rpc$/, async (dash, _, __, req, env) => {
    if (!authorized(req, env)) return json({ error: 'unauthorized' }, 401);
    const { method, params } = await req.json();
    return dash.call(method, params);
  }],
];

const authorized = (req, env) =>
  Boolean(env.WRITE_TOKEN) && req.headers.get('authorization') === `Bearer ${env.WRITE_TOKEN}`;

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    for (const [method, pattern, handler] of routes) {
      const match = req.method === method && url.pathname.match(pattern);
      if (!match) continue;
      try {
        const dash = await DashPlatform.connect({ network: env.NETWORK });
        const result = await handler(dash, match.slice(1), url.searchParams, req, env);
        return result instanceof Response ? result : json(result);
      } catch (e) {
        return json({ error: e?.message ?? String(e) }, 502);
      }
    }
    return json({ error: 'not found' }, 404);
  },
};
