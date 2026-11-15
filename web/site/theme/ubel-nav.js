// web/site/theme/ubel-nav.js
//
// Runs on every docs page. mdBook's menu bar comes from its own built-in
// template, which book.toml has no hook to add arbitrary markup into; the
// only supported extension points are additional-css and additional-js. So
// this script adds, at runtime:
//
//   * a small site nav (Home / Docs / Playground / CI Results) next to the
//     book title, so the sections read as one site;
//   * a documentation VERSION PICKER and, when the page is not the latest
//     release, a banner saying so (see "Versioned docs" below);
//   * the shared Ubel highlighter and the reader-customisable syntax colours
//     (/shared/ubel-highlight.js, /shared/ubel-syntax-theme.js);
//   * the Ubel favicon in place of mdBook's own.
//
// ── Versioned docs ──────────────────────────────────────────────────────
// The deploy workflow (.github/workflows/pipeline-dashboard.yml) builds this
// book once per documentation version and lays them out like this:
//
//   /docs/next/        built from master: the unreleased, in-development docs
//   /docs/vX.Y.Z/      built from the git tag vX.Y.Z (one per release)
//   /docs/latest/      a copy of the newest release, or of next while no
//                      release exists yet
//   /docs/versions.json   what exists (read below)
//   /docs/index.html   redirects to /docs/latest/
//
// versions.json: { "schema": 1, "latest": "v0.1.0" | null,
//                  "versions": [ { "id": "v0.1.0", "date": "2026-12-01" }, ... ] }
// newest first; "next" is always present and is not listed in "versions".
// Nothing here needs a release to exist: with "latest": null the picker shows
// only the development docs and the banner says no release has shipped.
//
// ── Why /shared/... is an absolute path ─────────────────────────────────
// web/shared/ lives outside this book's source tree and mdBook only copies
// additional-js files from inside it, so the shared scripts are loaded by
// absolute URL. The deploy workflow copies web/shared/ to dist/shared/, a
// sibling of docs/, playground/ and results/.
(function () {
  'use strict';

  var LINKS = [
    { href: '/',            label: 'Home' },
    { href: '/docs/',       label: 'Docs' },
    { href: '/playground/', label: 'Playground' },
    { href: '/results/',    label: 'CI Results' }
  ];

  var DOCS_PATH = /^\/docs\/([^\/]+)\//;

  function isCurrent(href) {
    var path = window.location.pathname;
    if (href === '/') return path === '/' || path === '/index.html';
    return path.indexOf(href) === 0;
  }

  function loadScriptOnce(src, marker) {
    if (document.querySelector('script[' + marker + ']')) return;
    var script = document.createElement('script');
    script.src = src;
    script.setAttribute(marker, 'true');
    document.body.appendChild(script);
  }

  function injectFavicon() {
    // mdBook always emits its own favicon <link> tags with a hashed
    // filename baked in at build time; the only way to show the Ubel mark
    // is to remove those and add ours, not just add-if-absent.
    document.querySelectorAll('link[rel="icon"], link[rel="shortcut icon"]')
      .forEach(function (el) { el.remove(); });
    var link = document.createElement('link');
    link.rel = 'icon';
    link.type = 'image/png';
    link.href = '/shared/favicon.png';
    document.head.appendChild(link);
  }

  function injectNav() {
    var title = document.querySelector('.menu-title');
    if (!title || document.querySelector('.ubel-nav-links')) return null;

    var nav = document.createElement('div');
    nav.className = 'ubel-nav-links';
    nav.innerHTML = LINKS.map(function (l) {
      var cls = isCurrent(l.href) ? ' class="ubel-nav-current"' : '';
      return '<a href="' + l.href + '"' + cls + '>' + l.label + '</a>';
    }).join('');
    title.insertAdjacentElement('afterend', nav);
    return nav;
  }

  // ── Version picker ────────────────────────────────────────────────────
  function pageIn(versionId) {
    return window.location.pathname.replace(DOCS_PATH, '/docs/' + versionId + '/') +
      window.location.search + window.location.hash;
  }

  function goToVersion(versionId) {
    var target = pageIn(versionId);
    // The same page may not exist in another version (a chapter added or
    // renamed between releases); fall back to that version's front page
    // rather than a 404.
    fetch(target.split('#')[0], { method: 'HEAD' }).then(function (res) {
      window.location.href = res.ok ? target : '/docs/' + versionId + '/';
    }).catch(function () {
      window.location.href = '/docs/' + versionId + '/';
    });
  }

  function injectVersionUi(manifest, navEl) {
    var m = window.location.pathname.match(DOCS_PATH);
    if (!m || !manifest) return;
    var shown = m[1];                       // what the URL says
    var latest = manifest.latest || null;   // newest release id, or null
    // /docs/latest/ is a copy; for the picker it means "the newest release",
    // or "next" while none has shipped.
    var effective = shown === 'latest' ? (latest || 'next') : shown;

    var select = document.createElement('select');
    select.className = 'ubel-version-select';
    select.setAttribute('aria-label', 'Documentation version');
    var options = [{ id: 'next', label: 'Development (next)' }];
    (manifest.versions || []).forEach(function (v) {
      options.push({ id: v.id, label: v.id + (v.id === latest ? ' (latest)' : '') });
    });
    options.forEach(function (o) {
      var opt = document.createElement('option');
      opt.value = o.id;
      opt.textContent = o.label;
      if (o.id === effective) opt.selected = true;
      select.appendChild(opt);
    });
    select.addEventListener('change', function () { goToVersion(select.value); });
    if (navEl) navEl.appendChild(select);

    // Banner
    var main = document.querySelector('#mdbook-content main') || document.querySelector('main');
    if (!main || document.querySelector('.ubel-version-banner')) return;
    var text = null;
    if (effective === 'next') {
      text = latest
        ? 'This is the <strong>development</strong> documentation: it describes unreleased changes and can differ from the current release. ' +
          '<a href="' + pageIn(latest) + '">View this page in ' + latest + '</a>.'
        : 'This is the <strong>development</strong> documentation. No stable version of Ubel Stratum has been released yet, so these pages describe the language as it is on the <code>master</code> branch and can change at any time.';
    } else if (latest && effective !== latest) {
      text = 'You are reading the documentation for <strong>' + effective + '</strong>. ' +
        '<a href="' + pageIn(latest) + '">The latest release is ' + latest + '</a>.';
    }
    if (text) {
      var banner = document.createElement('div');
      banner.className = 'ubel-version-banner';
      banner.innerHTML = text;
      main.insertBefore(banner, main.firstChild);
    }
  }

  function loadManifest(navEl) {
    if (!DOCS_PATH.test(window.location.pathname)) return;
    fetch('/docs/versions.json', { cache: 'no-cache' })
      .then(function (res) { return res.ok ? res.json() : null; })
      .then(function (manifest) { injectVersionUi(manifest, navEl); })
      .catch(function () { /* local preview or offline: no picker, nothing breaks */ });
  }

  function init() {
    injectFavicon();
    var navEl = injectNav();
    loadScriptOnce('/shared/ubel-syntax-theme.js', 'data-ubel-syntax-theme');
    loadScriptOnce('/shared/ubel-highlight.js', 'data-ubel-highlight');
    loadManifest(navEl);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
