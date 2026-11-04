"""Convert a .one section to Markdown pages (own implementation, read-only on the source)."""
import sys, os, re, uuid, struct, datetime
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from onepar import *
from cleanup import strip_spans

MONO = {'courier new', 'consolas', 'lucida console', 'courier', 'monaco', 'andale mono'}
CODEPAGES = {0: 'cp1252', 238: 'cp1250', 204: 'cp1251', 161: 'cp1253', 162: 'cp1254', 186: 'cp1257'}


def esc_plain(t):
    return re.sub(r'([\\*`\[\]<$])', r'\\\1', t)


URL_RE = re.compile(r'(?:https?|ftp)://[^\s<>"]+', re.I)


def esc(t):
    """escape markdown specials; bare URLs become <url> autolinks (as the earlier export did)"""
    out = []; pos = 0
    for m in URL_RE.finditer(t):
        url = m.group(0); tail = ''
        while url and url[-1] in ".,;:!?)'":
            tail = url[-1] + tail; url = url[:-1]
        out.append(esc_plain(t[pos:m.start()])); out.append('<%s>' % url); out.append(esc_plain(tail))
        pos = m.end()
    out.append(esc_plain(t[pos:]))
    return ''.join(out)


def last_sunday(y, m):
    d = datetime.date(y, m, 31)
    return d - datetime.timedelta(days=(d.weekday() + 1) % 7)


def local(dt):
    """UTC -> Central European time (EU DST rule)"""
    if dt is None: return None
    a = datetime.datetime.combine(last_sunday(dt.year, 3), datetime.time(1))
    b = datetime.datetime.combine(last_sunday(dt.year, 10), datetime.time(1))
    return dt + datetime.timedelta(hours=2 if a <= dt < b else 1)


class Page:
    def __init__(self):
        self.title = ''; self.created = None; self.updated = None; self.level = 0
        self.blocks = []           # list of outline renderings (each: list of lines)


