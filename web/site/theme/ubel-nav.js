// web/site/theme/ubel-nav.js
//
// mdBook's menu bar (title + icon buttons) comes from its own built-in
// index.hbs template, which book.toml has no hook to add arbitrary links
// into; the only supported extension points are additional-css and
// additional-js. This runs on every page and inserts a small nav
// (Home / Docs / Playground / CI Results) between the book title and
// mdBook's own icon buttons, so the four sections of the site read as
// one property instead of unrelated pages. The GitHub icon itself is
// unaffected; that one mdBook already adds on its own via book.toml's
// git-repository-url.
//
// The docs site now lives under /docs/ rather than site root (site root
// is the standalone landing page in web/home/), so "current" can no
// longer be hardcoded to the Docs link the way it was when this book was
// the only thing living behind "/". Detected from location.pathname
// instead, which also means this same nav markup can be reused verbatim
// by the landing/playground/dashboard pages without each one having to
// hand-pick which link gets the "current" class.
//
// This file also loads the shared Ubel syntax highlighter from
// /shared/ubel-highlight.js (an absolute, site-root-relative path) rather
// than book.toml referencing it directly via additional-js: that file
// lives in web/shared/, outside this book's own source tree, and mdBook
// only copies/rewrites additional-js paths that live inside the book
// source. A path like ../shared/ubel-highlight.js would resolve correctly
// in this local checkout but not once book/ becomes docs/ under the
// deployed site root, since ../ from docs/ has nowhere further to go once
// docs/ itself is one level under root. The deploy workflow copies
// web/shared/ to dist/shared/ as a sibling of docs/, playground/, and
// results/, which is what makes the absolute path resolve correctly once
// live, regardless of how deep the current page is nested.
(function () {
  var LINKS = [
    { href: '/',            label: 'Home' },
    { href: '/docs/',       label: 'Docs' },
    { href: '/playground/', label: 'Playground' },
    { href: '/results/',    label: 'CI Results' }
  ];

  function isCurrent(href) {
    var path = window.location.pathname;
    if (href === '/') return path === '/' || path === '/index.html';
    return path.indexOf(href) === 0;
  }

  function loadSharedHighlighter() {
    if (document.querySelector('script[data-ubel-highlight]')) return;
    var script = document.createElement('script');
    script.src = '/shared/ubel-highlight.js';
    script.setAttribute('data-ubel-highlight', 'true');
    document.body.appendChild(script);
  }

  function injectFavicon() {
    // mdBook always emits its own default favicon <link> tags (a
    // rust-book icon, via a hashed filename baked in at build time) —
    // confirmed by actually building web/site and inspecting the output
    // rather than assumed, since mdBook's docs don't clearly say so.
    // Placing a favicon.svg in the book's src/ root gets it copied as a
    // static asset but does NOT replace those link tags, so the only way
    // to make the docs pages show the same mark as the rest of the site
    // is to remove mdBook's own tags and add ours, not just add-if-absent.
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
    if (!title || document.querySelector('.ubel-nav-links')) return;

    var nav = document.createElement('div');
    nav.className = 'ubel-nav-links';
    nav.innerHTML = LINKS.map(function (l) {
      var cls = isCurrent(l.href) ? ' class="ubel-nav-current"' : '';
      return '<a href="' + l.href + '"' + cls + '>' + l.label + '</a>';
    }).join('');
    title.insertAdjacentElement('afterend', nav);
  }

  function init() {
    injectFavicon();
    injectNav();
    loadSharedHighlighter();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
