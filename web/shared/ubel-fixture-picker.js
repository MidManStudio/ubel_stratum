// web/shared/ubel-fixture-picker.js
//
// A search box with a result list, used by the playground to pick one of the
// project's ~370 test fixtures by name. It replaces a plain <select>, which
// is unusable at that length: you could only scroll, and on a phone the
// native picker is a single endless column.
//
//   const picker = UbelFixturePicker.create({
//     input,                // <input type="search">
//     list,                 // <ul hidden>, the result list (the script fills it)
//     names,                // ['ok_simple.ubl', 'err_duplicate_def.ubl', ...]
//     onSelect(fileName)    // called with the chosen file name
//   });
//   picker.setDisabled(true, 'Loading fixtures…');
//   picker.setStatus('Fixtures unavailable');
//
// Searching: the query is split on spaces, underscores and dashes, and every
// word must appear somewhere in the fixture's name, in any order, ignoring
// case. "move after" finds err_move_after_use; "err trait" lists the error
// fixtures about traits; "ok_" or "err_" narrows to passing or failing ones.
// Names that start with the query rank first.
//
// Keys: type to filter; Up/Down move; Enter picks the highlighted entry (or
// the first one); Escape closes; Home/End jump. Tap or click an entry to pick
// it. The list closes when you click or tap outside it, not when the input
// blurs: on a touch screen the input blurs before the tap on an entry
// arrives, which would close the list under your finger.
//
// Styles are injected by this script, from the page's CSS variables with
// fallbacks, so the page does not need any.

