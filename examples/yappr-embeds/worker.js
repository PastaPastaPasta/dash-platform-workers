// Rich link previews for Yappr. Yappr is a static app that loads posts in the browser, so the
// crawlers behind Discord, Slack, X, Telegram and friends see a generic page. In front of the
// site, this answers those crawlers with the post's author, text and image as Open Graph tags,
// read from Platform with proofs. Everyone else gets the site untouched.
import { DashPlatform } from '../../src/index.js';

const PREVIEW_BOTS =
  /discordbot|slackbot|slack-imgproxy|twitterbot|facebookexternalhit|facebot|telegrambot|whatsapp|linkedinbot|redditbot|mastodon|bluesky|cardyb|skypeuripreview|embedly|iframely|pinterest|vkshare|zulip|mattermost|google-inspectiontool/i;
const IDENTIFIER = /^[1-9A-HJ-NP-Za-km-z]{42,44}$/;
const CACHE_SECONDS = 300;
const MAX_DESCRIPTION = 300;

export default {
  async fetch(req, env, ctx) {
    const url = new URL(req.url);
    // Deployed on Yappr's own route, the origin is the same URL; elsewhere ORIGIN names it.
    const originUrl = env.ORIGIN ? new URL(url.pathname + url.search, env.ORIGIN) : url;
    const page = () => fetch(new Request(originUrl, req));

    const kind = { '/post/': 'post', '/user/': 'user' }[url.pathname];
    const id = url.searchParams.get('id');
    const forced = url.searchParams.has('preview');
    if (!kind || !IDENTIFIER.test(id ?? '') || !(forced || PREVIEW_BOTS.test(req.headers.get('user-agent') ?? ''))) {
      return page();
    }

    const cacheKey = new Request(`${url.origin}${url.pathname}?id=${encodeURIComponent(id)}&preview`);
    const cached = await caches.default.match(cacheKey);
    if (cached) return cached;

    const [html, card] = await Promise.all([page(), cardFor(kind, id, env).catch(() => null)]);
    // Any failure to read Platform leaves the page as it was rather than breaking the link.
    if (!card || !html.ok) return html;

    const canonical = new URL(`${url.pathname}?id=${encodeURIComponent(id)}`, env.CANONICAL_ORIGIN ?? url.origin);
    const tagged = withCard(html, card, canonical.href);
    const response = new Response(tagged.body, tagged);
    response.headers.set('cache-control', `public, max-age=${CACHE_SECONDS}`);
    ctx.waitUntil(caches.default.put(cacheKey, response.clone()));
    return response;
  },
};

async function cardFor(kind, id, env) {
  const dash = await DashPlatform.connect({ network: env.NETWORK });
  if (kind === 'user') {
    const who = await author(dash, env, id);
    if (!who.known) return null;
    return { title: who.title, description: who.bio || 'On Yappr', image: who.avatar, large: false };
  }
  const doc =
    (await dash.documents.get(env.SOCIAL_CONTRACT, 'post', id)) ??
    (await dash.documents.get(env.SOCIAL_CONTRACT, 'reply', id));
  if (!doc) return null;
  const who = await author(dash, env, doc.$ownerId);
  const media = httpsUrl(doc.mediaUrl, env);
  return {
    title: doc.parentId ? `${who.title} replied` : who.title,
    description: postText(doc),
    image: media && !doc.sensitive ? media : who.avatar,
    large: Boolean(media && !doc.sensitive),
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

/** The post's text as a preview can show it: never the content of private or sensitive posts. */
function postText(doc) {
  if (doc.encryptedContent) return '🔒 Private post';
  if (doc.sensitive) return 'Sensitive content. Open the post on Yappr to view it.';
  const text = (doc.content ?? '').replace(/\s+/g, ' ').trim();
  return text.length > MAX_DESCRIPTION ? `${text.slice(0, MAX_DESCRIPTION - 1)}…` : text || 'A post on Yappr';
}

/**
 * Yappr stores avatars as DiceBear settings (`{"seed", "style"}`) and draws them in the browser;
 * DiceBear's API draws the same one as a PNG. A plain URL is used as given.
 */
function avatarUrl(avatar, identityId, env) {
  let seed = identityId;
  let style = 'thumbs';
  if (avatar) {
    const direct = httpsUrl(avatar, env);
    if (direct) return direct;
    try {
      ({ seed = seed, style = style } = JSON.parse(avatar));
    } catch {
      // Not JSON either: fall back to the identity's default avatar.
    }
  }
  return `https://api.dicebear.com/9.x/${encodeURIComponent(style)}/png?seed=${encodeURIComponent(seed)}&size=256`;
}

/** An https URL for `value`, mapping ipfs:// through the configured gateway, or null. */
function httpsUrl(value, env) {
  if (!value) return null;
  if (value.startsWith('ipfs://')) return `${env.IPFS_GATEWAY ?? 'https://ipfs.io/ipfs/'}${value.slice(7)}`;
  return /^https:\/\//.test(value) ? value : null;
}

const escapeHtml = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/** `html` with its title, description and any Open Graph or Twitter tags replaced by `card`. */
function withCard(html, card, canonical) {
  const meta = [
    ['property', 'og:site_name', 'Yappr'],
    ['property', 'og:type', 'article'],
    ['property', 'og:url', canonical],
    ['property', 'og:title', card.title],
    ['property', 'og:description', card.description],
    ['property', 'og:image', card.image],
    ['name', 'twitter:card', card.large ? 'summary_large_image' : 'summary'],
    ['name', 'twitter:title', card.title],
    ['name', 'twitter:description', card.description],
    ['name', 'twitter:image', card.image],
    ['name', 'theme-color', '#0ea5e9'],
    ...(card.published ? [['property', 'article:published_time', new Date(card.published).toISOString()]] : []),
  ]
    .map(([attr, key, value]) => `<meta ${attr}="${key}" content="${escapeHtml(value)}">`)
    .join('');
  return new HTMLRewriter()
    .on('meta[property^="og:"], meta[name^="twitter:"], meta[name="theme-color"]', { element: (e) => e.remove() })
    .on('meta[name="description"]', { element: (e) => e.setAttribute('content', card.description) })
    .on('title', { element: (e) => e.setInnerContent(`${card.title} on Yappr`) })
    .on('head', { element: (e) => e.append(meta, { html: true }) })
    .transform(html);
}
