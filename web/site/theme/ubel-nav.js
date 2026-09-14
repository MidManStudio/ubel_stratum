// web/site/theme/ubel-nav.js
//
// mdBook's menu bar (title + icon buttons) comes from its own built-in
// index.hbs template, which book.toml has no hook to add arbitrary links
// into; the only supported extension points are additional-css and
// additional-js. This runs on every page and inserts a small nav
// (Docs / Playground / CI Results) between the book title and mdBook's
// own icon buttons, so the three sections of the site read as one
// property instead of three unrelated pages. The GitHub icon itself is
// unaffected; that one mdBook already adds on its own via book.toml's
// git-repository-url.
//
// This file also loads the shared Ubel syntax highlighter from
// /shared/ubel-highlight.js (an absolute, site-root-relative path) rather
// than book.toml referencing it directly via additional-js: that file
// lives in web/shared/, outside this book's own source tree, and mdBook
// only copies/rewrites additional-js paths that live inside the book
// source. A path like ../shared/ubel-highlight.js would resolve correctly
// in this local checkout but not once book/ becomes the deployed site
// root, since ../ from the site root has nowhere to go. The deploy
// workflow copies web/shared/ to dist/shared/ as a sibling of the docs,
// playground, and results output, which is what makes the absolute path
// resolve correctly once live.
(function () {
  function loadSharedHighlighter() {
    if (document.querySelector('script[data-ubel-highlight]')) return;
    var script = document.createElement('script');
    script.src = '/shared/ubel-highlight.js';
    script.setAttribute('data-ubel-highlight', 'true');
    document.body.appendChild(script);
  }

  function injectNav() {
    var title = document.querySelector('.menu-title');
    if (!title || document.querySelector('.ubel-nav-links')) return;

    var nav = document.createElement('div');
    nav.className = 'ubel-nav-links';
    nav.innerHTML =
      '<a href="/" class="ubel-nav-current">Docs</a>' +
      '<a href="/playground/">Playground</a>' +
      '<a href="/results/">CI Results</a>';
    title.insertAdjacentElement('afterend', nav);
  }

  function init() {
    injectNav();
    loadSharedHighlighter();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
