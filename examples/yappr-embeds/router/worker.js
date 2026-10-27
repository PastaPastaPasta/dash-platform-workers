// Rich link previews for Yappr. Yappr is a static app that loads posts in the browser, so the
// crawlers behind Discord, Slack, X and Telegram see one generic page for every post.
//
// A redirect rule on the site's zone sends only those crawlers from /post?id=… and /user?id=… to
// /_embed/post?id=… here, so people never touch a Worker. This answers with a small page carrying
// the post's Open Graph tags; the card comes from the `yappr-cards` Worker, which reads Platform.
// A person who lands here anyway is sent on to the post.

const IDENTIFIER = /^[1-9A-HJ-NP-Za-km-z]{42,44}$/;
const KINDS = { '/_embed/post': 'post', '/_embed/post/': 'post', '/_embed/user': 'user', '/_embed/user/': 'user' };
const CACHE_SECONDS = 300;

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    const kind = KINDS[url.pathname];
    const id = url.searchParams.get('id') ?? '';
    // The page the preview stands for, on the site itself.
    const site = env.SITE_ORIGIN ?? url.origin;
    const target = kind && IDENTIFIER.test(id) ? `${site}/${kind}/?id=${id}` : `${site}/`;
    if (!kind || !IDENTIFIER.test(id) || req.method !== 'GET') return Response.redirect(target, 302);

    const card = await env.CARDS.fetch(`https://cards/?kind=${kind}&id=${id}&contract=${env.SOCIAL_CONTRACT}`)
      .then((r) => (r.ok ? r.json() : null))
      .catch(() => null);
    return new Response(page(card, target), {
      headers: {
        'content-type': 'text/html; charset=utf-8',
        'cache-control': `public, max-age=${card ? CACHE_SECONDS : 60}`,
      },
    });
  },
};

const escapeHtml = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

/** A page with `card`'s tags (or Yappr's generic ones) that sends a browser on to `target`. */
function page(card, target) {
  const title = card ? `${card.title} on Yappr` : 'Yappr';
  const meta = card
    ? [
        ['name', 'description', card.description],
        ['property', 'og:site_name', 'Yappr'],
        ['property', 'og:type', card.type],
        ['property', 'og:url', target],
        ['property', 'og:title', card.title],
        ['property', 'og:description', card.description],
        ['property', 'og:image', card.image],
        ['property', 'og:image:alt', card.imageAlt],
        ...(card.avatar ? [['property', 'og:image:width', '256'], ['property', 'og:image:height', '256']] : []),
        ['name', 'twitter:card', card.avatar ? 'summary' : 'summary_large_image'],
        ['name', 'twitter:title', card.title],
        ['name', 'twitter:description', card.description],
        ['name', 'twitter:image', card.image],
        ['name', 'theme-color', '#0ea5e9'],
        ...(card.published ? [['property', 'article:published_time', new Date(card.published).toISOString()]] : []),
      ]
    : [
        ['name', 'description', 'A modern social platform for sharing thoughts and connecting with others'],
        ['property', 'og:site_name', 'Yappr'],
        ['property', 'og:title', 'Yappr'],
        ['property', 'og:url', target],
      ];
  const tags = meta.map(([attr, key, value]) => `<meta ${attr}="${key}" content="${escapeHtml(value)}">`).join('');
  const href = escapeHtml(target);
  return `<!doctype html><html><head><meta charset="utf-8"><title>${escapeHtml(title)}</title>${tags}` +
    `<link rel="canonical" href="${href}"><script>location.replace(${JSON.stringify(target).replace(/</g, '\\u003c')})</script>` +
    `</head><body><a href="${href}">Open on Yappr</a></body></html>`;
}
