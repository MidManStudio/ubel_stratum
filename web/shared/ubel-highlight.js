// web/shared/ubel-highlight.js
//
// Dependency-free Ubel syntax highlighter: no CDN, no build step. One
// tokenizer, three consumers:
//
//   1. Docs site: auto-applies to every <code class="language-ubel"> block
//      (mdBook marks a fenced ```ubel block that way).
//   2. Playground: UbelSyntax.defineCodeMirrorMode(CodeMirror) registers an
//      "ubel" mode for the editor, using the same keyword tables.
//   3. Anything else: UbelSyntax.highlight(source) returns an HTML string.
//
// Colours are never written here. Every token class maps to a CSS variable
// (--ubl-keyword, --ubl-type, ...) declared in /shared/ubel-theme.css, which
// is what lets /shared/ubel-syntax-theme.js let a reader change them from
// the site itself.
//
// The keyword tables mirror crates/core/src/lexer/keywords.rs. When a
// keyword is added to the lexer, add it here too (docs/SITE_DOCS.md).

(function (root) {
  'use strict';

  // Reserved words, in the order keywords.rs lists them (tier and allocator
  // words are split out below because they get their own colour).
  var KEYWORDS = new Set([
    'fn', 'let', 'mut', 'const', 'static', 'if', 'elif', 'else', 'match',
    'where', 'then', 'for', 'in', 'while', 'loop', 'break', 'continue',
    'return', 'summon', 'from', 'as', 'package', 'async', 'await', 'Task',
    'try', 'catch', 'fail', 'struct', 'enum', 'trait', 'impl', 'pub', 'edge',
    'unsafe', 'with', 'defer', 'and', 'or', 'not', 'true', 'false', 'null',
    'self', 'getter', 'setter', 'ref', 'deref', 'extend', 'type', 'extract',
    'using', 'lifetime',
    // Not reserved by the lexer; they are read as identifiers and given
    // meaning by the parser only in type and where-clause position.
    'dyn', 'outlives'
  ]);

  var TIER_WORDS = new Set([
    'tier', 'high', 'mid', 'low', 'arena', 'pool', 'gc', 'heap'
  ]);

  var TYPES = new Set([
    // scalars
    'int', 'uint', 'long', 'ulong', 'short', 'ushort', 'byte', 'ubyte',
    'float', 'double', 'bool', 'char', 'string', 'str', 'void',
    'i8', 'i16', 'i32', 'i64', 'u8', 'u16', 'u32', 'u64', 'f32', 'f64',
    'isize', 'usize',
    // built-in collections and wrappers
    'List', 'Dictionary', 'Set', 'Queue', 'Stack', 'InlineList',
    'Pool', 'Handle', 'Linqerizer', 'Unique', 'Shared', 'SyncShared', 'Self'
  ]);

  var INT_SUFFIX = '(?:u8|i8|u16|i16|u32|i32|u64|i64|usize|isize)';
  var NUMBER_RE = new RegExp(
    '0x[0-9a-fA-F][0-9a-fA-F_]*' + INT_SUFFIX + '?' +
    '|0b[01][01_]*' + INT_SUFFIX + '?' +
    '|[0-9][0-9_]*\\.[0-9][0-9_]*(?:[eE][+-]?[0-9][0-9_]*)?[fF]?' +
    '|[0-9][0-9_]*[eE][+-]?[0-9][0-9_]*[fF]?' +
    '|[0-9][0-9_]*' + INT_SUFFIX + '?', 'y');
  var SIZE_UNIT_RE = /[ \t]?(?:KB|MB|GB)\b/y;
  var IDENT_RE = /[A-Za-z_][A-Za-z0-9_]*/y;

  function escapeHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  // Index just past the string literal that starts at src[i]. `i` points at
  // the first character of the prefix ($, @, $@ or the opening quote).
  // Interpolation holes in $"..." may contain nested string literals, so a
  // hole is scanned with a brace depth rather than "up to the next quote".
  function scanString(src, i) {
    var n = src.length;
    var interp = false, verbatim = false;
    if (src[i] === '$') { interp = true; i++; }
    if (src[i] === '@') { verbatim = true; i++; }
    i++; // opening quote
    var depth = 0;
    while (i < n) {
      var c = src[i];
      if (depth === 0) {
        if (verbatim) {
          if (c === '"') {
            if (src[i + 1] === '"') { i += 2; continue; }
            return i + 1;
          }
        } else if (c === '\\') {
          i += 2; continue;
        } else if (c === '"') {
          return i + 1;
        }
        if (interp && c === '{') depth = 1;   // a literal brace is written \{ (skipped above)
        i++;
      } else {
        if (c === '"' || (c === '$' && (src[i + 1] === '"' || src[i + 1] === '@'))) {
          i = scanString(src, i); continue;
        }
        if (c === '{') depth++;
        else if (c === '}') depth--;
        i++;
      }
    }
    return n; // unterminated: swallow the rest, the compiler reports LEX-002
  }

  function isUpper(ch) { return ch >= 'A' && ch <= 'Z'; }

  // Classify an identifier. `prev` is the previous significant token text,
  // `next` the next non-space character (for "looks like a call").
  function classifyIdent(text, prev, next) {
    if (TIER_WORDS.has(text)) return 'tier';
    if (KEYWORDS.has(text)) return 'keyword';
    if (TYPES.has(text)) return 'type';
    if (prev === 'fn') return 'fn';
    if (next === '(') return 'fn';
    if (text.length > 1 && text === text.toUpperCase() && /[A-Z]/.test(text)) return 'const';
    if (isUpper(text[0])) return 'type';
    return null;
  }

  // Returns an array of [className|null, text] pairs covering `src` exactly.
  function tokenize(src) {
    var out = [];
    var i = 0, n = src.length, prev = '';
    function push(cls, text) {
      out.push([cls, text]);
      if (cls !== null || text.trim() !== '') prev = text;
    }
    while (i < n) {
      var c = src[i], d = src[i + 1], j;

      if (c === '/' && d === '/') {
        j = src.indexOf('\n', i); if (j < 0) j = n;
        push('comment', src.slice(i, j)); i = j; continue;
      }
      if (c === '/' && d === '*') {
        var depth = 1; j = i + 2;
        while (j < n && depth > 0) {
          if (src[j] === '/' && src[j + 1] === '*') { depth++; j += 2; }
          else if (src[j] === '*' && src[j + 1] === '/') { depth--; j += 2; }
          else j++;
        }
        push('comment', src.slice(i, j)); i = j; continue;
      }
      if (c === '"' ||
          (c === '$' && (d === '"' || (d === '@' && src[i + 2] === '"'))) ||
          (c === '@' && d === '"')) {
        j = scanString(src, i);
        push('string', src.slice(i, j)); i = j; continue;
      }
      if (c === "'") {
        var cm = /'(?:\\.|[^'\\\n])'/y; cm.lastIndex = i;
        var cmatch = cm.exec(src);
        if (cmatch) { push('string', cmatch[0]); i += cmatch[0].length; continue; }
      }
      if (c >= '0' && c <= '9') {
        NUMBER_RE.lastIndex = i;
        var nm = NUMBER_RE.exec(src);
        if (nm) {
          var text = nm[0];
          SIZE_UNIT_RE.lastIndex = i + text.length;
          var sm = SIZE_UNIT_RE.exec(src);
          if (sm) text += sm[0];
          push('number', text); i += text.length; continue;
        }
      }
      if (c === '@' && /[A-Za-z_]/.test(d || '')) {
        IDENT_RE.lastIndex = i + 1;
        var am = IDENT_RE.exec(src);
        if (am) { push('attr', '@' + am[0]); i += 1 + am[0].length; continue; }
      }
      if (/[A-Za-z_]/.test(c)) {
        IDENT_RE.lastIndex = i;
        var im = IDENT_RE.exec(src);
        var word = im[0];
        var k = i + word.length;
        while (k < n && (src[k] === ' ' || src[k] === '\t')) k++;
        var prevIsDot = prev === '.';
        var cls = classifyIdent(word, prev, src[k]);
        // `x.field` is never a keyword/type colour except a call.
        if (prevIsDot && cls !== 'fn') cls = null;
        push(cls, word); i += word.length; continue;
      }
      // Anything else: one character of plain text (whitespace, operators).
      push(null, c); i++;
    }
    return out;
  }

  function highlight(source) {
    var parts = tokenize(source), html = '';
    for (var p = 0; p < parts.length; p++) {
      var cls = parts[p][0], text = escapeHtml(parts[p][1]);
      html += cls ? '<span class="ubl-' + cls + '">' + text + '</span>' : text;
    }
    return html;
  }

  // Docs-site auto-apply. mdBook renders ```ubel as
  // <pre><code class="language-ubel">escaped text</code></pre>; textContent
  // reads the escaped text back as plain source, so re-escaping inside
  // highlight() is correct and does not double-escape.
  function applyAll(scope) {
    (scope || document).querySelectorAll('code.language-ubel').forEach(function (block) {
      if (block.getAttribute('data-ubel-highlighted') === '1') return;
      block.innerHTML = highlight(block.textContent);
      block.setAttribute('data-ubel-highlighted', '1');
    });
  }

  // CodeMirror 5 mode. Token styles come back as "ubl-keyword" etc., which
  // CodeMirror turns into the class cm-ubl-keyword; /shared/ubel-theme.css
  // colours those from the same variables as the docs.
  function defineCodeMirrorMode(CodeMirror) {
    CodeMirror.defineMode('ubel', function () {
      function blockComment(stream, state) {
        while (!stream.eol()) {
          if (stream.match('/*')) state.block++;
          else if (stream.match('*/')) { state.block--; if (state.block === 0) return; }
          else stream.next();
        }
      }
      function readString(stream, state) {
        // state.str = { verbatim, interp, depth }
        var s = state.str;
        while (!stream.eol()) {
          var ch = stream.next();
          if (s.depth === 0) {
            if (s.verbatim) {
              if (ch === '"') {
                if (stream.peek() === '"') { stream.next(); continue; }
                state.str = null; return;
              }
            } else if (ch === '\\') {
              stream.next(); continue;
            } else if (ch === '"') {
              state.str = null; return;
            }
            if (s.interp && ch === '{') s.depth = 1;   // a literal brace is \{ (handled above)
          } else {
            if (ch === '{') s.depth++;
            else if (ch === '}') s.depth--;
          }
        }
        // A plain "..." string does not continue past the end of the line.
        if (state.str && !state.str.verbatim && state.str.depth === 0) state.str = null;
      }
      return {
        lineComment: '//',
        blockCommentStart: '/*',
        blockCommentEnd: '*/',
        startState: function () { return { block: 0, str: null, prev: '' }; },
        token: function (stream, state) {
          if (state.block > 0) { blockComment(stream, state); return 'ubl-comment'; }
          if (state.str) { readString(stream, state); return 'ubl-string'; }
          if (stream.eatSpace()) return null;

          if (stream.match('//')) { stream.skipToEnd(); return 'ubl-comment'; }
          if (stream.match('/*')) { state.block = 1; blockComment(stream, state); return 'ubl-comment'; }

          var ch = stream.peek();
          if (ch === '"' || ch === '$' || ch === '@') {
            var m = stream.match(/^(\$@|\$|@)?"/);
            if (m) {
              var pre = m[1] || '';
              state.str = { verbatim: pre.indexOf('@') >= 0, interp: pre.indexOf('$') >= 0, depth: 0 };
              readString(stream, state);
              return 'ubl-string';
            }
            if (ch === '@' && stream.match(/^@[A-Za-z_][A-Za-z0-9_]*/)) return 'ubl-attr';
          }
          if (stream.match(/^'(?:\\.|[^'\\])'/)) return 'ubl-string';
          if (stream.match(/^(?:0x[0-9a-fA-F_]+|0b[01_]+|[0-9][0-9_]*(?:\.[0-9][0-9_]*)?(?:[eE][+-]?[0-9]+)?[fF]?)(?:u8|i8|u16|i16|u32|i32|u64|i64|usize|isize)?(?:[ \t]?(?:KB|MB|GB)\b)?/)) {
            return 'ubl-number';
          }
          if (stream.match(/^[A-Za-z_][A-Za-z0-9_]*/)) {
            var word = stream.current();
            var rest = stream.string.slice(stream.pos);
            var next = (rest.match(/^[ \t]*(.)/) || [])[1];
            var prev = state.prev;
            state.prev = word;
            var cls = classifyIdent(word, prev, next);
            if (prev === '.' && cls !== 'fn') cls = null;
            return cls ? 'ubl-' + cls : null;
          }
          var op = stream.next();
          state.prev = op;
          return null;
        }
      };
    });
    CodeMirror.defineMIME('text/x-ubel', 'ubel');
  }

  var api = {
    KEYWORDS: KEYWORDS,
    TIER_WORDS: TIER_WORDS,
    TYPES: TYPES,
    tokenize: tokenize,
    highlight: highlight,
    applyAll: applyAll,
    defineCodeMirrorMode: defineCodeMirrorMode
  };
  root.UbelSyntax = api;
  // Kept for the CI dashboard and any caller written against the old name.
  root.highlightUbel = highlight;

  if (typeof document !== 'undefined') {
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', function () { applyAll(); });
    } else {
      applyAll();
    }
  }
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof window !== 'undefined' ? window : globalThis);
