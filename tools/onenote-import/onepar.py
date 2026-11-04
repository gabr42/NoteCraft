"""Minimal read-only MS-ONESTORE / MS-ONE reader (OneNote 2010+ .one files).
Builds section -> pages -> (title, outlines, ...) from object spaces. Own implementation."""
import struct, uuid, datetime

NIL64 = 0xFFFFFFFFFFFFFFFF

# --- property ids (key = value & 0x7FFFFFFF) ---
P_PAGE_LEVEL = 0x14001DFF; P_CACHED_TITLE = 0x1C001CF3
P_CREATED32 = 0x14001D09; P_MOD32 = 0x14001D7A; P_MODSTAMP = 0x18001D77; P_TOPO_CREATED = 0x18001C65
P_CONTENT = 0x24001C1F; P_ELEM = 0x24001C20
P_RICH_UNI = 0x1C001C22; P_TEXT_ASCII = 0x1C003498; P_RUN_INDEX = 0x1C001E12; P_RUN_FMT = 0x24001E13
P_BOLD = 0x08001C04; P_ITALIC = 0x08001C05; P_UNDER = 0x08001C06; P_STRIKE = 0x08001C07
P_SUPER = 0x08001C08; P_SUB = 0x08001C09; P_HYPERLINK = 0x08001E14; P_HIDDEN = 0x08001E16
P_FONT = 0x1C001C0A; P_PARA_STYLE = 0x2000342C; P_PARA_STYLE_ID = 0x1C00345A
P_LIST_NODES = 0x24001C26; P_NUMLIST_FMT = 0x1C001C1A
P_PICTURE = 0x20001C3F; P_IMG_ALT = 0x1C001E58; P_IMG_NAME = 0x1C001DD7
P_EMB_CONT = 0x20001D9B; P_EMB_NAME = 0x1C001D9C
P_ROWS = 0x14001D57; P_COLS = 0x14001D58
P_OFF_H = 0x14001C14; P_OFF_V = 0x14001C15
P_CHILD_GRAPH = 0x2C001D63; P_META_ABOVE = 0x24003442
P_CHARSET = 0x0C001D01
P_NOTETAG_STATES = 0x40003489

J_SECTION = 0x00060007; J_PAGESERIES = 0x00060008; J_PAGE = 0x0006000B; J_OUTLINE = 0x0006000C
J_OE = 0x0006000D; J_RICHTEXT = 0x0006000E; J_IMAGE = 0x00060011; J_NUMLIST = 0x00060012
J_TABLE = 0x00060022; J_ROW = 0x00060023; J_CELL = 0x00060024; J_TITLE = 0x0006002C
J_EMBEDDED = 0x00060035; J_PAGEMANIFEST = 0x00060037


class Obj:
    __slots__ = ('oid', 'jcid', 'props')
    def __init__(self, oid, jcid, props):
        self.oid = oid; self.jcid = jcid; self.props = props


class Rev:
    def __init__(self, rid, dep, ctx_nil):
        self.rid = rid; self.dep = dep; self.ctx_nil = ctx_nil
        self.objects = {}; self.filedecl = {}; self.roots = {}; self.table = {}


class Space:
    def __init__(self, gosid):
        self.gosid = gosid
        self.revs = {}         # rid -> Rev
        self.order = []        # rids in file order
        self.objects = {}      # oid -> Obj   (effective state of the head revision)
        self.filedecl = {}     # oid -> (ref string, ext)
        self.roots = {}        # role -> oid

    def finalize(self):
        cand = [self.revs[r] for r in self.order if self.revs[r].ctx_nil]
        if not cand: return
        # Revision order/dependency does not tell which state is newest (measured: the base revision can hold
        # the newest edits). Use the objects' own LastModifiedTime: per oid the copy with the latest stamp wins;
        # revisions are applied freshest-last so stamp-less objects (styles, metadata) follow the freshest revision.
        MIN = datetime.datetime.min
        def stamp(ob):
            return time32(ob.props.get(P_MOD32)) or MIN
        fresh = {r.rid: max([stamp(o) for o in r.objects.values()] or [MIN]) for r in cand}
        ordered = sorted(cand, key=lambda r: fresh[r.rid])        # stable: ties keep file order
        best = {}
        for r in ordered:
            for oid, ob in r.objects.items():
                prev = best.get(oid)
                if prev is None or stamp(ob) >= stamp(prev): best[oid] = ob
            self.filedecl.update(r.filedecl)
        self.objects = best
        for r in ordered:                                         # roots from the freshest revision having them
            if r.roots: self.roots = dict(r.roots)
        self.head_rid = ordered[-1].rid; self.n_revs = len(cand)