(function (root) {
  'use strict';

  var MAX_SHOWN = 150;

  var CSS = [
    '.fx-picker{position:relative;min-width:0}',
    '.fx-picker input[type=search]{box-sizing:border-box;width:100%;min-width:12rem;background:var(--panel-alt,#2A2520);color:var(--text,#EDE6DA);border:1px solid var(--border,#3A342C);border-radius:5px;padding:.45rem .6rem;font-family:var(--font-body,system-ui,sans-serif);font-size:.82rem}',
    '.fx-picker input[type=search]:focus{outline:2px solid var(--tok,#C9A46A);outline-offset:0}',
    '.fx-picker input[type=search]:disabled{opacity:.6;cursor:wait}',
    '.fx-list{position:absolute;z-index:9000;top:calc(100% + 4px);right:0;width:min(380px,92vw);max-height:min(60vh,26rem);overflow-y:auto;margin:0;padding:.25rem 0;list-style:none;',
    '  background:var(--panel,#211D19);color:var(--text,#EDE6DA);border:1px solid var(--border,#3A342C);border-radius:6px;box-shadow:0 8px 28px rgba(0,0,0,.45);font-family:var(--font-body,system-ui,sans-serif);font-size:.82rem}',
    '.fx-list[hidden]{display:none}',
    '.fx-item{display:flex;align-items:center;gap:.55rem;padding:.5rem .7rem;cursor:pointer;min-height:1.6rem}',
    '.fx-item[aria-selected=true]{background:var(--panel-alt,#2A2520)}',
    '.fx-item:hover{background:var(--panel-alt,#2A2520)}',
    '.fx-name{font-family:var(--font-mono,ui-monospace,monospace);font-size:.78rem;overflow-wrap:anywhere;min-width:0}',
    '.fx-name mark{background:none;color:var(--tok,#C9A46A);font-weight:700;text-decoration:underline}',
    '.fx-badge{flex:0 0 auto;font-size:.68rem;line-height:1;padding:.2rem .4rem;border-radius:999px;border:1px solid var(--border,#3A342C);color:var(--text-muted,#9C9284)}',
    '.fx-badge.fx-ok{color:var(--interp,#7E9270);border-color:var(--interp,#7E9270)}',
    '.fx-badge.fx-err{color:var(--fail,#C24B3F);border-color:var(--fail,#C24B3F)}',
    '.fx-head,.fx-more{padding:.4rem .7rem;color:var(--text-muted,#9C9284);font-size:.74rem;cursor:default}',
    '.fx-empty{padding:.7rem;color:var(--text-muted,#9C9284);cursor:default}',
    '@media (max-width:640px){.fx-picker input[type=search]{min-width:0}.fx-list{left:0;right:0;width:auto;max-height:60vh}.fx-item{padding:.7rem .8rem}}'
  ].join('\n');

  function escapeHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function words(query) {
    return query.toLowerCase().split(/[\s_\-]+/).filter(Boolean);
  }

  // Index of every item: the searchable form of its name, and its group.
  function indexNames(names) {
    return names.map(function (file) {
      var key = file.replace(/\.ubl$/, '');
      var group = key.indexOf('ok_') === 0 ? 'ok' : key.indexOf('err_') === 0 ? 'err' : 'other';
      return { file: file, key: key, norm: key.toLowerCase().replace(/[_\-]+/g, ' '), group: group };
    });
  }

  // Returns the items matching `query`, best first. An empty query matches
  // everything in the original order.
  function search(items, query) {
    var ws = words(query);
    if (!ws.length) return items.slice();
    var joined = ws.join(' ');
    var hits = [];
    for (var i = 0; i < items.length; i++) {
      var it = items[i], ok = true;
      for (var w = 0; w < ws.length; w++) {
        if (it.norm.indexOf(ws[w]) < 0) { ok = false; break; }
      }
      if (!ok) continue;
      // Rank: starts with the whole query, then starts after the ok_/err_
      // prefix, then anything else; ties keep the original (sorted) order.
      var rank = 2;
      if (it.norm.indexOf(joined) === 0) rank = 0;
      else if (it.norm.replace(/^(ok|err) /, '').indexOf(joined) === 0) rank = 1;
      hits.push({ item: it, rank: rank, order: i });
    }
    hits.sort(function (a, b) { return a.rank - b.rank || a.order - b.order; });
    return hits.map(function (h) { return h.item; });
  }

  // HTML for `text` with every occurrence of any of `ws` wrapped in <mark>.
  function highlight(text, ws) {
    var lower = text.toLowerCase(), marks = [];
    ws.forEach(function (w) {
      var from = 0, at;
      while ((at = lower.indexOf(w, from)) >= 0) { marks.push([at, at + w.length]); from = at + w.length; }
    });
    if (!marks.length) return escapeHtml(text);
    marks.sort(function (a, b) { return a[0] - b[0]; });
    var merged = [marks[0].slice()];
    for (var i = 1; i < marks.length; i++) {
      var last = merged[merged.length - 1];
      if (marks[i][0] <= last[1]) last[1] = Math.max(last[1], marks[i][1]);
      else merged.push(marks[i].slice());
    }
    var out = '', pos = 0;
    merged.forEach(function (m) {
      out += escapeHtml(text.slice(pos, m[0])) + '<mark>' + escapeHtml(text.slice(m[0], m[1])) + '</mark>';
      pos = m[1];
    });
    return out + escapeHtml(text.slice(pos));
  }

  function injectStyle() {
    if (typeof document === 'undefined' || document.getElementById('ubel-fx-style')) return;
    var s = document.createElement('style');
    s.id = 'ubel-fx-style';
    s.textContent = CSS;
    document.head.appendChild(s);
  }

  function create(opts) {
    var input = opts.input, list = opts.list, onSelect = opts.onSelect || function () {};
    var items = indexNames(opts.names || []);
    var matches = [], shown = [], active = -1, pristine = false, open = false;

    injectStyle();
    input.setAttribute('role', 'combobox');
    input.setAttribute('aria-autocomplete', 'list');
    input.setAttribute('aria-expanded', 'false');
    input.setAttribute('aria-controls', list.id);
    input.setAttribute('autocomplete', 'off');
    input.setAttribute('spellcheck', 'false');
    input.setAttribute('autocapitalize', 'off');
    list.setAttribute('role', 'listbox');

    function setActive(i, scroll) {
      active = i;
      var rows = list.querySelectorAll('.fx-item');
      for (var r = 0; r < rows.length; r++) {
        rows[r].setAttribute('aria-selected', r === i ? 'true' : 'false');
      }
      if (i >= 0 && rows[i]) {
        input.setAttribute('aria-activedescendant', rows[i].id);
        if (scroll !== false && rows[i].scrollIntoView) rows[i].scrollIntoView({ block: 'nearest' });
      } else {
        input.removeAttribute('aria-activedescendant');
      }
    }

    function render() {
      var q = pristine ? '' : input.value;
      var ws = words(q);
      matches = search(items, q);
      shown = matches.slice(0, MAX_SHOWN);
      var html = '<li class="fx-head" role="presentation" aria-live="polite">' +
        (ws.length ? matches.length + ' of ' + items.length + ' fixtures match' : items.length + ' fixtures') + '</li>';
      if (!matches.length) {
        html += '<li class="fx-empty" role="presentation">No fixture name contains “' + escapeHtml(q.trim()) + '”.</li>';
      }
      shown.forEach(function (it, i) {
        html += '<li class="fx-item" role="option" id="' + list.id + '-' + i + '" data-index="' + i + '" aria-selected="false">' +
          (it.group === 'other' ? '' : '<span class="fx-badge fx-' + it.group + '">' + it.group + '</span>') +
          '<span class="fx-name">' + highlight(it.key, ws) + '</span></li>';
      });
      if (matches.length > shown.length) {
        html += '<li class="fx-more" role="presentation">' + (matches.length - shown.length) + ' more: keep typing to narrow</li>';
      }
      list.innerHTML = html;
      setActive(shown.length && ws.length ? 0 : -1, false);
    }

    function openList() {
      if (input.disabled) return;
      render();
      list.hidden = false;
      open = true;
      input.setAttribute('aria-expanded', 'true');
    }
    function closeList() {
      list.hidden = true;
      open = false;
      input.setAttribute('aria-expanded', 'false');
      input.removeAttribute('aria-activedescendant');
    }

    function choose(i) {
      var it = shown[i];
      if (!it) return;
      input.value = it.key;
      pristine = true;
      closeList();
      onSelect(it.file);
    }

    input.addEventListener('focus', function () {
      input.select();
      openList();
    });
    input.addEventListener('click', function () { if (!open) openList(); });
    input.addEventListener('input', function () {
      pristine = false;
      openList();
    });
    input.addEventListener('keydown', function (e) {
      var rows = shown.length;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (!open) { openList(); return; }
        if (rows) setActive(active < rows - 1 ? active + 1 : 0);
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (!open) { openList(); return; }
        if (rows) setActive(active > 0 ? active - 1 : rows - 1);
      } else if (e.key === 'Home' && open && rows && e.ctrlKey) {
        e.preventDefault(); setActive(0);
      } else if (e.key === 'End' && open && rows && e.ctrlKey) {
        e.preventDefault(); setActive(rows - 1);
      } else if (e.key === 'Enter') {
        if (open && rows) { e.preventDefault(); choose(active >= 0 ? active : 0); }
      } else if (e.key === 'Escape') {
        if (open) { e.preventDefault(); closeList(); }
        else if (input.value) { input.value = ''; pristine = false; }
      } else if (e.key === 'Tab') {
        closeList();
      }
    });

    list.addEventListener('click', function (e) {
      var row = e.target.closest ? e.target.closest('.fx-item') : null;
      if (row) choose(parseInt(row.getAttribute('data-index'), 10));
    });
    list.addEventListener('mousemove', function (e) {
      var row = e.target.closest ? e.target.closest('.fx-item') : null;
      if (row) {
        var i = parseInt(row.getAttribute('data-index'), 10);
        if (i !== active) setActive(i, false);
      }
    });
    document.addEventListener('pointerdown', function (e) {
      if (open && !input.contains(e.target) && !list.contains(e.target)) closeList();
    });

    return {
      setDisabled: function (disabled, placeholder) {
        input.disabled = !!disabled;
        if (placeholder != null) input.placeholder = placeholder;
        if (disabled) closeList();
      },
      setStatus: function (text) { input.placeholder = text; },
      // Exposed for tests and for callers that want the same matching.
      search: function (q) { return search(items, q).map(function (it) { return it.file; }); }
    };
  }

  var api = { create: create, search: search, indexNames: indexNames, highlight: highlight, words: words };
  root.UbelFixturePicker = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
})(typeof window !== 'undefined' ? window : globalThis);
