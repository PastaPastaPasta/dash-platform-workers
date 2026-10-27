// Rich link previews for Yappr. Yappr is a static app that loads posts in the browser, so the
// crawlers behind Discord, Slack, X and Telegram see one generic page for every post. Routed in
// front of the site, this answers those crawlers with the post's own Open Graph tags; the card
// comes from the `yappr-cards` Worker, which reads Platform. Everyone else, and every failure,
// gets the site untouched. This Worker loads no SDK, so it adds almost nothing to a page view.

const PREVIEW_BOTS =
  /discordbot|slackbot|slack-imgproxy|twitterbot|facebookexternalhit|facebot|telegrambot|whatsapp|linkedinbot|redditbot|mastodon|misskey|bluesky|cardyb|matrix|synapse|kakaotalk-scrap|viber|skypeuripreview|embedly|iframely|pinterest|vkshare|zulip|mattermost/i;
const IDENTIFIER = /^[1-9A-HJ-NP-Za-km-z]{42,44}$/;
// Yappr shares `/post?id=`, which GitHub Pages redirects to `/post/?id=`; both are handled.
const KINDS = { '/post': 'post', '/post/': 'post', '/user': 'user', '/user/': 'user' };
// The shell is fetched fresh for a crawler: ranges and revalidation would reach the origin and
// come back as 206 or 304 bodies that can't carry the tags.
const DROPPED_HEADERS = ['range', 'if-range', 'if-none-match', 'if-modified-since'];
const CACHE_SECONDS = 300;

export default {
  async fetch(req, env, ctx) {
    ctx.passThroughOnException();
    const url = new URL(req.url);
    const target = originUrl(url, env);
    const kind = KINDS[url.pathname];
    const id = url.searchParams.get('id') ?? '';
    const wantsCard =
      req.method === 'GET' &&
      kind &&
      IDENTIFIER.test(id) &&
      (url.searchParams.has('preview') || PREVIEW_BOTS.test(req.headers.get('user-agent') ?? ''));
    if (!wantsCard) return fetch(new Request(target, req));

    const headers = new Headers(req.headers);
    for (const name of DROPPED_HEADERS) headers.delete(name);
    const [html, card] = await Promise.all([
      fetch(target, { headers, redirect: 'manual' }),
      env.CARDS.fetch(`https://cards/?kind=${kind}&id=${id}&contract=${env.SOCIAL_CONTRACT}`)
        .then((r) => (r.ok ? r.json() : null))
        .catch(() => null),
    ]);
    if (!card || html.status !== 200) return html;
    try {
      const canonical = new URL(`${url.pathname}?id=${id}`, env.CANONICAL_ORIGIN ?? url.origin);
      const tagged = withCard(html, card, canonical.href);
      const response = new Response(tagged.body, tagged);
      response.headers.delete('etag');
      response.headers.delete('last-modified');
      response.headers.set('cache-control', `public, max-age=${CACHE_SECONDS}`);
      return response;
    } catch {
      return fetch(new Request(target, req));
    }
  },
};

/**
 * Where the page really lives. Routed on Yappr's own zone the request URL is it (Cloudflare
 * sends a route Worker's same-zone fetches to the origin). With ORIGIN set, as on workers.dev,
 * only the path and query carry over, so no path can point the fetch at another host.
 */
function originUrl(url, env) {
  if (!env.ORIGIN) return url;
  const target = new URL(env.ORIGIN);
  target.pathname = url.pathname;
  target.search = url.search;
  return target;
}

const escapeHtml = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/** `html` with its title, description, Open Graph and Twitter tags replaced by `card`'s. */
function withCard(html, card, canonical) {
  const large = !card.avatar;
  const meta = [
    ['name', 'description', card.description],
    ['property', 'og:site_name', 'Yappr'],
    ['property', 'og:type', card.type],
    ['property', 'og:url', canonical],
    ['property', 'og:title', card.title],
    ['property', 'og:description', card.description],
    ['property', 'og:image', card.image],
    ['property', 'og:image:alt', card.imageAlt],
    ...(card.avatar ? [['property', 'og:image:width', '256'], ['property', 'og:image:height', '256']] : []),
    ['name', 'twitter:card', large ? 'summary_large_image' : 'summary'],
    ['name', 'twitter:title', card.title],
    ['name', 'twitter:description', card.description],
    ['name', 'twitter:image', card.image],
    ['name', 'theme-color', '#0ea5e9'],
    ...(card.published ? [['property', 'article:published_time', new Date(card.published).toISOString()]] : []),
  ]
    .map(([attr, key, value]) => `<meta ${attr}="${key}" content="${escapeHtml(value)}">`)
    .join('');
  return new HTMLRewriter()
    .on('meta[property^="og:"], meta[name^="twitter:"], meta[name="theme-color"], meta[name="description"]', {
      element: (e) => e.remove(),
    })
    .on('head > title', { element: (e) => e.setInnerContent(`${card.title} on Yappr`) })
    // Prepended, so a crawler reading only the start of the page still sees them.
    .on('head', { element: (e) => e.prepend(meta, { html: true }) })
    .transform(html);
}