class OneFile:
    def __init__(self, data):
        self.d = data
        self.spaces = {}
        self.root_gosid = None
        self.filedata = {}     # guid(lower, no braces) -> bytes
        self.cur = None        # current Space
        self.table = {}        # current global id table
        self.saved_tables = []
        self.dep = None
        hdr_type = uuid.UUID(bytes_le=data[0:16])
        if str(hdr_type)[:8] != '7b5c52e4':
            raise ValueError('not a .one section file: %s' % hdr_type)
        self.currev = None
        stp, cb = struct.unpack('<QI', data[172:184])
        self.read_list(stp, cb)
        for sp in self.spaces.values(): sp.finalize()

    def dep_rev_table(self):
        r = self.currev
        d = self.cur.revs.get(r.dep) if r is not None else None
        return dict(d.table) if d is not None else {}

    # ---- primitives ----
    def eg(self, pos):
        g, n = struct.unpack_from('<16sI', self.d, pos)
        return (uuid.UUID(bytes_le=g), n)

    def fcr(self, pos, stp_f, cb_f):
        d = self.d
        if stp_f == 0: stp, = struct.unpack_from('<Q', d, pos); pos += 8
        elif stp_f == 1: stp, = struct.unpack_from('<I', d, pos); pos += 4
        elif stp_f == 2: stp, = struct.unpack_from('<H', d, pos); pos += 2; stp *= 8
        else: stp, = struct.unpack_from('<I', d, pos); pos += 4; stp *= 8
        if cb_f == 0: cb, = struct.unpack_from('<I', d, pos); pos += 4
        elif cb_f == 1: cb, = struct.unpack_from('<Q', d, pos); pos += 8
        elif cb_f == 2: cb, = struct.unpack_from('<B', d, pos); pos += 1; cb *= 8
        else: cb, = struct.unpack_from('<H', d, pos); pos += 2; cb *= 8
        return stp, cb, pos

    def cid(self, raw):
        n = raw & 0xFF; idx = raw >> 8
        g = self.table.get(idx)
        return None if g is None else (g, n)

    # ---- file node lists ----
    def read_list(self, stp, cb):
        d = self.d
        while True:
            end = stp + cb
            pos = stp + 16
            while pos + 4 <= end - 20:
                hdr, = struct.unpack_from('<I', d, pos)
                fid = hdr & 0x3FF; size = (hdr >> 10) & 0x1FFF
                if fid == 0 or fid == 0xFF or size < 4: break
                self.node(fid, pos + 4, pos + size, (hdr >> 23) & 3, (hdr >> 25) & 3)
                pos += size
            nstp, ncb = struct.unpack_from('<QI', d, end - 20)
            if nstp == NIL64: break
            stp, cb = nstp, ncb

    def node(self, fid, pos, nend, stp_f, cb_f):
        d = self.d
        if fid == 0x04:                                   # ObjectSpaceManifestRootFND
            self.root_gosid = self.eg(pos)
        elif fid == 0x08:                                 # ObjectSpaceManifestListReferenceFND
            s, c, p = self.fcr(pos, stp_f, cb_f)
            self.read_list(s, c)
        elif fid == 0x0C:                                 # ObjectSpaceManifestListStartFND
            g = self.eg(pos)
            self.cur = self.spaces.setdefault(g, Space(g))
        elif fid == 0x10:                                 # RevisionManifestListReferenceFND
            s, c, p = self.fcr(pos, stp_f, cb_f)
            self.read_list(s, c)
        elif fid in (0x1B, 0x1E, 0x1F):                   # RevisionManifestStart4/6/7
            rid = self.eg(pos); dep = self.eg(pos + 20)
            ctx_nil = True
            if fid == 0x1F:
                g, n = self.eg(pos + 20 + 20 + 6)
                ctx_nil = (g.int == 0)
            self.currev = Rev(rid, dep, ctx_nil)
            self.cur.revs[rid] = self.currev; self.cur.order.append(rid)
            self.table = {}
            self.saved_tables = []
        elif fid == 0x1C:                                 # RevisionManifestEndFND
            if self.currev is not None: self.currev.table = dict(self.table)
        elif fid == 0x21:                                 # GlobalIdTableStartFNDX
            self.table = {}
            self.dep_table = self.dep_rev_table()
        elif fid == 0x22:                                 # GlobalIdTableStart2FND
            self.dep_table = self.dep_rev_table()
            self.table = dict(self.dep_table)
        elif fid == 0x24:                                 # GlobalIdTableEntryFNDX
            idx, g = struct.unpack_from('<I16s', d, pos)
            self.table[idx] = uuid.UUID(bytes_le=g)
        elif fid == 0x25:                                 # GlobalIdTableEntry2FNDX
            a, b = struct.unpack_from('<II', d, pos)
            if a in self.dep_table: self.table[b] = self.dep_table[a]
        elif fid == 0x26:                                 # GlobalIdTableEntry3FNDX
            a, n, b = struct.unpack_from('<III', d, pos)
            for i in range(n):
                if a + i in self.dep_table: self.table[b + i] = self.dep_table[a + i]
        elif fid == 0xB0:                                 # ObjectGroupListReferenceFND
            s, c, p = self.fcr(pos, stp_f, cb_f)
            self.read_list(s, c)
        elif fid == 0xB4:                                 # ObjectGroupStartFND
            self.saved_tables.append(self.table)
        elif fid == 0xB8:                                 # ObjectGroupEndFND
            if self.saved_tables: self.table = self.saved_tables.pop()
        elif fid in (0xA4, 0xA5, 0xC4, 0xC5):             # ObjectDeclaration2(Large)RefCount (ReadOnly)
            s, c, p = self.fcr(pos, stp_f, cb_f)
            raw, jcid, flags = struct.unpack_from('<IIB', d, p)
            oid = self.cid(raw)
            if oid is not None and (jcid >> 17) & 1:
                try:
                    props = self.read_propset(s)
                    self.currev.objects[oid] = Obj(oid, jcid, props)
                except Exception as e:
                    self.currev.objects[oid] = Obj(oid, jcid, {'_error': str(e)})
        elif fid in (0x72, 0x73):                         # ObjectDeclarationFileData3(Large)RefCountFND
            raw, jcid = struct.unpack_from('<II', d, pos)
            oid = self.cid(raw)
            p = pos + 8 + (1 if fid == 0x72 else 4)
            cch, = struct.unpack_from('<I', d, p); ref = d[p + 4:p + 4 + 2 * cch].decode('utf-16le', 'replace'); p += 4 + 2 * cch
            cch2, = struct.unpack_from('<I', d, p); ext = d[p + 4:p + 4 + 2 * cch2].decode('utf-16le', 'replace')
            if oid is not None: self.currev.filedecl[oid] = (ref, ext)
        elif fid == 0x59:                                 # RootObjectReference2FNDX
            raw, role = struct.unpack_from('<II', d, pos)
            oid = self.cid(raw)
            if oid is not None: self.currev.roots[role] = oid
        elif fid == 0x5A:                                 # RootObjectReference3FND
            oid = self.eg(pos); role, = struct.unpack_from('<I', d, pos + 20)
            self.currev.roots[role] = oid
        elif fid == 0x90:                                 # FileDataStoreListReferenceFND
            s, c, p = self.fcr(pos, stp_f, cb_f)
            self.read_list(s, c)
        elif fid == 0x94:                                 # FileDataStoreObjectReferenceFND
            s, c, p = self.fcr(pos, stp_f, cb_f)
            g = uuid.UUID(bytes_le=d[p:p + 16])
            length, = struct.unpack_from('<Q', d, s + 16)
            self.filedata[str(g).lower()] = d[s + 36:s + 36 + length]

    # ---- property sets ----
    def read_propset(self, pos):
        d = self.d
        def stream(pos):
            v, = struct.unpack_from('<I', d, pos); cnt = v & 0xFFFFFF
            ids = [self.cid(x) for x in struct.unpack_from('<%dI' % cnt, d, pos + 4)]
            return ids, pos + 4 + 4 * cnt, bool(v & (1 << 30)), bool(v & (1 << 31))
        oids, pos, ext, no_osid = stream(pos)
        osids = ctxs = []
        if not no_osid: osids, pos, _, _ = stream(pos)
        if ext: ctxs, pos, _, _ = stream(pos)
        cur = {'oid': 0, 'osid': 0, 'ctx': 0}
        def take(lst, key, count):
            r = lst[cur[key]:cur[key] + count]; cur[key] += count; return r
        def ps(pos):
            n, = struct.unpack_from('<H', d, pos); pos += 2
            prids = list(struct.unpack_from('<%dI' % n, d, pos)); pos += 4 * n
            props = {}
            for prid in prids:
                t = (prid >> 26) & 0x1F
                v, pos = value(t, prid, pos)
                props[prid & 0x7FFFFFFF] = v
            return props, pos
        def value(t, prid, pos):
            if t == 1: return None, pos
            if t == 2: return bool(prid >> 31), pos
            if t == 3: return d[pos:pos + 1], pos + 1
            if t == 4: return d[pos:pos + 2], pos + 2
            if t == 5: return d[pos:pos + 4], pos + 4
            if t == 6: return d[pos:pos + 8], pos + 8
            if t == 7:
                cb, = struct.unpack_from('<I', d, pos); return d[pos + 4:pos + 4 + cb], pos + 4 + cb
            if t in (8, 0xA, 0xC): return take({8: oids, 0xA: osids, 0xC: ctxs}[t], {8: 'oid', 0xA: 'osid', 0xC: 'ctx'}[t], 1), pos
            if t in (9, 0xB, 0xD):
                cnt, = struct.unpack_from('<I', d, pos)
                return take({9: oids, 0xB: osids, 0xD: ctxs}[t], {9: 'oid', 0xB: 'osid', 0xD: 'ctx'}[t], cnt), pos + 4
            if t == 0x10:
                cnt, = struct.unpack_from('<I', d, pos); pos += 4
                out = []
                if cnt:
                    sub, = struct.unpack_from('<I', d, pos); pos += 4
                    st = (sub >> 26) & 0x1F
                    for _ in range(cnt):
                        v, pos = value(st, sub, pos); out.append(v)
                return out, pos
            if t == 0x11:
                return ps(pos)
            raise ValueError('prop type %x' % t)
        props, _ = ps(pos)
        return props


def u16s(b):
    return b.decode('utf-16le', 'replace').rstrip('\x00') if b else ''

def filetime(b):
    if not b or len(b) != 8: return None
    v, = struct.unpack('<Q', b)
    if v == 0: return None
    try: return datetime.datetime(1601, 1, 1) + datetime.timedelta(microseconds=v // 10)
    except OverflowError: return None

def time32(b):
    if not b or len(b) != 4: return None
    v, = struct.unpack('<I', b)
    if v == 0: return None
    try: return datetime.datetime(1980, 1, 1) + datetime.timedelta(seconds=v)
    except OverflowError: return None