class Converter:
    def __init__(self, one, add_resource):
        """add_resource(bytes, ext) -> filename in resources folder"""
        self.o = one; self.add_resource = add_resource
        self.sp = None
        self.warn = []

    # ---- helpers ----
    def get(self, oid):
        return self.sp.objects.get(oid) if oid is not None else None

    def kids(self, ob, *keys):
        out = []
        for k in keys:
            for c in ob.props.get(k) or []:
                x = self.get(c)
                if x is not None: out.append(x)
        return out

    def text_of(self, rt):
        p = rt.props
        raw = p.get(P_RICH_UNI)
        if raw is not None:
            txt = raw.decode('utf-16le', 'replace')
        elif p.get(P_TEXT_ASCII) is not None:
            cs = p.get(P_CHARSET); cs = cs[0] if cs else 0
            txt = p[P_TEXT_ASCII].decode(CODEPAGES.get(cs, 'cp1252'), 'replace')
        else:
            return ''
        return txt.rstrip('\x00')

    def runs(self, rt, txt):
        idx = rt.props.get(P_RUN_INDEX)
        idxs = list(struct.unpack('<%dI' % (len(idx) // 4), idx)) if idx else []
        fmts = rt.props.get(P_RUN_FMT) or []
        bounds = [0] + idxs + [len(txt)]
        out = []
        for i in range(len(bounds) - 1):
            a, b = bounds[i], min(bounds[i + 1], len(txt))
            if b <= a: continue
            f = self.get(fmts[i]) if i < len(fmts) else None
            out.append((txt[a:b], f.props if f else {}))
        return out

    def render_rich(self, rt):
        txt = self.text_of(rt)
        if not txt: return ''
        style = ''
        ps = self.get((rt.props.get(P_PARA_STYLE) or [None])[0])
        if ps: style = u16s(ps.props.get(P_PARA_STYLE_ID)).lower()
        parts = []
        for t, f in self.runs(rt, txt):
            if f.get(P_HIDDEN): continue
            link = None
            if '﷟' in t:
                m = re.search(r'﷟HYPERLINK "([^"]*)"', t)
                if m:
                    link = m.group(1); t = (t[:m.start()] + t[m.end():]).replace('﷟', '')
                    if not t: t = link
            t = t.replace('\r\n', '\n').replace('\r', '\n').replace('\x0b', '\n').replace(' ', '\n').replace('\x00', '')
            t = t.replace('﷟', '')
            font = u16s(f.get(P_FONT)).lower() if f.get(P_FONT) else ''
            code = font in MONO
            lead = t[:len(t) - len(t.lstrip(' \t'))]; trail = t[len(t.rstrip(' \t')):]
            core = t.strip(' \t')
            if not core:
                parts.append(t); continue
            body = ('`' + core.replace('`', "'") + '`') if code else (esc_plain(core) if link else esc(core))
            if link:
                body = '<%s>' % link if core == link else '[%s](%s)' % (body, link)
            if f.get(P_STRIKE): body = '~~%s~~' % body
            if f.get(P_BOLD) and f.get(P_ITALIC): body = '***%s***' % body
            elif f.get(P_BOLD): body = '**%s**' % body
            elif f.get(P_ITALIC): body = '*%s*' % body
            parts.append(lead + body + trail)
        s = ''.join(parts)
        s = re.sub(r'\n', '  \n', s)
        m = re.match(r'h([1-6])$', style)
        if m and s.strip() and '\n' not in s: s = '#' * int(m.group(1)) + ' ' + s
        return s

    def image_md(self, im):
        ref = (im.props.get(P_PICTURE) or [None])[0]
        fd = self.sp.filedecl.get(ref) if ref else None
        alt = u16s(im.props.get(P_IMG_ALT)) or 'image1'
        if not fd:
            self.warn.append('image without data'); return ''
        guid = fd[0].replace('<ifndf>{', '').replace('}', '').lower()
        data = self.o.filedata.get(guid)
        if not data:
            self.warn.append('image data missing %s' % fd[0]); return ''
        ext = fd[1].lower() or self.sniff(data)
        if not ext.startswith('.'): ext = '.' + ext
        name = self.add_resource(data, ext)
        return '![%s](@RES@/%s)' % (alt.replace('\n', ' '), name)

    @staticmethod
    def sniff(d):
        if d[:8] == b'\x89PNG\r\n\x1a\n': return '.png'
        if d[:3] == b'\xff\xd8\xff': return '.jpg'
        if d[:4] == b'GIF8': return '.gif'
        if d[:2] == b'BM': return '.bmp'
        return '.bin'

    def embedded_md(self, ef):
        ref = (ef.props.get(P_EMB_CONT) or [None])[0]
        fd = self.sp.filedecl.get(ref) if ref else None
        name = u16s(ef.props.get(P_EMB_NAME)) or 'file'
        if not fd: return ''
        guid = fd[0].replace('<ifndf>{', '').replace('}', '').lower()
        data = self.o.filedata.get(guid)
        if not data:
            self.warn.append('embedded data missing %s' % name); return '`%s` (attachment data missing)' % name
        ext = os.path.splitext(name)[1].lower() or fd[1].lower()
        fn = self.add_resource(data, ext)
        return '[%s](@RES@/%s)' % (esc(name), fn)

    def todo_state(self, ob):
        st = ob.props.get(P_NOTETAG_STATES)
        if not st: return None
        for e in st:
            if isinstance(e, dict):
                comp = e.get(0x1400346F)
                done = bool(comp and struct.unpack('<I', comp)[0])
                return 'x' if done else ' '
        return None

    # ---- content tree ----
    def content_lines(self, c, indent):
        """c: a content object of an OE. returns list of lines"""
        j = c.jcid
        if j == J_RICHTEXT:
            return [self.render_rich(c)]
        if j == J_IMAGE:
            return [self.image_md(c)]
        if j == J_EMBEDDED:
            return [self.embedded_md(c)]
        if j == J_TABLE:
            return self.table_lines(c)
        return []

    def table_lines(self, t):
        rows = []
        for r in self.kids(t, P_ELEM):
            cells = []
            for cell in self.kids(r, P_ELEM):
                txt = []
                for oe in self.kids(cell, P_ELEM, P_CONTENT):
                    txt += [l for l in self.oe_lines(oe, 0, cell=True) if l.strip()]
                cells.append('<br />'.join(x.strip() for x in txt).replace('|', '\\|'))
            rows.append(cells)
        if not rows: return []
        n = max(len(r) for r in rows)
        rows = [r + [''] * (n - len(r)) for r in rows]
        out = ['| ' + ' | '.join(rows[0]) + ' |', '|' + '|'.join(['---'] * n) + '|']
        for r in rows[1:]: out.append('| ' + ' | '.join(r) + ' |')
        return out

    def list_marker(self, oe, counters, depth):
        ln = self.kids(oe, P_LIST_NODES)
        if not ln: return None
        fmt = ln[0].props.get(P_NUMLIST_FMT)
        f = fmt.decode('utf-16le', 'replace') if fmt else ''
        numbered = any(ord(ch) < 0x20 or ch == '�' for ch in f) or any(ch.isalnum() for ch in f)
        if numbered:
            counters[depth] = counters.get(depth, 0) + 1
            return '%d. ' % counters[depth]
        return '- '

    def oe_lines(self, oe, depth, cell=False, counters=None):
        counters = counters if counters is not None else {}
        lines = []
        marker = self.list_marker(oe, counters, depth) if not cell else None
        if marker is None: counters.pop(depth, None)
        first = True
        body = []
        for c in self.kids(oe, P_CONTENT):
            body += self.content_lines(c, depth)
        todo = None
        if not cell:
            todo = self.todo_state(oe)
            if todo is None:
                for c in self.kids(oe, P_CONTENT):
                    todo = self.todo_state(c)
                    if todo is not None: break
        pad = '  ' * depth if (marker or depth) else ''
        if todo is not None and marker is None: marker = '- '
        for i, b in enumerate(body):
            seg = b.split('\n')
            if i == 0:
                mk = (marker or '') + ('[%s]  ' % todo if todo is not None else '')
                lines.append(pad + mk + seg[0])
                ind = pad + ' ' * len(marker or '')
            else:
                ind = pad + ' ' * len(marker or '')
                lines.append(ind + seg[0])
            for s2 in seg[1:]: lines.append(ind + s2)
        if not body and marker is None and False: lines.append('')
        sub_counters = {}
        for ch in self.kids(oe, P_ELEM):
            lines += self.oe_lines(ch, depth + 1, cell, sub_counters)
        return lines

    def outline_lines(self, ol):
        lines = []
        counters = {}
        for oe in self.kids(ol, P_ELEM):
            lines += self.oe_lines(oe, 0, counters=counters)
        return lines

    # ---- pages ----
    def page_from_space(self, gosid, meta=None):
        self.sp = self.o.spaces[gosid]
        pg = Page()
        root = self.get(self.sp.roots.get(1)) or self.get(next(iter(self.sp.roots.values()), None))
        if root is None: return pg
        pnode = None
        stack = [root]; seen = set()
        while stack and pnode is None:
            x = stack.pop()
            if id(x) in seen: continue
            seen.add(id(x))
            if x.jcid == J_PAGE: pnode = x; break
            stack += self.kids(x, P_CONTENT, P_ELEM)
        if pnode is None:
            self.warn.append('no page node'); return pg
        pr = pnode.props
        pg.title = u16s(pr.get(P_CACHED_TITLE))
        pg.created = time32(pr.get(P_CREATED32)) or filetime(pr.get(P_TOPO_CREATED))
        pg.updated = filetime(pr.get(P_MODSTAMP)) or time32(pr.get(P_MOD32))
        lvl = pr.get(P_PAGE_LEVEL)
        pg.level = struct.unpack('<I', lvl)[0] if lvl else 0
        elems = self.kids(pnode, P_ELEM, P_CONTENT)
        def pos(e):
            h = e.props.get(P_OFF_H); v = e.props.get(P_OFF_V)
            fv = struct.unpack('<f', v)[0] if v else 0.0
            fh = struct.unpack('<f', h)[0] if h else 0.0
            return (round(fv, 1), round(fh, 1))
        title_text = ''
        for e in sorted([x for x in elems if x.jcid != J_TITLE], key=pos):
            if e.jcid == J_OUTLINE:
                ls = self.outline_lines(e)
                if any(l.strip() for l in ls): pg.blocks.append(ls)
            elif e.jcid == J_IMAGE:
                pg.blocks.append([self.image_md(e)])
            elif e.jcid == J_EMBEDDED:
                pg.blocks.append([self.embedded_md(e)])
            elif e.jcid == J_TABLE:
                pg.blocks.append(self.table_lines(e))
        for e in elems:
            if e.jcid == J_TITLE:
                for ol in self.kids(e, P_ELEM):
                    for oe in self.kids(ol, P_ELEM):
                        title_text += ' '.join(l.strip() for l in self.oe_lines(oe, 0))
        if not pg.title: pg.title = re.sub(r'\\([\\*_`\[\]<])', r'\1', title_text).strip()
        return pg

    def section_pages(self):
        o = self.o
        self.sp = o.spaces[o.root_gosid]
        root = self.get(self.sp.roots.get(1))
        pages = []
        for series in self.kids(root, P_ELEM):
            if series.jcid != J_PAGESERIES: continue
            metas = [self.sp.objects[m] for m in series.props.get(P_META_ABOVE) or [] if m in self.sp.objects]
            for i, gs in enumerate(series.props.get(P_CHILD_GRAPH) or []):
                if gs in o.spaces:
                    meta = metas[i] if i < len(metas) else (metas[0] if metas else None)
                    pg = self.page_from_space(gs)
                    self.sp = o.spaces[o.root_gosid]
                    if meta is not None:
                        t = u16s(meta.props.get(P_CACHED_TITLE))
                        if t: pg.title = t
                        c = filetime(meta.props.get(P_TOPO_CREATED))
                        if c: pg.created = c
                        lv = meta.props.get(P_PAGE_LEVEL)
                        if lv: pg.level = struct.unpack('<I', lv)[0]
                    pages.append(pg)
        return pages


def page_markdown(pg, resdepth):
    rel = '/'.join(['..'] * resdepth) + '/resources'
    def fmt(d): return local(d).strftime('%Y-%m-%dT%H:%M:%S') if d else ''
    title = pg.title or 'Neimenovana stran'
    head = ['---', 'title: ' + (('"%s"' % title.replace('"', '\\"')) if re.search(r'[:#\[\]{}&*!|>\'"%@`]', title) or title != title.strip() else title)]
    if pg.updated: head.append('updated: ' + fmt(pg.updated))
    if pg.created: head.append('created: ' + fmt(pg.created))
    head.append('---')
    out = head + ['']
    for i, b in enumerate(pg.blocks):
        if i: out.append('---')
        out += [l.replace('@RES@', rel) for l in b]
    return '\n'.join(out).rstrip() + '\n'
