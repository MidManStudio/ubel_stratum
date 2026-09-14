// web/shared/ubel-highlight.js
//
// Minimal, dependency-free Ubel syntax highlighter, no CDN, no build
// step. The keyword sets and highlightUbel() itself are extracted
// verbatim from web/dashboard/index.html so the docs site's code blocks
// and the dashboard's source panel highlight identically. The dashboard
// keeps its own inline copy and calls highlightUbel() by hand on its one
// source panel; this file adds one thing the dashboard doesn't need: an
// auto-apply pass over every `<code class="language-ubel">` block, which
// is how mdBook marks up a fenced ```ubel code block.

const UBL_KEYWORDS = new Set([
  'fn','let','mut','const','if','elif','else','match','where','for','in',
  'while','loop','break','continue','return','summon','from','as','package',
  'async','await','Task','try','catch','fail','struct','enum','trait','impl',
  'pub','edge','unsafe','with','defer','and','or','not','true','false','null',
  'self','get','set','extend','type','extract','using','lifetime',
]);
const UBL_TIER_KEYWORDS = new Set(['tier','high','mid','low','arena','pool','gc','heap']);
const UBL_TYPE_KEYWORDS = new Set([
  'int','uint','long','ulong','short','ushort','byte','ubyte',
  'float','double','bool','char','string','void',
  'i8','i16','i32','i64','u8','u16','u32','u64','f32','f64','isize','usize',
  'List','Dictionary','Set','Queue','Stack',
]);
const UBL_TOKEN_RE = new RegExp([
  '(?<comment>//[^\\n]*)',
  '(?<dqstring>\\$?"(?:\\\\.|[^"\\\\])*")',
  '(?<sqstring>\'(?:\\\\.|[^\'\\\\])*\')',
  '(?<number>\\b\\d[\\d_]*(?:\\.\\d[\\d_]*)?(?:[eE][+-]?\\d+)?[fFlLuU]?)',
  '(?<ident>\\b[A-Za-z_][A-Za-z0-9_]*\\b)',
].join('|'), 'g');

function ublEscapeHtml(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function highlightUbel(source) {
  let out = '';
  let last = 0;
  for (const m of source.matchAll(UBL_TOKEN_RE)) {
    if (m.index > last) out += ublEscapeHtml(source.slice(last, m.index));
    const text = m[0];
    const g = m.groups;
    let cls = null;
    if (g.comment) cls = 'ubl-comment';
    else if (g.dqstring || g.sqstring) cls = 'ubl-string';
    else if (g.number) cls = 'ubl-number';
    else if (g.ident) {
      if (UBL_TIER_KEYWORDS.has(text)) cls = 'ubl-tier';
      else if (UBL_KEYWORDS.has(text)) cls = 'ubl-keyword';
      else if (UBL_TYPE_KEYWORDS.has(text)) cls = 'ubl-type';
    }
    out += cls ? `<span class="${cls}">${ublEscapeHtml(text)}</span>` : ublEscapeHtml(text);
    last = m.index + text.length;
  }
  if (last < source.length) out += ublEscapeHtml(source.slice(last));
  return out;
}

// ── Docs-site auto-apply ────────────────────────────────────────────
// mdBook renders a ```ubel fenced block as <pre><code class="language-ubel">
// already-escaped-text</code></pre>. textContent below reads the escaped
// text back out as plain source (the browser un-escapes it for us), so
// re-escaping inside highlightUbel is correct and doesn't double-escape.
function applyUbelHighlighting() {
  document.querySelectorAll('code.language-ubel').forEach((block) => {
    block.innerHTML = highlightUbel(block.textContent);
  });
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', applyUbelHighlighting);
} else {
  applyUbelHighlighting();
}
