// Card data for Yappr link previews: the author, text and image of a post or profile, read from
// Platform with proofs. Reached only through the router's service binding, never publicly.
//
// GET /?kind=post|user&id=<base58> → JSON card, or null when there is none to show.
import { DashPlatform } from '../../../src/index.js';

const IDENTIFIER = /^[1-9A-HJ-NP-Za-km-z]{42,44}$/;
// Answer within this; a slower lookup still finishes in the background and is cached.
const BUDGET_MS = 5_000;
const CARD_SECONDS = 300;
// Unknown ids are remembered too, so repeated misses cost no Platform reads.
const MISS_SECONDS = 60;
const MAX_DESCRIPTION = 300;
const DEFAULT_STYLE = 'thumbs';
// The DiceBear styles Yappr draws (lib/services/avatar-generator.ts); others fall back.
const STYLES = new Set([
  'adventurer', 'adventurer-neutral', 'avataaars', 'avataaars-neutral', 'big-ears', 'big-ears-neutral',
  'big-smile', 'bottts', 'bottts-neutral', 'croodles', 'croodles-neutral', 'fun-emoji', 'icons',
  'identicon', 'initials', 'lorelei', 'lorelei-neutral', 'micah', 'miniavs', 'notionists',
  'notionists-neutral', 'open-peeps', 'personas', 'pixel-art', 'pixel-art-neutral', 'rings',
  'shapes', 'thumbs',
]);

export default {
  async fetch(req, env, ctx) {
    const url = new URL(req.url);
    const kind = url.searchParams.get('kind');
    const id = url.searchParams.get('id') ?? '';
    if ((kind !== 'post' && kind !== 'user') || !IDENTIFIER.test(id)) {
      return Response.json(null, { status: 400 });
    }

    const key = new Request(`https://yappr-cards.internal/${kind}/${id}`);
    const cached = await caches.default.match(key);
    if (cached) return cached;

    const lookup = cardFor(kind, id, env).catch(() => null);
    ctx.waitUntil(
      lookup.then((card) =>
        caches.default.put(key, Response.json(card, {
          headers: { 'cache-control': `max-age=${card ? CARD_SECONDS : MISS_SECONDS}` },
        })),
      ),
    );
    const card = await Promise.race([lookup, new Promise((resolve) => setTimeout(resolve, BUDGET_MS, null))]);
    return Response.json(card);
  },
};

async function cardFor(kind, id, env) {
  const dash = await DashPlatform.connect({ network: env.NETWORK });
  if (kind === 'user') {
    const who = await author(dash, env, id);
    if (!who.known) return null;
    return {
      type: 'profile',
      title: who.title,
      description: who.bio || 'On Yappr',
      image: who.avatar,
      imageAlt: `${who.title}'s avatar`,
      avatar: true,
    };
  }
  const [post, reply] = await Promise.all([
    dash.documents.get(env.SOCIAL_CONTRACT, 'post', id),
    dash.documents.get(env.SOCIAL_CONTRACT, 'reply', id),
  ]);
  const doc = post ?? reply;
  if (!doc) return null;
  const who = await author(dash, env, doc.$ownerId);
  // Media of private or sensitive posts is not pushed into chats; the author's avatar stands in.
  const media = doc.sensitive || doc.encryptedContent ? null : directUrl(doc.mediaUrl, env);
  return {
    type: 'article',
    title: reply ? `${who.title} replied` : who.title,
    description: postText(doc),
    image: media ?? who.avatar,
    imageAlt: media ? 'Image attached to the post' : `${who.title}'s avatar`,
    avatar: !media,
    published: doc.$createdAt,
  };
}

/** Display name, username and avatar of an identity, each falling back when absent. */
async function author(dash, env, identityId) {
  const [names, profiles] = await Promise.all([
    dash.dpns.usernames(identityId, { limit: 1 }).catch(() => []),
    dash.documents
      .query({
        dataContractId: env.PROFILE_CONTRACT,
        documentTypeName: 'profile',
        where: [['$ownerId', '==', identityId]],
        limit: 1,
      })
      .catch(() => []),
  ]);
  const username = names[0]?.replace(/\.dash$/, '');
  const profile = profiles[0];
  const name = profile?.displayName || username || `${identityId.slice(0, 8)}…`;
  return {
    title: username && username !== name ? `${name} (@${username})` : name,
    bio: profile?.bio,
    avatar: avatarUrl(profile?.avatar, identityId, env),
    known: Boolean(profile || username),
  };
}

/** The post's text as a preview may show it: never the content of private or sensitive posts. */
function postText(doc) {
  if (doc.encryptedContent) return '🔒 Private post';
  if (doc.sensitive) return 'Sensitive content. Open the post on Yappr to view it.';
  const text = Array.from((doc.content ?? '').replace(/\s+/g, ' ').trim());
  if (!text.length) return 'A post on Yappr';
  return text.length > MAX_DESCRIPTION ? `${text.slice(0, MAX_DESCRIPTION - 1).join('')}…` : text.join('');
}

/** The avatar Yappr would draw (its parseAvatarField), as a PNG DiceBear renders. */
function avatarUrl(field, identityId, env) {
  const draw = (style, seed) =>
    `https://api.dicebear.com/9.x/${STYLES.has(style) ? style : DEFAULT_STYLE}/png?seed=${encodeURIComponent(seed)}&size=256`;
  if (!field) return draw(DEFAULT_STYLE, identityId);
  if (/^(https?|ipfs):\/\//.test(field)) return directUrl(field, env) ?? draw(DEFAULT_STYLE, identityId);
  try {
    const { style, seed } = JSON.parse(field);
    if (style && seed) return draw(String(style), String(seed));
  } catch {
    // Not JSON: Yappr treats the field itself as the seed.
  }
  return draw(DEFAULT_STYLE, field);
}

/** An https URL for `value`, mapping ipfs:// through the configured gateway, or null. */
function directUrl(value, env) {
  if (!value) return null;
  if (value.startsWith('ipfs://')) return `${env.IPFS_GATEWAY ?? 'https://ipfs.io/ipfs/'}${value.slice(7)}`;
  return /^https:\/\//.test(value) ? value : null;
}
