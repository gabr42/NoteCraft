// NoteCraft page view and editor.
//
// The app sends the page as HTML (made from its Markdown); this script shows it, lets the person
// edit it in place when the app says so, and sends the page back as Markdown after every change.
// Messages to the app: window.ipc.postMessage("kind\npayload").
'use strict';
(function () {
  const ORIGIN = 'http://notecraft.localhost';
  const page = document.getElementById('page');
  const toolbar = document.getElementById('toolbar');
  const blockSel = document.getElementById('block');
  const tableGroup = document.getElementById('tablegroup');
  const baseEl = document.getElementById('base');
  let editing = false;
  let lastSent = null;
  let savedRange = null;

  function post(kind, ...parts) {
    try { window.ipc.postMessage([kind, ...parts].join('\n')); } catch (e) { /* not in the app */ }
  }
  window.addEventListener('error', e => post('error', String(e.message || e)));

  // ---------------------------------------------------------------------------------------------
  // Cleaning HTML (notes may contain HTML from other programs; nothing in them may run).

  const DROP = new Set(['SCRIPT', 'STYLE', 'IFRAME', 'FRAME', 'FRAMESET', 'OBJECT', 'EMBED', 'APPLET', 'LINK',
    'META', 'TITLE', 'NOSCRIPT', 'TEMPLATE', 'SVG', 'MATH', 'FORM', 'BUTTON', 'TEXTAREA', 'SELECT', 'OPTION',
    'HEAD', 'BASE', 'VIDEO', 'AUDIO', 'SOURCE', 'TRACK', 'CANVAS', 'DIALOG', 'PORTAL']);
  const KEEP = new Set(['P', 'BR', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'STRONG', 'B', 'EM', 'I', 'U', 'S', 'DEL',
    'STRIKE', 'CODE', 'PRE', 'BLOCKQUOTE', 'UL', 'OL', 'LI', 'INPUT', 'TABLE', 'THEAD', 'TBODY', 'TFOOT', 'TR',
    'TH', 'TD', 'A', 'IMG', 'HR', 'SPAN', 'DIV', 'SUP', 'SUB', 'MARK', 'KBD', 'DL', 'DT', 'DD', 'CAPTION',
    'FIGURE', 'FIGCAPTION', 'SECTION']);
  const ATTRS = {
    A: ['href', 'title'], IMG: ['src', 'alt', 'title', 'width', 'height', 'data-md-src'],
    TD: ['colspan', 'rowspan'], TH: ['colspan', 'rowspan'], INPUT: ['type', 'checked'], OL: ['start'],
    CODE: ['class'], PRE: ['class'],
  };

  function safeUrl(u) {
    const t = (u || '').replace(/[\u0000-\u0020]+/g, '').toLowerCase();
    if (t.startsWith('javascript:') || t.startsWith('vbscript:')) return false;
    if (t.startsWith('data:') && !t.startsWith('data:image/')) return false;
    return true;
  }

  function wrapChildren(el, tag) {
    const w = document.createElement(tag);
    while (el.firstChild) w.appendChild(el.firstChild);
    el.appendChild(w);
  }

  function sanitize(root) {
    for (const node of Array.from(root.childNodes)) {
      if (node.nodeType === 8 || node.nodeType === 7) { node.remove(); continue; }
      if (node.nodeType !== 1) continue;
      const tag = node.tagName.toUpperCase();
      if (DROP.has(tag)) { node.remove(); continue; }
      if (tag === 'INPUT' && (node.getAttribute('type') || '').toLowerCase() !== 'checkbox') { node.remove(); continue; }
      sanitize(node);
      // Formatting written as inline styles (exported notes, pasted text).
      const style = (node.getAttribute('style') || '').toLowerCase().replace(/\s+/g, '');
      const flags = [];
      if (/font-weight:(bold|bolder|[6-9]00)/.test(style) && !/^H[1-6]$|^TH$|^B$|^STRONG$/.test(tag)) flags.push('b');
      if (/font-style:italic/.test(style) && tag !== 'I' && tag !== 'EM') flags.push('i');
      if (/text-decoration[a-z-]*:[^;]*line-through/.test(style)) flags.push('s');
      if (/text-decoration[a-z-]*:[^;]*underline/.test(style) && tag !== 'A') flags.push('u');
      let el = node;
      if (!KEEP.has(tag)) {
        el = document.createElement('span');
        while (node.firstChild) el.appendChild(node.firstChild);
        node.replaceWith(el);
      }
      const allowed = ATTRS[el.tagName.toUpperCase()] || [];
      for (const a of Array.from(el.attributes)) {
        const name = a.name.toLowerCase();
        if (!allowed.includes(name) || ((name === 'href' || name === 'src') && !safeUrl(a.value))) el.removeAttribute(a.name);
      }
      for (const f of flags) wrapChildren(el, f);
      // Text and highlight colours (OneNote exports use them) are kept as a plain styled span.
      const colors = colorStyle(style, tag === 'FONT' ? node.getAttribute('color') : null);
      if (colors && el.tagName !== 'A') {
        const span = document.createElement('span');
        span.setAttribute('style', colors);
        while (el.firstChild) span.appendChild(el.firstChild);
        el.appendChild(span);
      }
      if (el.tagName === 'SPAN' && !el.attributes.length) el.replaceWith(...el.childNodes);
    }
  }

  const PLAIN_TEXT = /^(black|windowtext|inherit|initial|currentcolor|transparent|#000|#000000|rgb\(0,0,0\)|#252423|rgb\(37,36,35\)|rgb\(31,29,27\)|rgb\(93,90,87\)|rgb\(122,119,116\))$/;
  const PLAIN_BACK = /^(white|window|inherit|initial|transparent|#fff|#ffffff|rgb\(255,255,255\))$/;
  const COLOR = /^(#[0-9a-f]{3,8}|rgba?\([0-9.,%]+\)|[a-z]{3,20})$/;

  // "color:...;background-color:..." for the colours worth keeping, or '' (style is lower case, no spaces).
  function colorStyle(style, fontColor) {
    const pick = re => { const m = re.exec(style); return m ? m[1] : ''; };
    let fg = pick(/(?:^|;)color:([^;!]+)/) || (fontColor || '').toLowerCase().replace(/\s+/g, '');
    let bg = pick(/(?:^|;)background(?:-color)?:([^;!]+)/);
    fg = COLOR.test(fg) && !PLAIN_TEXT.test(fg) ? fg : '';
    bg = COLOR.test(bg) && !PLAIN_BACK.test(bg) ? bg : '';
    return [fg && 'color:' + fg, bg && 'background-color:' + bg].filter(Boolean).join(';');
  }

  function resolveSrc(raw) {
    if (/^[a-z]:[\\/]/i.test(raw)) return ORIGIN + '/file/' + encodeURI(raw.replace(/\\/g, '/'));
    if (/^file:\/\/\//i.test(raw)) return ORIGIN + '/file/' + raw.slice(8);
    return raw;
  }

  // Images remember how the note wrote their address; checkboxes become clickable.
  function fixMedia(root) {
    for (const img of root.querySelectorAll('img')) {
      if (img.hasAttribute('data-md-src')) continue;
      const raw = img.getAttribute('src') || '';
      img.setAttribute('data-md-src', raw);
      img.setAttribute('src', resolveSrc(raw));
    }
    for (const box of root.querySelectorAll('input[type=checkbox]')) {
      box.removeAttribute('disabled');
      box.checked = box.hasAttribute('checked');
    }
  }

  // ---------------------------------------------------------------------------------------------
  // HTML -> Markdown

  const BLOCK = new Set(['P', 'DIV', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'UL', 'OL', 'LI', 'BLOCKQUOTE', 'PRE',
    'TABLE', 'HR', 'SECTION', 'FIGURE', 'FIGCAPTION', 'DL', 'DT', 'DD', 'CAPTION', 'THEAD', 'TBODY', 'TFOOT',
    'TR', 'TD', 'TH']);
  const isBlock = n => n.nodeType === 1 && BLOCK.has(n.tagName);
  const isList = n => n.nodeType === 1 && (n.tagName === 'UL' || n.tagName === 'OL');
  const isWord = c => !!c && /[\p{L}\p{N}]/u.test(c);

  function serialize() {
    const md = blocks(page).join('\n\n').replace(/\n{3,}/g, '\n\n').trim();
    return md ? md + '\n' : '';
  }

  function blocks(parent) {
    const out = [];
    let run = [];
    const flush = () => {
      if (run.length) { const t = para(inlineOf(run)); if (t) out.push(t); run = []; }
    };
    for (const n of parent.childNodes) {
      if (n.nodeType === 3 || (n.nodeType === 1 && !isBlock(n))) { run.push(n); continue; }
      if (n.nodeType !== 1) continue;
      flush();
      const b = block(n);
      if (b) out.push(b);
    }
    flush();
    return out;
  }

  function block(el) {
    const t = el.tagName;
    if (/^H[1-6]$/.test(t)) {
      const s = inline(el).replace(/\s*\n\s*/g, ' ').trim();
      return s ? '#'.repeat(+t[1]) + ' ' + s : '';
    }
    if (isList(el)) return list(el, '').join('\n');
    if (t === 'BLOCKQUOTE') {
      const inner = blocks(el).join('\n\n');
      return inner ? inner.split('\n').map(l => (l ? '> ' + l : '>')).join('\n') : '';
    }
    if (t === 'PRE') {
      const code = el.textContent.replace(/\n$/, '');
      const m = ((el.querySelector('code') || el).getAttribute('class') || '').match(/language-(\S+)/);
      const fence = code.includes('```') ? '~~~' : '```';
      return fence + (m ? m[1] : '') + '\n' + code + '\n' + fence;
    }
    if (t === 'TABLE') return table(el);
    if (t === 'HR') return '---';
    if (Array.from(el.childNodes).some(isBlock)) return blocks(el).join('\n\n');
    return para(inline(el));
  }

  function para(s) {
    s = s.replace(/[ \t]+\n/g, '\n').replace(/\n[ \t]+/g, '\n').replace(/^\n+|\n+$/g, '').trim();
    return s.split('\n').map(escapeLineStart).join('\n');
  }

  function escapeLineStart(l) {
    if (/^\d+[.)](\s|$)/.test(l)) return l.replace(/^(\d+)([.)])/, '$1\\$2');
    if (/^(#{1,6}(\s|$)|>|[-+](\s|$)|=+\s*$|-+\s*$|~~~|\|)/.test(l)) return '\\' + l;
    return l;
  }

  function escapeText(s) {
    // A * or _ between spaces (as in "5 * 3") cannot start bold or italic, so it stays as typed.
    const loose = (str, i) => /\s/.test(str[i - 1] || 'x') && /\s/.test(str[i + 1] || 'x');
    s = s.replace(/[\\`\[\]<]/g, '\\$&').replace(/~/g, '\\~').replace(/&(?=#?[a-z0-9]+;)/gi, '\\&');
    s = s.replace(/\*/g, (m, i, str) => (loose(str, i) ? '*' : '\\*'));
    return s.replace(/_/g, (m, i, str) => ((isWord(str[i - 1]) && isWord(str[i + 1])) || loose(str, i) ? '_' : '\\_'));
  }

  function wrap(mark, s) {
    if (!s.trim()) return s;
    const lead = s.match(/^\s*/)[0];
    const trail = s.match(/\s*$/)[0];
    const core = s.slice(lead.length, s.length - trail.length);
    return lead + core.split('\n').map(l => (l.trim() ? mark + l.trim() + mark : '')).join('\n') + trail;
  }

  function wrapTag(tag, s) {
    return s.trim() ? '<' + tag + '>' + s + '</' + tag + '>' : s;
  }

  function mdUrl(u) {
    let v = u.trim();
    const scheme = /^([a-z][a-z0-9+.-]+):/i.exec(v);
    if (!scheme || scheme[1].toLowerCase() === 'file') {
      try { v = decodeURI(v); } catch (e) { /* keep as written */ }
    }
    return /[\s()<>]/.test(v) ? '<' + v.replace(/</g, '%3C').replace(/>/g, '%3E') + '>' : v;
  }

  function inlineOf(nodes) { return nodes.map(inlineNode).join(''); }
  function inline(el) { return inlineOf(Array.from(el.childNodes)); }

  function inlineNode(n) {
    if (n.nodeType === 3) return escapeText(n.nodeValue.replace(/[\s\u00a0]+/g, ' '));
    if (n.nodeType !== 1) return '';
    switch (n.tagName) {
      case 'BR': return '\n';
      case 'STRONG': case 'B': return wrap('**', inline(n));
      case 'EM': case 'I': return wrap('*', inline(n));
      case 'S': case 'DEL': case 'STRIKE': return wrap('~~', inline(n));
      case 'U': return wrapTag('u', inline(n));
      case 'SUP': return wrapTag('sup', inline(n));
      case 'SUB': return wrapTag('sub', inline(n));
      case 'CODE': {
        const c = n.textContent.replace(/\n/g, ' ');
        if (!c) return '';
        const tick = c.includes('`') ? '``' : '`';
        const pad = c.startsWith('`') || c.endsWith('`') ? ' ' : '';
        return tick + pad + c + pad + tick;
      }
      case 'A': {
        const href = (n.getAttribute('href') || '').trim();
        const text = inline(n);
        if (!href || !text.trim()) return text;
        if (n.textContent === href && /^[a-z][a-z0-9+.-]+:[^\s<>]*$/i.test(href)) return '<' + href + '>';
        return '[' + text + '](' + mdUrl(href) + ')';
      }
      case 'IMG': {
        const src = n.getAttribute('data-md-src') || n.getAttribute('src') || '';
        if (!src) return '';
        const alt = (n.getAttribute('alt') || '').replace(/[\[\]\\]/g, '');
        return '![' + alt + '](' + mdUrl(src) + ')';
      }
      case 'INPUT': return '';
      case 'SPAN': {
        const colors = colorStyle((n.getAttribute('style') || '').toLowerCase().replace(/\s+/g, ''), null);
        const text = inline(n);
        return colors && text.trim() ? '<span style="' + colors + '">' + text + '</span>' : text;
      }
      default:
        if (isBlock(n)) {
          const s = blocks(n).join('\n');
          return s ? '\n' + s + '\n' : '';
        }
        return inline(n);
    }
  }

  function list(el, pad) {
    const ordered = el.tagName === 'OL';
    let n = parseInt(el.getAttribute('start') || '1', 10) || 1;
    let inner = pad + (ordered ? '   ' : '  ');
    const lines = [];
    for (const c of el.childNodes) {
      if (c.nodeType === 3) {
        if (c.nodeValue.trim()) lines.push(pad + '- ' + escapeText(c.nodeValue.trim()));
        continue;
      }
      if (c.nodeType !== 1) continue;
      if (isList(c)) { lines.push(...list(c, inner)); continue; }
      if (c.tagName !== 'LI') {
        const s = para(inline(c));
        if (s) lines.push(pad + '- ' + s);
        continue;
      }
      const number = ordered ? (n++) + '. ' : '- ';
      inner = pad + ' '.repeat(number.length);
      const box = c.querySelector(':scope > input[type=checkbox], :scope > p:first-child > input[type=checkbox]');
      const marker = number + (box ? (box.checked ? '[x] ' : '[ ] ') : '');
      const parts = [];
      const nested = [];
      let run = [];
      const flush = () => { if (run.length) { parts.push(para(inlineOf(run))); run = []; } };
      for (const k of c.childNodes) {
        if (isList(k)) { flush(); nested.push(k); } else if (isBlock(k)) { flush(); parts.push(block(k)); } else run.push(k);
      }
      flush();
      const text = parts.filter(Boolean).join('\n').split('\n');
      lines.push((pad + marker + text[0]).replace(/\s+$/, marker.endsWith('] ') ? ' ' : ''));
      for (const l of text.slice(1)) lines.push(l ? inner + l : '');
      for (const k of nested) lines.push(...list(k, inner));
    }
    return lines;
  }

  function cellText(cell) {
    return blocks(cell).join('\n').replace(/\|/g, '\\|').replace(/\n/g, '<br>').trim();
  }

  function table(el) {
    const rows = Array.from(el.rows);
    if (!rows.length) return '';
    const grid = rows.map(r => Array.from(r.cells).map(cellText));
    const cols = Math.max(...grid.map(r => r.length));
    if (!cols) return '';
    for (const r of grid) while (r.length < cols) r.push('');
    const line = r => '| ' + r.join(' | ') + ' |';
    const out = [line(grid[0]), '|' + Array(cols).fill(' --- ').join('|') + '|'];
    for (const r of grid.slice(1)) out.push(line(r));
    return out.join('\n');
  }

  function notify() {
    if (!editing) return;
    const md = serialize();
    if (md !== lastSent) {
      lastSent = md;
      post('changed', md);
    }
  }

  // ---------------------------------------------------------------------------------------------
  // Selection helpers

  function selRange() {
    const sel = window.getSelection();
    if (!sel.rangeCount) return null;
    const r = sel.getRangeAt(0);
    return page.contains(r.commonAncestorContainer) ? r : null;
  }

  function around(tagSelector) {
    const r = selRange();
    if (!r) return null;
    let n = r.startContainer;
    if (n.nodeType !== 1) n = n.parentNode;
    const hit = n && n.closest ? n.closest(tagSelector) : null;
    return hit && page.contains(hit) ? hit : null;
  }

  function caretInto(el, atEnd) {
    const r = document.createRange();
    r.selectNodeContents(el);
    r.collapse(!atEnd);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(r);
  }

  function restoreSelection() {
    page.focus();
    if (savedRange) {
      const sel = window.getSelection();
      sel.removeAllRanges();
      sel.addRange(savedRange);
    }
  }

  document.addEventListener('selectionchange', () => {
    const r = selRange();
    if (r) savedRange = r.cloneRange();
    if (editing) updateToolbar();
  });

  function updateToolbar() {
    const b = around('h1,h2,h3,h4,h5,h6,blockquote,pre');
    blockSel.value = b ? (/^H[4-6]$/.test(b.tagName) ? 'h3' : b.tagName.toLowerCase()) : 'p';
    tableGroup.hidden = !around('td,th');
    for (const name of ['bold', 'italic', 'strikeThrough', 'insertUnorderedList', 'insertOrderedList']) {
      const btn = toolbar.querySelector('[data-cmd="' + name + '"]');
      let on = false;
      try { on = document.queryCommandState(name); } catch (e) { /* not supported */ }
      btn.classList.toggle('on', on);
    }
  }

  // ---------------------------------------------------------------------------------------------
  // Commands

  function escHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
  }

  function setBlock(v) {
    restoreSelection();
    if (v !== 'blockquote' && around('blockquote')) document.execCommand('outdent');
    if (v === 'blockquote' && around('blockquote')) return;
    document.execCommand('formatBlock', false, v === 'pre' || v === 'blockquote' ? v : '<' + v + '>');
  }

  function inlineCode() {
    const r = selRange();
    if (!r || r.collapsed) return;
    const code = around('code');
    if (code) { code.replaceWith(document.createTextNode(code.textContent)); return; }
    document.execCommand('insertHTML', false, '<code>' + escHtml(r.toString()) + '</code>');
  }

  function link() {
    const a = around('a');
    const url = window.prompt('Link address (web address, or a file or page next to this one):',
      a ? a.getAttribute('href') : 'https://');
    if (url === null) return;
    restoreSelection();
    const u = url.trim();
    if (!u || u === 'https://') { if (a) a.replaceWith(...a.childNodes); return; }
    if (!safeUrl(u)) return;
    if (a) { a.setAttribute('href', u); return; }
    const r = selRange();
    if (!r || r.collapsed) document.execCommand('insertHTML', false, '<a href="' + escHtml(u) + '">' + escHtml(u) + '</a>');
    else document.execCommand('createLink', false, u);
  }

  function task() {
    let items = [];
    const r = selRange();
    if (r) items = Array.from(page.querySelectorAll('li')).filter(li => r.intersectsNode(li) && !li.querySelector('li'));
    if (!items.length) {
      document.execCommand('insertUnorderedList');
      const li = around('li');
      if (li) items = [li];
    }
    const allOn = items.every(li => li.querySelector(':scope > input[type=checkbox]'));
    for (const li of items) {
      const box = li.querySelector(':scope > input[type=checkbox]');
      if (allOn && box) box.remove();
      else if (!box) {
        const b = document.createElement('input');
        b.type = 'checkbox';
        li.insertBefore(b, li.firstChild);
      }
    }
  }

  function newCell(tag) {
    const c = document.createElement(tag);
    c.appendChild(document.createElement('br'));
    return c;
  }

  function insertTable() {
    restoreSelection();
    const head = '<tr>' + '<th><br></th>'.repeat(3) + '</tr>';
    const row = '<tr>' + '<td><br></td>'.repeat(3) + '</tr>';
    document.execCommand('insertHTML', false,
      '<table data-new="1"><thead>' + head + '</thead><tbody>' + row + row + '</tbody></table><p><br></p>');
    const t = page.querySelector('table[data-new]');
    if (t) { t.removeAttribute('data-new'); caretInto(t.rows[0].cells[0], false); }
  }

  function cellIndex(cell) { return Array.from(cell.parentElement.cells).indexOf(cell); }

  function addRow(below) {
    const cell = around('td,th');
    if (!cell) return;
    const row = cell.parentElement;
    const tableEl = row.closest('table');
    const tr = document.createElement('tr');
    for (let i = 0; i < Math.max(1, row.cells.length); i++) tr.appendChild(newCell('td'));
    if (row.parentElement.tagName === 'THEAD') {
      let body = tableEl.tBodies[0];
      if (!body) body = tableEl.appendChild(document.createElement('tbody'));
      body.insertBefore(tr, body.firstChild);
    } else if (below) row.after(tr);
    else row.before(tr);
    caretInto(tr.cells[Math.min(cellIndex(cell), tr.cells.length - 1)], false);
  }

  function addCol(right) {
    const cell = around('td,th');
    if (!cell) return;
    const idx = cellIndex(cell);
    let target = null;
    for (const row of cell.closest('table').rows) {
      const ref = row.cells[Math.min(idx, row.cells.length - 1)];
      const c = newCell(row.parentElement.tagName === 'THEAD' || (ref && ref.tagName === 'TH') ? 'th' : 'td');
      if (!ref) row.appendChild(c);
      else if (right) ref.after(c);
      else ref.before(c);
      if (row === cell.parentElement) target = c;
    }
    if (target) caretInto(target, false);
  }

  function deleteTable(tableEl) {
    const p = document.createElement('p');
    p.appendChild(document.createElement('br'));
    tableEl.replaceWith(p);
    caretInto(p, false);
  }

  function delRow() {
    const cell = around('td,th');
    if (!cell) return;
    const row = cell.parentElement;
    const tableEl = row.closest('table');
    if (tableEl.rows.length <= 1) { deleteTable(tableEl); return; }
    const idx = cellIndex(cell);
    if (row.parentElement.tagName === 'THEAD') {
      // The next row becomes the heading row.
      const next = tableEl.tBodies[0] && tableEl.tBodies[0].rows[0];
      if (next) {
        for (const c of Array.from(next.cells)) {
          const th = document.createElement('th');
          while (c.firstChild) th.appendChild(c.firstChild);
          c.replaceWith(th);
        }
        row.replaceWith(next);
        caretInto(next.cells[Math.min(idx, next.cells.length - 1)], false);
        return;
      }
    }
    const other = row.nextElementSibling || row.previousElementSibling ||
      (tableEl.rows[0] !== row ? tableEl.rows[0] : tableEl.rows[1]);
    row.remove();
    if (other && other.cells.length) caretInto(other.cells[Math.min(idx, other.cells.length - 1)], false);
  }

  function delCol() {
    const cell = around('td,th');
    if (!cell) return;
    const tableEl = cell.closest('table');
    const idx = cellIndex(cell);
    const row = cell.parentElement;
    if (row.cells.length <= 1) { deleteTable(tableEl); return; }
    for (const r of Array.from(tableEl.rows)) if (r.cells[idx]) r.cells[idx].remove();
    caretInto(row.cells[Math.min(idx, row.cells.length - 1)], false);
  }

  function moveCell(back) {
    const cell = around('td,th');
    if (!cell) return false;
    const tableEl = cell.closest('table');
    const cells = Array.from(tableEl.querySelectorAll('th,td')).filter(c => c.closest('table') === tableEl);
    const i = cells.indexOf(cell);
    if (back) { if (i > 0) caretInto(cells[i - 1], true); return true; }
    if (i < cells.length - 1) caretInto(cells[i + 1], true);
    else { addRow(true); }
    return true;
  }

  function command(cmd) {
    restoreSelection();
    switch (cmd) {
      case 'code': inlineCode(); break;
      case 'link': link(); break;
      case 'task': task(); break;
      case 'table': insertTable(); break;
      case 'rowAbove': addRow(false); break;
      case 'rowBelow': addRow(true); break;
      case 'colLeft': addCol(false); break;
      case 'colRight': addCol(true); break;
      case 'delRow': delRow(); break;
      case 'delCol': delCol(); break;
      case 'delTable': { const t = around('table'); if (t) deleteTable(t); break; }
      default: document.execCommand(cmd, false, null);
    }
    notify();
    updateToolbar();
  }

  toolbar.addEventListener('mousedown', e => { if (e.target.closest('button')) e.preventDefault(); });
  toolbar.addEventListener('click', e => {
    const b = e.target.closest('button');
    if (b && !b.disabled) command(b.dataset.cmd);
  });
  blockSel.addEventListener('change', () => { setBlock(blockSel.value); notify(); updateToolbar(); });

  // ---------------------------------------------------------------------------------------------
  // Events

  page.addEventListener('input', notify);
  page.addEventListener('change', notify);

  page.addEventListener('click', e => {
    const box = e.target.closest('input[type=checkbox]');
    if (box && !editing) {
      const all = Array.from(page.querySelectorAll('input[type=checkbox]'));
      post('toggle', String(all.indexOf(box)), box.checked ? '1' : '0');
      return;
    }
    const a = e.target.closest('a');
    if (!a) return;
    e.preventDefault();
    if (editing && !e.ctrlKey) return;
    const href = a.getAttribute('href') || '';
    if (href.startsWith('#')) {
      const id = decodeURIComponent(href.slice(1));
      const t = document.getElementById(id) || document.getElementsByName(id)[0];
      if (t) t.scrollIntoView({ block: 'start' });
    } else if (href) post('open', href);
  });

  document.addEventListener('keydown', e => {
    const k = e.key.toLowerCase();
    const ctrl = e.ctrlKey || e.metaKey;
    const send = name => { e.preventDefault(); notify(); post('key', name); };
    if (ctrl && !e.altKey) {
      if (k === 's') return send('save');
      if (k === 'e') return send('edit');
      if (k === 'f') return send('find');
      if (k === 'p') return send('print');
      if (k === 'n') return send('new');
      if (k === 'r') return e.preventDefault();
      if (k === 'k' && editing) { e.preventDefault(); link(); notify(); return; }
    }
    if (e.key === 'F5') return send('refresh');
    if (e.key === 'Escape' && editing) return send('done');
    if (e.key === 'Tab' && editing) {
      e.preventDefault();
      if (moveCell(e.shiftKey)) return;
      if (around('li')) { document.execCommand(e.shiftKey ? 'outdent' : 'indent'); notify(); }
    }
  });

  function readImage(file) {
    const reader = new FileReader();
    reader.onload = () => {
      const s = String(reader.result);
      post('image', file.type || 'image/png', s.slice(s.indexOf(',') + 1));
    };
    reader.readAsDataURL(file);
  }

  page.addEventListener('paste', e => {
    if (!editing) return;
    const dt = e.clipboardData;
    const item = Array.from(dt.items || []).find(i => i.kind === 'file' && i.type.startsWith('image/'));
    e.preventDefault();
    if (item) { readImage(item.getAsFile()); return; }
    const html = dt.getData('text/html');
    if (html && !around('pre')) {
      const doc = new DOMParser().parseFromString(html, 'text/html');
      sanitize(doc.body);
      document.execCommand('insertHTML', false, doc.body.innerHTML);
      fixMedia(page);
    } else {
      document.execCommand('insertText', false, dt.getData('text/plain'));
    }
    notify();
  });

  document.addEventListener('dragover', e => e.preventDefault());
  document.addEventListener('drop', e => {
    e.preventDefault();
    if (!editing) return;
    const files = Array.from(e.dataTransfer.files || []).filter(f => f.type.startsWith('image/'));
    if (!files.length) return;
    const r = document.caretRangeFromPoint(e.clientX, e.clientY);
    if (r && page.contains(r.startContainer)) savedRange = r;
    files.forEach(readImage);
  });

  // ---------------------------------------------------------------------------------------------
  // Search highlight (letters with and without accents match, like the app's search).

  const SPECIAL = { 'đ': 'd', 'Đ': 'd', 'ł': 'l', 'Ł': 'l', 'ø': 'o', 'Ø': 'o', 'ß': 's', 'æ': 'a', 'Æ': 'a' };
  function fold(s) {
    let out = '';
    for (let i = 0; i < s.length; i++) {
      const c = s[i];
      const f = SPECIAL[c] || c.normalize('NFD')[0];
      out += f.toLowerCase()[0];
    }
    return out;
  }

  function clearMarks() {
    for (const m of page.querySelectorAll('mark[data-nc]')) m.replaceWith(...m.childNodes);
    page.normalize();
  }

  function highlight(words) {
    clearMarks();
    if (editing) return;
    const wanted = words.map(fold).filter(w => w.length);
    if (!wanted.length) return;
    const walker = document.createTreeWalker(page, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    let first = null;
    for (const node of nodes) {
      const f = fold(node.nodeValue);
      let ranges = [];
      for (const w of wanted) {
        for (let i = f.indexOf(w); i >= 0; i = f.indexOf(w, i + w.length)) ranges.push([i, i + w.length]);
      }
      if (!ranges.length) continue;
      ranges.sort((a, b) => a[0] - b[0]);
      const merged = [];
      for (const r of ranges) {
        const last = merged[merged.length - 1];
        if (last && r[0] <= last[1]) last[1] = Math.max(last[1], r[1]);
        else merged.push(r.slice());
      }
      let firstHere = null;
      for (let k = merged.length - 1; k >= 0; k--) {
        const [s, e] = merged[k];
        const mid = node.splitText(s);
        mid.splitText(e - s);
        const m = document.createElement('mark');
        m.dataset.nc = '1';
        mid.replaceWith(m);
        m.appendChild(mid);
        firstHere = m;
      }
      if (!first) first = firstHere;
    }
    if (first) {
      first.classList.add('current');
      first.scrollIntoView({ block: 'center' });
    }
  }

  // ---------------------------------------------------------------------------------------------
  // Called by the app

  function setEditable(on) {
    editing = !!on;
    page.contentEditable = editing ? 'true' : 'false';
    toolbar.hidden = !editing;
    if (editing) {
      clearMarks();
      document.execCommand('defaultParagraphSeparator', false, 'p');
      lastSent = serialize();
      page.focus();
      updateToolbar();
    }
  }

  function load(html, base, editable, keepScroll) {
    baseEl.href = base;
    const y = window.scrollY;
    const doc = new DOMParser().parseFromString('<!DOCTYPE html><body>' + html + '</body>', 'text/html');
    sanitize(doc.body);
    page.replaceChildren(...Array.from(doc.body.childNodes).map(n => document.adoptNode(n)));
    fixMedia(page);
    savedRange = null;
    setEditable(editable);
    window.scrollTo(0, keepScroll ? y : 0);
    post('loaded');
  }

  function insertImage(src) {
    restoreSelection();
    document.execCommand('insertHTML', false, '<img src="' + escHtml(encodeURI(src)) + '" alt="">');
    fixMedia(page);
    notify();
  }

  window.nc = { load, setEditable, highlight, insertImage, focus: () => page.focus(), serialize };
  post('ready');
})();
