// web/shared/ubel-syntax-theme.js
//
// Lets a reader change the syntax colours of Ubel code from the site itself.
// Loaded on every page that shows Ubel source: the docs (via ubel-nav.js),
// the playground, and the landing page.
//
// How it works: every Ubel token colour is a CSS variable (--ubl-keyword,
// --ubl-type, ... see /shared/ubel-theme.css). This script writes the
// reader's choices onto <html> as inline custom properties and keeps them in
// localStorage, so the docs, the playground editor and the landing page all
// pick the same palette up, and a change in one tab shows in the others.
// Nothing is sent anywhere. If storage is blocked the controls still work for
// the current page; the choice just isn't remembered.
//
// Public API: window.UbelSyntaxTheme = { TOKENS, PRESETS, getColors,
// setColors(partial), reset(), mount(container) }.

(function (root) {
  'use strict';

  var STORAGE_KEY = 'ubel.syntax.v1';

  // id, label, CSS variable, fallback (used only if the stylesheet has not
  // loaded yet; ubel-theme.css is the source of truth for the defaults).
  var TOKENS = [
    { id: 'bg',      label: 'Background', css: '--ubl-bg',      fallback: '#211D19' },
    { id: 'text',    label: 'Plain text', css: '--ubl-text',    fallback: '#EDE6DA' },
    { id: 'keyword', label: 'Keywords',   css: '--ubl-keyword', fallback: '#81a1c1' },
    { id: 'type',    label: 'Types',      css: '--ubl-type',    fallback: '#8fbcbb' },
    { id: 'tier',    label: 'Tier words', css: '--ubl-tier',    fallback: '#bf616a' },
    { id: 'string',  label: 'Strings',    css: '--ubl-string',  fallback: '#ebcb8b' },
    { id: 'number',  label: 'Numbers',    css: '--ubl-number',  fallback: '#C9A46A' },
    { id: 'comment', label: 'Comments',   css: '--ubl-comment', fallback: '#6b7a99' },
    { id: 'attr',    label: 'Attributes', css: '--ubl-attr',    fallback: '#b48ead' },
    { id: 'fn',      label: 'Functions',  css: '--ubl-fn',      fallback: '#88c0d0' },
    { id: 'const',   label: 'Constants',  css: '--ubl-const',   fallback: '#d08770' }
  ];

  // A preset is a complete palette. "Ubel" has no colours of its own: it
  // means "no overrides", so it always follows the stylesheet.
  var PRESETS = [
    { id: 'ubel', name: 'Ubel (default)', colors: null },
    { id: 'monokai', name: 'Monokai style', colors: {
      bg: '#272822', text: '#f8f8f2', keyword: '#f92672', type: '#66d9ef', tier: '#fd971f',
      string: '#e6db74', number: '#ae81ff', comment: '#75715e', attr: '#a6e22e', fn: '#a6e22e', const: '#ae81ff' } },
    { id: 'solarized-dark', name: 'Solarized dark', colors: {
      bg: '#002b36', text: '#93a1a1', keyword: '#859900', type: '#b58900', tier: '#cb4b16',
      string: '#2aa198', number: '#d33682', comment: '#586e75', attr: '#6c71c4', fn: '#268bd2', const: '#cb4b16' } },
    { id: 'solarized-light', name: 'Solarized light', colors: {
      bg: '#fdf6e3', text: '#586e75', keyword: '#859900', type: '#b58900', tier: '#cb4b16',
      string: '#2aa198', number: '#d33682', comment: '#93a1a1', attr: '#6c71c4', fn: '#268bd2', const: '#cb4b16' } },
    { id: 'github-light', name: 'GitHub light', colors: {
      bg: '#ffffff', text: '#24292f', keyword: '#cf222e', type: '#953800', tier: '#8250df',
      string: '#0a3069', number: '#0550ae', comment: '#6e7781', attr: '#116329', fn: '#8250df', const: '#0550ae' } },
    { id: 'high-contrast', name: 'High contrast', colors: {
      bg: '#000000', text: '#ffffff', keyword: '#6cb6ff', type: '#4ae0d0', tier: '#ff7b72',
      string: '#ffd866', number: '#ffa657', comment: '#a0a8b8', attr: '#d2a8ff', fn: '#7ee787', const: '#ffa657' } }
  ];

  // Shown in the panel so a reader sees their palette on realistic code.
  // Valid Ubel: it checks and runs with the `ubel` binary.
  var SAMPLE = [
    '// Sum of squares, with a MID-tier scratch arena',
    'const LIMIT: int = 10',
    '',
    '@tier(mid)',
    'fn sum_squares(n: int) int {',
    '    let mut total = 0',
    '    with arena(64KB) {',
    '        for i in 1..=n {',
    '            total += i * i',
    '        }',
    '    }',
    '    return total',
    '}',
    '',
    'fn main() {',
    '    let label: string = "total"',
    '    println($"{label} = {sum_squares(LIMIT)}")',
    '}',
    ''
  ].join('\n');

  var HEX = /^#[0-9a-fA-F]{6}$/;
  var defaults = {};   // id -> colour from the stylesheet, read once, before any override
  var current = {};    // id -> colour override chosen by the reader

  function readDefaults() {
    var cs = getComputedStyle(document.documentElement);
    TOKENS.forEach(function (t) {
      var v = (cs.getPropertyValue(t.css) || '').trim();
      defaults[t.id] = HEX.test(v) ? v.toLowerCase() : t.fallback.toLowerCase();
    });
  }

  function sanitize(obj) {
    var out = {};
    if (!obj || typeof obj !== 'object') return out;
    TOKENS.forEach(function (t) {
      var v = obj[t.id];
      if (typeof v === 'string' && HEX.test(v.trim())) out[t.id] = v.trim().toLowerCase();
    });
    return out;
  }

  function load() {
    try {
      var raw = localStorage.getItem(STORAGE_KEY);
      if (!raw) return {};
      return sanitize((JSON.parse(raw) || {}).colors);
    } catch (e) { return {}; }
  }

  function save() {
    try {
      if (Object.keys(current).length) {
        localStorage.setItem(STORAGE_KEY, JSON.stringify({ v: 1, colors: current }));
      } else {
        localStorage.removeItem(STORAGE_KEY);
      }
    } catch (e) { /* storage blocked: the palette lasts for this page only */ }
  }

  function applyToPage() {
    var style = document.documentElement.style;
    TOKENS.forEach(function (t) {
      if (current[t.id] && current[t.id] !== defaults[t.id]) style.setProperty(t.css, current[t.id]);
      else style.removeProperty(t.css);
    });
  }

  function effective(id) { return current[id] || defaults[id]; }

  // ── WCAG contrast, to warn about unreadable choices ─────────────────
  function luminance(hex) {
    var c = [1, 3, 5].map(function (i) {
      var v = parseInt(hex.substr(i, 2), 16) / 255;
      return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
  }
  function contrast(a, b) {
    var la = luminance(a), lb = luminance(b);
    var hi = Math.max(la, lb), lo = Math.min(la, lb);
    return (hi + 0.05) / (lo + 0.05);
  }

  // ── Public state operations ─────────────────────────────────────────
  function setColors(partial) {
    var clean = sanitize(partial);
    Object.keys(clean).forEach(function (k) { current[k] = clean[k]; });
    // A value equal to the default is "no override": keeps storage minimal
    // and lets later stylesheet updates reach readers who never changed it.
    Object.keys(current).forEach(function (k) { if (current[k] === defaults[k]) delete current[k]; });
    applyToPage(); save();
  }
  function replaceColors(all) {
    current = sanitize(all);
    Object.keys(current).forEach(function (k) { if (current[k] === defaults[k]) delete current[k]; });
    applyToPage(); save();
  }
  function reset() { current = {}; applyToPage(); save(); }
  function getColors() {
    var out = {};
    TOKENS.forEach(function (t) { out[t.id] = effective(t.id); });
    return out;
  }

  // ── UI ──────────────────────────────────────────────────────────────
  var CSS = [
    '.ubel-syn-btn{display:inline-flex;align-items:center;gap:.4rem;cursor:pointer;color:inherit;background:none;border:0;font:inherit;padding:.4rem;border-radius:5px;line-height:1}',
    '.ubel-syn-btn:hover,.ubel-syn-btn[aria-expanded="true"]{color:var(--tok,#C9A46A)}',
    '.ubel-syn-btn svg{width:1.15em;height:1.15em;fill:currentColor}',
    '.ubel-syn-btn-text{font-size:.85rem;border:1px solid var(--border,#3A342C);background:var(--panel-alt,#2A2520);color:var(--text,#EDE6DA);padding:.45rem .7rem}',
    '.ubel-syn-btn-float{position:fixed;right:1rem;bottom:1rem;z-index:9999;font-size:.85rem;border:1px solid var(--border,#3A342C);background:var(--panel,#211D19);color:var(--text,#EDE6DA);padding:.55rem .8rem;box-shadow:0 2px 10px rgba(0,0,0,.4)}',
    '.ubel-syn-panel{position:fixed;top:3.4rem;right:.75rem;z-index:10000;width:min(360px,94vw);max-height:calc(100vh - 4.2rem);overflow:auto;box-sizing:border-box;',
    '  background:var(--sidebar-bg,var(--panel,#211D19));color:var(--fg,var(--text,#EDE6DA));border:1px solid var(--border,var(--sidebar-spacer,#3A342C));border-radius:8px;padding:.9rem;box-shadow:0 8px 30px rgba(0,0,0,.45);font-family:var(--font-body,system-ui,sans-serif);font-size:.85rem;text-align:left}',
    '.ubel-syn-panel[hidden]{display:none}',
    '.ubel-syn-panel.ubel-syn-panel-float{top:auto;bottom:4rem}',
    '.ubel-syn-head{display:flex;align-items:center;justify-content:space-between;margin-bottom:.6rem}',
    '.ubel-syn-head strong{font-family:var(--font-display,Georgia,serif);font-size:1rem}',
    '.ubel-syn-x{background:none;border:0;color:inherit;font-size:1.3rem;line-height:1;cursor:pointer;padding:.2rem .5rem}',
    '.ubel-syn-panel select,.ubel-syn-panel input[type=text],.ubel-syn-panel textarea{width:100%;box-sizing:border-box;background:var(--panel-alt,#2A2520);color:var(--text,#EDE6DA);border:1px solid var(--border,#3A342C);border-radius:5px;padding:.45rem .5rem;font:inherit}',
    '.ubel-syn-rows{display:grid;grid-template-columns:auto 1fr auto;gap:.45rem .6rem;align-items:center;margin:.7rem 0}',
    '.ubel-syn-rows input[type=color]{width:2.4rem;height:2rem;padding:0;border:1px solid var(--border,#3A342C);border-radius:5px;background:none;cursor:pointer}',
    '.ubel-syn-rows input[type=text]{width:6.2rem;font-family:var(--font-mono,monospace);font-size:.8rem}',
    '.ubel-syn-warn{color:#e0a030;font-size:.75rem}',
    '.ubel-syn-preview{margin:.5rem 0;border-radius:6px;border:1px solid var(--border,#3A342C);overflow:auto;max-height:15rem}',
    '.ubel-syn-preview pre{margin:0;padding:.7rem;background:var(--ubl-bg);color:var(--ubl-text);font-family:var(--font-mono,monospace);font-size:.78rem;line-height:1.5;white-space:pre}',
    '.ubel-syn-preview code{background:none !important;color:inherit !important;padding:0 !important;font-family:inherit}',
    '.ubel-syn-actions{display:flex;flex-wrap:wrap;gap:.5rem;margin-top:.5rem}',
    '.ubel-syn-actions button{cursor:pointer;background:var(--panel-alt,#2A2520);color:var(--text,#EDE6DA);border:1px solid var(--border,#3A342C);border-radius:5px;padding:.5rem .8rem;font:inherit}',
    '.ubel-syn-actions button:hover{border-color:var(--tok,#C9A46A)}',
    '.ubel-syn-panel details{margin-top:.7rem}',
    '.ubel-syn-panel textarea{height:5.5rem;font-family:var(--font-mono,monospace);font-size:.75rem;margin:.4rem 0}',
    '.ubel-syn-note{color:var(--text-muted,#9C9284);font-size:.75rem;margin:.5rem 0 0}',
    '@media (max-width:640px){.ubel-syn-panel{top:auto;bottom:0;right:0;left:0;width:auto;max-height:80vh;border-radius:12px 12px 0 0}}'
  ].join('\n');

  var PALETTE_ICON =
    '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 3a9 9 0 0 0 0 18c1.1 0 2-.9 2-2 0-.5-.2-1-.5-1.3-.3-.3-.5-.8-.5-1.2 0-1.1.9-2 2-2H17a4 4 0 0 0 4-4c0-4.4-4-7.5-9-7.5zM6.5 12a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3zm3-4a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3zm5 0a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3zm3 4a1.5 1.5 0 1 1 0-3 1.5 1.5 0 0 1 0 3z"/></svg>';

  var ui = null; // { btn, panel, rows: {id: {color, text, warn}}, preset, preview, io }

  function el(tag, attrs, html) {
    var e = document.createElement(tag);
    if (attrs) Object.keys(attrs).forEach(function (k) { e.setAttribute(k, attrs[k]); });
    if (html != null) e.innerHTML = html;
    return e;
  }

  function injectStyle() {
    if (document.getElementById('ubel-syn-style')) return;
    var s = el('style', { id: 'ubel-syn-style' });
    s.textContent = CSS;
    document.head.appendChild(s);
  }

  function detectPreset() {
    var keys = Object.keys(current);
    if (keys.length === 0) return 'ubel';
    for (var p = 0; p < PRESETS.length; p++) {
      var pc = PRESETS[p].colors;
      if (!pc) continue;
      var same = TOKENS.every(function (t) { return effective(t.id) === pc[t.id].toLowerCase(); });
      if (same) return PRESETS[p].id;
    }
    return 'custom';
  }

  function refreshPanel() {
    if (!ui) return;
    TOKENS.forEach(function (t) {
      var v = effective(t.id);
      ui.rows[t.id].color.value = v;
      if (document.activeElement !== ui.rows[t.id].text) ui.rows[t.id].text.value = v;
      if (t.id !== 'bg') {
        var ratio = contrast(v, effective('bg'));
        ui.rows[t.id].warn.textContent = ratio < 3 ? 'low contrast' : '';
      }
    });
    ui.preset.value = detectPreset();
    if (window.UbelSyntax) ui.previewCode.innerHTML = window.UbelSyntax.highlight(SAMPLE);
    else ui.previewCode.textContent = SAMPLE;
    ui.io.value = JSON.stringify({ ubelSyntaxColors: getColors() }, null, 2);
  }

  function buildPanel() {
    var panel = el('div', { 'class': 'ubel-syn-panel', role: 'dialog', 'aria-label': 'Syntax colours', hidden: '' });
    panel.innerHTML =
      '<div class="ubel-syn-head"><strong>Syntax colours</strong>' +
      '<button type="button" class="ubel-syn-x" aria-label="Close">&times;</button></div>' +
      '<label>Preset <select class="ubel-syn-preset"></select></label>' +
      '<div class="ubel-syn-rows"></div>' +
      '<div class="ubel-syn-preview"><pre><code class="language-ubel" data-ubel-highlighted="1"></code></pre></div>' +
      '<div class="ubel-syn-actions"><button type="button" class="ubel-syn-reset">Reset to default</button></div>' +
      '<details><summary>Share or import a palette</summary>' +
      '<textarea class="ubel-syn-io" spellcheck="false" aria-label="Palette as JSON"></textarea>' +
      '<div class="ubel-syn-actions"><button type="button" class="ubel-syn-copy">Copy</button>' +
      '<button type="button" class="ubel-syn-import">Apply pasted JSON</button></div></details>' +
      '<p class="ubel-syn-note">Saved in this browser only. It applies to the docs, the playground and the home page.</p>';

    var presetSel = panel.querySelector('.ubel-syn-preset');
    PRESETS.forEach(function (p) { presetSel.appendChild(el('option', { value: p.id }, p.name)); });
    presetSel.appendChild(el('option', { value: 'custom' }, 'Custom'));

    var rowsHost = panel.querySelector('.ubel-syn-rows');
    var rows = {};
    TOKENS.forEach(function (t) {
      var label = el('label', { 'for': 'ubel-syn-' + t.id }, t.label);
      var color = el('input', { type: 'color', id: 'ubel-syn-' + t.id, 'aria-label': t.label + ' colour' });
      var text = el('input', { type: 'text', maxlength: '7', spellcheck: 'false', 'aria-label': t.label + ' hex value' });
      var warn = el('span', { 'class': 'ubel-syn-warn' });
      rowsHost.appendChild(label);
      var cell = el('span', null);
      cell.appendChild(color); cell.appendChild(document.createTextNode(' ')); cell.appendChild(text);
      rowsHost.appendChild(cell);
      rowsHost.appendChild(warn);
      rows[t.id] = { color: color, text: text, warn: warn };

      color.addEventListener('input', function () {
        var o = {}; o[t.id] = color.value; setColors(o); refreshPanel();
      });
      text.addEventListener('input', function () {
        var v = text.value.trim();
        if (v[0] !== '#') v = '#' + v;
        if (HEX.test(v)) { var o = {}; o[t.id] = v; setColors(o); refreshPanel(); }
      });
      text.addEventListener('blur', function () { text.value = effective(t.id); });
    });

    presetSel.addEventListener('change', function () {
      var id = presetSel.value;
      if (id === 'custom') return;
      var p = PRESETS.filter(function (x) { return x.id === id; })[0];
      if (!p || !p.colors) reset(); else replaceColors(p.colors);
      refreshPanel();
    });
    panel.querySelector('.ubel-syn-reset').addEventListener('click', function () { reset(); refreshPanel(); });
    panel.querySelector('.ubel-syn-x').addEventListener('click', function () { setOpen(false); });

    var io = panel.querySelector('.ubel-syn-io');
    panel.querySelector('.ubel-syn-copy').addEventListener('click', function (ev) {
      io.select();
      try { document.execCommand('copy'); ev.target.textContent = 'Copied'; }
      catch (e) { ev.target.textContent = 'Select and copy'; }
      setTimeout(function () { ev.target.textContent = 'Copy'; }, 1500);
    });
    panel.querySelector('.ubel-syn-import').addEventListener('click', function (ev) {
      try {
        var parsed = JSON.parse(io.value);
        replaceColors(parsed.ubelSyntaxColors || parsed);
        refreshPanel();
        ev.target.textContent = 'Applied';
      } catch (e) { ev.target.textContent = 'Not valid JSON'; }
      setTimeout(function () { ev.target.textContent = 'Apply pasted JSON'; }, 1800);
    });

    return { panel: panel, rows: rows, preset: presetSel, previewCode: panel.querySelector('code'), io: io };
  }

  function setOpen(open) {
    if (!ui) return;
    ui.panel.hidden = !open;
    ui.btn.setAttribute('aria-expanded', open ? 'true' : 'false');
    if (open) refreshPanel();
  }

  function mount(container, opts) {
    if (ui) return ui.btn;
    opts = opts || {};
    injectStyle();
    var built = buildPanel();
    var btn;
    if (opts.floating) {
      btn = el('button', { type: 'button', 'class': 'ubel-syn-btn ubel-syn-btn-float', 'aria-haspopup': 'dialog', 'aria-expanded': 'false' }, PALETTE_ICON + '<span>Syntax colours</span>');
      built.panel.className += ' ubel-syn-panel-float';
      document.body.appendChild(btn);
    } else if (opts.icon) {
      btn = el('button', { type: 'button', 'class': 'icon-button ubel-syn-btn', title: 'Syntax colours', 'aria-label': 'Syntax colours', 'aria-haspopup': 'dialog', 'aria-expanded': 'false' }, PALETTE_ICON);
      container.insertBefore(btn, container.firstChild);
    } else {
      btn = el('button', { type: 'button', 'class': 'ubel-syn-btn ubel-syn-btn-text', title: 'Change the syntax colours', 'aria-haspopup': 'dialog', 'aria-expanded': 'false' }, PALETTE_ICON + '<span>Colours</span>');
      container.appendChild(btn);
    }
    document.body.appendChild(built.panel);
    ui = { btn: btn, panel: built.panel, rows: built.rows, preset: built.preset, previewCode: built.previewCode, io: built.io };

    btn.addEventListener('click', function () { setOpen(ui.panel.hidden); });
    document.addEventListener('keydown', function (e) { if (e.key === 'Escape' && !ui.panel.hidden) { setOpen(false); btn.focus(); } });
    document.addEventListener('click', function (e) {
      if (!ui.panel.hidden && !ui.panel.contains(e.target) && !btn.contains(e.target)) setOpen(false);
    });
    return btn;
  }

  function autoMount() {
    var explicit = document.querySelector('[data-ubel-syntax-mount]');
    if (explicit) return mount(explicit);
    var mdbook = document.querySelector('#mdbook-menu-bar .right-buttons, .menu-bar .right-buttons');
    if (mdbook) return mount(mdbook, { icon: true });
    return mount(document.body, { floating: true });
  }

  // Apply stored colours immediately: highlighted code is created later and
  // reads these variables, so there is no flash of the default palette.
  readDefaults();
  current = load();
  applyToPage();

  window.addEventListener('storage', function (e) {
    if (e.key !== STORAGE_KEY) return;
    current = load(); applyToPage(); refreshPanel();
  });

  root.UbelSyntaxTheme = {
    TOKENS: TOKENS, PRESETS: PRESETS,
    getColors: getColors, setColors: setColors, reset: reset, mount: mount
  };

  function init() { if (!root.UBEL_SYNTAX_NO_AUTOMOUNT) autoMount(); }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init);
  else init();
})(window);
