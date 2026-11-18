//! Low-level readers for OneNote (.one, MS-ONESTORE / MS-ONE) files: bounds-checked byte
//! access, file chunk references, property sets and the time formats.
//! Every read returns `Err` instead of panicking, whatever the input.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// A GUID as stored in the file (little-endian fields, raw bytes).
pub type Guid = [u8; 16];
/// A GUID plus a 32-bit number, identifying an object.
pub type ExGuid = (Guid, u32);
pub type ObjId = ExGuid;

// --- property ids (key = value & 0x7FFFFFFF) ---
pub const P_PAGE_LEVEL: u32 = 0x1400_1DFF;
pub const P_CACHED_TITLE: u32 = 0x1C00_1CF3;
pub const P_CREATED32: u32 = 0x1400_1D09;
pub const P_MOD32: u32 = 0x1400_1D7A;
pub const P_MODSTAMP: u32 = 0x1800_1D77;
pub const P_TOPO_CREATED: u32 = 0x1800_1C65;
pub const P_CONTENT: u32 = 0x2400_1C1F;
pub const P_ELEM: u32 = 0x2400_1C20;
pub const P_RICH_UNI: u32 = 0x1C00_1C22;
pub const P_TEXT_ASCII: u32 = 0x1C00_3498;
pub const P_RUN_INDEX: u32 = 0x1C00_1E12;
pub const P_RUN_FMT: u32 = 0x2400_1E13;
pub const P_BOLD: u32 = 0x0800_1C04;
pub const P_ITALIC: u32 = 0x0800_1C05;
pub const P_UNDER: u32 = 0x0800_1C06;
pub const P_STRIKE: u32 = 0x0800_1C07;
pub const P_SUPER: u32 = 0x0800_1C08;
pub const P_SUB: u32 = 0x0800_1C09;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const P_HYPERLINK: u32 = 0x0800_1E14;
pub const P_HIDDEN: u32 = 0x0800_1E16;
pub const P_FONT: u32 = 0x1C00_1C0A;
pub const P_PARA_STYLE: u32 = 0x2000_342C;
pub const P_PARA_STYLE_ID: u32 = 0x1C00_345A;
pub const P_LIST_NODES: u32 = 0x2400_1C26;
pub const P_NUMLIST_FMT: u32 = 0x1C00_1C1A;
pub const P_PICTURE: u32 = 0x2000_1C3F;
pub const P_IMG_ALT: u32 = 0x1C00_1E58;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const P_IMG_NAME: u32 = 0x1C00_1DD7;
pub const P_EMB_CONT: u32 = 0x2000_1D9B;
pub const P_EMB_NAME: u32 = 0x1C00_1D9C;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const P_ROWS: u32 = 0x1400_1D57;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const P_COLS: u32 = 0x1400_1D58;
pub const P_OFF_H: u32 = 0x1400_1C14;
pub const P_OFF_V: u32 = 0x1400_1C15;
pub const P_CHILD_GRAPH: u32 = 0x2C00_1D63;
pub const P_META_ABOVE: u32 = 0x2400_3442;
pub const P_CHARSET: u32 = 0x0C00_1D01;
pub const P_NOTETAG_STATES: u32 = 0x4000_3489;

// --- object type ids (jcid) ---
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const J_SECTION: u32 = 0x0006_0007;
pub const J_PAGESERIES: u32 = 0x0006_0008;
pub const J_PAGE: u32 = 0x0006_000B;
pub const J_OUTLINE: u32 = 0x0006_000C;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const J_OE: u32 = 0x0006_000D;
pub const J_RICHTEXT: u32 = 0x0006_000E;
pub const J_IMAGE: u32 = 0x0006_0011;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const J_NUMLIST: u32 = 0x0006_0012;
pub const J_TABLE: u32 = 0x0006_0022;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const J_ROW: u32 = 0x0006_0023;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const J_CELL: u32 = 0x0006_0024;
pub const J_TITLE: u32 = 0x0006_002C;
pub const J_EMBEDDED: u32 = 0x0006_0035;
#[allow(dead_code)] // listed with the other ids; the converter does not read it
pub const J_PAGEMANIFEST: u32 = 0x0006_0037;

/// Bounds-checked little-endian reader over a byte slice.
#[derive(Clone, Copy)]
pub struct Rd<'a> {
    pub d: &'a [u8],
}

impl<'a> Rd<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Rd { d }
    }

    /// `len` bytes at `pos`, or an error when they are not all inside the buffer.
    pub fn bytes(&self, pos: usize, len: usize) -> Result<&'a [u8], String> {
        pos.checked_add(len).and_then(|end| self.d.get(pos..end)).ok_or_else(|| {
            format!("read of {len} bytes at offset {pos} is past the end of the data ({} bytes)", self.d.len())
        })
    }

    pub fn u8_at(&self, pos: usize) -> Result<u8, String> {
        Ok(self.bytes(pos, 1)?[0])
    }

    pub fn u16_at(&self, pos: usize) -> Result<u16, String> {
        let b = self.bytes(pos, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32_at(&self, pos: usize) -> Result<u32, String> {
        let b = self.bytes(pos, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64_at(&self, pos: usize) -> Result<u64, String> {
        let b = self.bytes(pos, 8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn guid_at(&self, pos: usize) -> Result<Guid, String> {
        let b = self.bytes(pos, 16)?;
        let mut g = [0u8; 16];
        g.copy_from_slice(b);
        Ok(g)
    }
}

/// Text form of a GUID, lowercase `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
pub fn guid_string(g: &Guid) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g[3], g[2], g[1], g[0], g[5], g[4], g[7], g[6], g[8], g[9], g[10], g[11], g[12], g[13], g[14], g[15]
    )
}

/// A decoded property value.
#[derive(Debug, Clone, PartialEq)]
pub enum PropValue {
    NoData,
    Bool(bool),
    /// Raw bytes of the 1, 2, 4, 8 byte values and of the length-prefixed ones.
    Bytes(Vec<u8>),
    /// References to other objects (object, object space or context ids).
    Ids(Vec<Option<ObjId>>),
    Array(Vec<PropValue>),
    Set(PropSet),
}

/// Properties of one object, keyed by property id with the top bit cleared.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PropSet(pub HashMap<u32, PropValue>);

impl PropSet {
    pub fn get(&self, id: u32) -> Option<&PropValue> {
        self.0.get(&(id & 0x7FFF_FFFF))
    }

    pub fn get_bytes(&self, id: u32) -> Option<&[u8]> {
        match self.get(id) {
            Some(PropValue::Bytes(b)) => Some(b),
            _ => None,
        }
    }

    pub fn get_bool(&self, id: u32) -> bool {
        matches!(self.get(id), Some(PropValue::Bool(true)))
    }

    pub fn get_ids(&self, id: u32) -> &[Option<ObjId>] {
        match self.get(id) {
            Some(PropValue::Ids(v)) => v,
            _ => &[],
        }
    }

    pub fn get_array(&self, id: u32) -> &[PropValue] {
        match self.get(id) {
            Some(PropValue::Array(v)) => v,
            _ => &[],
        }
    }
}

/// Reads a file chunk reference (position and size) whose field widths are given by
/// the node header formats. Returns `(stp, cb, position after the reference)`.
pub fn fcr(r: &Rd, pos: usize, stp_f: u32, cb_f: u32) -> Result<(u64, u64, usize), String> {
    let (stp, pos) = match stp_f {
        0 => (r.u64_at(pos)?, pos + 8),
        1 => (r.u32_at(pos)? as u64, pos + 4),
        2 => (r.u16_at(pos)? as u64 * 8, pos + 2),
        3 => (r.u32_at(pos)? as u64 * 8, pos + 4),
        _ => return Err(format!("bad file chunk position format {stp_f}")),
    };
    let (cb, pos) = match cb_f {
        0 => (r.u32_at(pos)? as u64, pos + 4),
        1 => (r.u64_at(pos)?, pos + 8),
        2 => (r.u8_at(pos)? as u64 * 8, pos + 1),
        3 => (r.u16_at(pos)? as u64 * 8, pos + 2),
        _ => return Err(format!("bad file chunk size format {cb_f}")),
    };
    Ok((stp, cb, pos))
}

/// Decodes UTF-16LE text (lossily) and trims trailing NULs.
pub fn u16s(b: &[u8]) -> String {
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
    String::from_utf16_lossy(&units).trim_end_matches('\0').to_string()
}

/// A Windows FILETIME (100 ns ticks since 1601-01-01 UTC); `None` when empty or invalid.
pub fn filetime(b: &[u8]) -> Option<NaiveDateTime> {
    let a: [u8; 8] = b.try_into().ok()?;
    let v = u64::from_le_bytes(a);
    if v == 0 {
        return None;
    }
    let base = NaiveDate::from_ymd_opt(1601, 1, 1)?.and_hms_opt(0, 0, 0)?;
    let t = base.checked_add_signed(Duration::microseconds((v / 10) as i64))?;
    // Like the Python tool, treat years past 9999 as invalid.
    if t.year() > 9999 { None } else { Some(t) }
}

/// A 32-bit time (seconds since 1980-01-01 UTC); `None` when empty or invalid.
pub fn time32(b: &[u8]) -> Option<NaiveDateTime> {
    let a: [u8; 4] = b.try_into().ok()?;
    let v = u32::from_le_bytes(a);
    if v == 0 {
        return None;
    }
    let base = NaiveDate::from_ymd_opt(1980, 1, 1)?.and_hms_opt(0, 0, 0)?;
    base.checked_add_signed(Duration::try_seconds(v as i64)?)
}

const MAX_DEPTH: u32 = 32;
/// Largest accepted array of elements that take no bytes (types 1 and 2).
const MAX_EMPTY_ARRAY: usize = 65_536;
/// Most values (array elements and object ids) one property set may produce. Each array is
/// bounded by the data, but nested arrays multiply; this keeps a crafted set from exhausting memory.
const MAX_PROPSET_VALUES: usize = 1 << 20;

/// One id stream of a property set: (ids, position after it, extended flag, no-osid flag).
type IdStream = (Vec<Option<ObjId>>, usize, bool, bool);

/// Reads the property set at `pos`: the object, object space and context id streams,
/// then the properties. `cid` turns a raw compact id into an object id.
pub fn read_propset(r: &Rd, pos: usize, cid: &dyn Fn(u32) -> Option<ObjId>) -> Result<PropSet, String> {
    let stream = |pos: usize| -> Result<IdStream, String> {
        let v = r.u32_at(pos)?;
        let cnt = (v & 0xFF_FFFF) as usize;
        let raw = r.bytes(pos + 4, cnt * 4)?;
        let ids = raw.as_chunks::<4>().0.iter().map(|&c| cid(u32::from_le_bytes(c))).collect();
        Ok((ids, pos + 4 + cnt * 4, v & (1 << 30) != 0, v & (1 << 31) != 0))
    };
    let (oids, mut pos, ext, no_osid) = stream(pos)?;
    let mut osids = Vec::new();
    let mut ctxs = Vec::new();
    if !no_osid {
        let (s, p, _, _) = stream(pos)?;
        osids = s;
        pos = p;
    }
    if ext {
        let (s, p, _, _) = stream(pos)?;
        ctxs = s;
        pos = p;
    }
    let mut st = PropState { r, oids, osids, ctxs, cur: [0; 3], budget: MAX_PROPSET_VALUES };
    let (props, _) = st.prop_set(pos, 0)?;
    Ok(props)
}

struct PropState<'a, 'b> {
    r: &'b Rd<'a>,
    oids: Vec<Option<ObjId>>,
    osids: Vec<Option<ObjId>>,
    ctxs: Vec<Option<ObjId>>,
    /// Next unused entry of the oid, osid and context streams.
    cur: [usize; 3],
    /// Values this property set may still produce (see `MAX_PROPSET_VALUES`).
    budget: usize,
}

impl PropState<'_, '_> {
    /// Uses up `n` values of the budget; an error when the set would produce too many.
    fn spend(&mut self, n: usize) -> Result<(), String> {
        self.budget = self
            .budget
            .checked_sub(n)
            .ok_or_else(|| format!("property set has more than {MAX_PROPSET_VALUES} values"))?;
        Ok(())
    }

    /// Takes the next `count` ids of stream `which` (0 oid, 1 osid, 2 context); short streams give fewer.
    fn take(&mut self, which: usize, count: usize) -> Result<Vec<Option<ObjId>>, String> {
        let list = match which {
            0 => &self.oids,
            1 => &self.osids,
            _ => &self.ctxs,
        };
        let start = self.cur[which].min(list.len());
        let end = start.saturating_add(count).min(list.len());
        self.cur[which] = self.cur[which].saturating_add(count);
        let ids = list[start..end].to_vec();
        self.spend(ids.len())?;
        Ok(ids)
    }

    fn prop_set(&mut self, pos: usize, depth: u32) -> Result<(PropSet, usize), String> {
        if depth > MAX_DEPTH {
            return Err("property sets are nested too deeply".to_string());
        }
        let n = self.r.u16_at(pos)? as usize;
        let mut pos = pos + 2;
        let raw = self.r.bytes(pos, n * 4)?;
        let prids: Vec<u32> = raw.as_chunks::<4>().0.iter().map(|&c| u32::from_le_bytes(c)).collect();
        pos += n * 4;
        let mut props = HashMap::new();
        for prid in prids {
            let t = (prid >> 26) & 0x1F;
            let (v, p) = self.value(t, prid, pos, depth)?;
            pos = p;
            props.insert(prid & 0x7FFF_FFFF, v);
        }
        Ok((PropSet(props), pos))
    }

    fn value(&mut self, t: u32, prid: u32, pos: usize, depth: u32) -> Result<(PropValue, usize), String> {
        let r = self.r;
        if depth > MAX_DEPTH {
            return Err("property values are nested too deeply".to_string());
        }
        self.spend(1)?;
        Ok(match t {
            1 => (PropValue::NoData, pos),
            2 => (PropValue::Bool(prid >> 31 != 0), pos),
            3..=6 => {
                let len = 1usize << (t - 3);
                (PropValue::Bytes(r.bytes(pos, len)?.to_vec()), pos + len)
            }
            7 => {
                let cb = r.u32_at(pos)? as usize;
                (PropValue::Bytes(r.bytes(pos + 4, cb)?.to_vec()), pos + 4 + cb)
            }
            8 | 0xA | 0xC => (PropValue::Ids(self.take(((t - 8) / 2) as usize, 1)?), pos),
            9 | 0xB | 0xD => {
                let cnt = r.u32_at(pos)? as usize;
                (PropValue::Ids(self.take(((t - 9) / 2) as usize, cnt)?), pos + 4)
            }
            0x10 => {
                let cnt = r.u32_at(pos)? as usize;
                let mut pos = pos + 4;
                let mut out = Vec::new();
                if cnt > 0 {
                    let sub = r.u32_at(pos)?;
                    pos += 4;
                    let st = (sub >> 26) & 0x1F;
                    // A real array cannot have more elements than the data (or the id stream) can hold:
                    // elements that take bytes are bounded by remaining bytes / smallest element size,
                    // id elements (types 8, A, C) by the unused ids of their stream, and elements that
                    // take nothing (types 1, 2) by a fixed cap.
                    let max = match st {
                        1 | 2 => MAX_EMPTY_ARRAY,
                        3..=6 => (r.d.len().saturating_sub(pos)) >> (st - 3),
                        7 | 9 | 0xB | 0xD | 0x10 => r.d.len().saturating_sub(pos) / 4,
                        8 | 0xA | 0xC => {
                            let which = ((st - 8) / 2) as usize;
                            let len = [&self.oids, &self.osids, &self.ctxs][which].len();
                            len.saturating_sub(self.cur[which])
                        }
                        0x11 => r.d.len().saturating_sub(pos) / 2,
                        _ => 0,
                    };
                    if cnt > max {
                        return Err(format!("property array of {cnt} elements is larger than the data allows"));
                    }
                    for _ in 0..cnt {
                        let (v, p) = self.value(st, sub, pos, depth + 1)?;
                        pos = p;
                        out.push(v);
                    }
                }
                (PropValue::Array(out), pos)
            }
            0x11 => {
                let (set, p) = self.prop_set(pos, depth + 1)?;
                (PropValue::Set(set), p)
            }
            _ => return Err(format!("unknown property type {t:#x}")),
        })
    }
}

// --- file nodes, object spaces and revisions ---

/// An object declared in an object space.
#[derive(Debug, Clone, PartialEq)]
pub struct Obj {
    pub jcid: u32,
    pub props: PropSet,
}

/// The effective state of one object space (the head of its revisions).
#[derive(Debug, Clone, Default)]
pub struct Space {
    pub objects: HashMap<ObjId, Obj>,
    /// Embedded file declarations: object id -> (reference text, extension).
    pub filedecl: HashMap<ObjId, (String, String)>,
    /// Root objects by role.
    pub roots: HashMap<u32, ObjId>,
}

/// What one revision of an object space declares.
#[derive(Debug, Clone, Default)]
pub struct RevData {
    pub objects: HashMap<ObjId, Obj>,
    pub filedecl: HashMap<ObjId, (String, String)>,
    pub roots: HashMap<u32, ObjId>,
    /// True when the revision has the nil context (only those are used).
    pub ctx_nil: bool,
}

/// Last modification time of an object; `None` (older than anything) when it has none.
fn stamp(o: &Obj) -> Option<NaiveDateTime> {
    o.props.get_bytes(P_MOD32).and_then(time32)
}

/// Merges the revisions (in file order) into the effective state of the space.
/// Revision order and dependencies do not tell which state is newest, so the objects' own
/// last-modified stamps decide: per object id the copy with the latest stamp wins, and revisions
/// are applied freshest-last so stamp-less objects follow the freshest revision.
pub fn finalize(revs: Vec<RevData>) -> Space {
    let mut cand: Vec<RevData> = revs.into_iter().filter(|r| r.ctx_nil).collect();
    let mut sp = Space::default();
    if cand.is_empty() {
        return sp;
    }
    let fresh: Vec<Option<NaiveDateTime>> =
        cand.iter().map(|r| r.objects.values().map(stamp).max().unwrap_or(None)).collect();
    let mut idx: Vec<usize> = (0..cand.len()).collect();
    idx.sort_by_key(|&i| fresh[i]); // stable: ties keep file order
    let mut best: HashMap<ObjId, Obj> = HashMap::new();
    for i in idx {
        let r = std::mem::take(&mut cand[i]);
        for (id, ob) in r.objects {
            let replace = match best.get(&id) {
                None => true,
                Some(prev) => stamp(&ob) >= stamp(prev),
            };
            if replace {
                best.insert(id, ob);
            }
        }
        sp.filedecl.extend(r.filedecl);
        // roots from the freshest revision having them
        if !r.roots.is_empty() {
            sp.roots = r.roots;
        }
    }
    sp.objects = best;
    sp
}

/// A parsed .one section file.
#[derive(Debug, Default)]
pub struct OneFile {
    pub spaces: HashMap<ExGuid, Space>,
    pub root_gosid: Option<ExGuid>,
    /// Embedded file contents by lowercase GUID text without braces.
    pub filedata: HashMap<String, Vec<u8>>,
}

const NIL64: u64 = u64::MAX;
/// FileNodeListHeader.uintMagic (MS-ONESTORE 2.4.2).
const FRAGMENT_HEADER_MAGIC: u64 = 0xA456_7AB1_F5F7_F4C4;
/// FileNodeListFragment.footer (MS-ONESTORE 2.4.1).
const FRAGMENT_FOOTER_MAGIC: u64 = 0x8BC2_15C3_8233_BA4B;
/// Smallest file node list fragment: header (16) + nextFragment (12) + footer (8).
const MIN_FRAGMENT: u64 = 36;
/// Deepest nesting of file node lists followed.
const MAX_LIST_DEPTH: u32 = 64;
/// Most file nodes read from one file; stops lists that reference each other in a fan-out.
const MAX_NODES: u64 = 50_000_000;

/// Global id table: index -> GUID. Shared like the Python dict: an object group start saves a
/// reference, so entries added inside the group stay after its end unless the table was replaced.
type Table = Rc<RefCell<HashMap<u32, Guid>>>;

struct Rev {
    dep: ExGuid,
    table: HashMap<u32, Guid>,
    data: RevData,
}

#[derive(Default)]
struct SpaceBuild {
    revs: Vec<Rev>,
    by_id: HashMap<ExGuid, usize>,
    /// Revision ids in file order (a repeated id refers to the latest revision with it).
    order: Vec<ExGuid>,
}

struct Parser<'a> {
    r: Rd<'a>,
    spaces: HashMap<ExGuid, SpaceBuild>,
    root_gosid: Option<ExGuid>,
    filedata: HashMap<String, Vec<u8>>,
    cur: Option<ExGuid>,
    currev: Option<(ExGuid, usize)>,
    table: Table,
    saved_tables: Vec<Table>,
    dep_table: HashMap<u32, Guid>,
    /// Lists being read now, to stop reference cycles.
    active: HashSet<(u64, u64)>,
    nodes: u64,
    /// Last revision manifest list reference seen in the object space manifest list being read.
    /// Only the last one is followed; earlier ones are stale (often zero) references.
    rev_list_ref: Option<(u64, u64)>,
}

fn us(v: u64) -> Result<usize, String> {
    usize::try_from(v).map_err(|_| format!("offset {v} does not fit in memory"))
}

impl OneFile {
    /// Parses the bytes of a .one file.
    pub fn parse(data: &[u8]) -> Result<OneFile, String> {
        let r = Rd::new(data);
        match r.guid_at(0) {
            Ok(g) if guid_string(&g).starts_with("7b5c52e4") => {}
            _ => return Err("not a .one section file".to_string()),
        }
        let mut p = Parser {
            r,
            spaces: HashMap::new(),
            root_gosid: None,
            filedata: HashMap::new(),
            cur: None,
            currev: None,
            table: Rc::new(RefCell::new(HashMap::new())),
            saved_tables: Vec::new(),
            dep_table: HashMap::new(),
            active: HashSet::new(),
            nodes: 0,
            rev_list_ref: None,
        };
        let stp = r.u64_at(172)?;
        let cb = r.u32_at(180)? as u64;
        p.read_list(stp, cb, 0)?;
        let mut spaces = HashMap::new();
        for (g, sb) in p.spaces {
            // a repeated revision id refers to the same (latest) revision each time
            let mut uses = vec![0usize; sb.revs.len()];
            for rid in &sb.order {
                uses[sb.by_id[rid]] += 1;
            }
            let mut slots: Vec<Option<RevData>> = sb.revs.into_iter().map(|r| Some(r.data)).collect();
            let mut list = Vec::with_capacity(sb.order.len());
            for rid in &sb.order {
                let i = sb.by_id[rid];
                uses[i] -= 1;
                let d = if uses[i] == 0 { slots[i].take() } else { slots[i].clone() };
                list.extend(d);
            }
            spaces.insert(g, finalize(list));
        }
        Ok(OneFile { spaces, root_gosid: p.root_gosid, filedata: p.filedata })
    }
}

impl Parser<'_> {
    fn eg(&self, pos: usize) -> Result<ExGuid, String> {
        Ok((self.r.guid_at(pos)?, self.r.u32_at(pos + 16)?))
    }

    /// Turns a compact id into an object id through the current global id table.
    fn cid(&self, raw: u32) -> Option<ObjId> {
        self.table.borrow().get(&(raw >> 8)).map(|g| (*g, raw & 0xFF))
    }

    fn rev_mut(&mut self) -> Result<&mut Rev, String> {
        let (sp, i) = self.currev.ok_or("object declared before any revision")?;
        self.spaces.get_mut(&sp).and_then(|s| s.revs.get_mut(i)).ok_or_else(|| "lost revision".to_string())
    }

    /// The global id table the current revision's dependency ended with (empty when unknown).
    fn dep_rev_table(&self) -> HashMap<u32, Guid> {
        let dep = self.currev.and_then(|(sp, i)| self.spaces.get(&sp).and_then(|s| s.revs.get(i)).map(|r| r.dep));
        let d = dep.zip(self.cur).and_then(|(dep, cur)| {
            let s = self.spaces.get(&cur)?;
            s.revs.get(*s.by_id.get(&dep)?)
        });
        d.map(|d| d.table.clone()).unwrap_or_default()
    }

    /// Reads a file node list and the lists chained behind it.
    fn read_list(&mut self, stp: u64, cb: u64, depth: u32) -> Result<(), String> {
        if depth > MAX_LIST_DEPTH {
            return Err("file node lists are nested too deeply".to_string());
        }
        let outer = self.rev_list_ref.take();
        let mut chain = Vec::new();
        let mut res = self.read_chain(stp, cb, depth, &mut chain);
        // An object space manifest list may hold several revision manifest list references;
        // "all but the last MUST be ignored" (MS-ONESTORE 2.1.6, 2.1.10). Followed while this
        // list is still active, so a reference back to it is caught as a loop.
        // take() first: the reference must be cleared even when the list failed
        if let Some((s, c)) = self.rev_list_ref.take()
            && res.is_ok()
        {
            res = self.read_list(s, c, depth + 1);
        }
        self.rev_list_ref = outer;
        for c in chain {
            self.active.remove(&c);
        }
        res
    }

    fn read_chain(&mut self, mut stp: u64, mut cb: u64, depth: u32, chain: &mut Vec<(u64, u64)>) -> Result<(), String> {
        loop {
            if !self.active.insert((stp, cb)) {
                return Err(format!("file node lists form a loop at offset {stp}"));
            }
            chain.push((stp, cb));
            if cb < MIN_FRAGMENT {
                return Err(format!("file node list at offset {stp} is too short ({cb} bytes)"));
            }
            let end = stp.checked_add(cb).ok_or("file node list runs past the end of the file")?;
            if self.r.u64_at(us(stp)?)? != FRAGMENT_HEADER_MAGIC {
                return Err(format!("no file node list at offset {stp}"));
            }
            if self.r.u64_at(us(end - 8)?)? != FRAGMENT_FOOTER_MAGIC {
                return Err(format!("file node list at offset {stp} has no valid end marker"));
            }
            // nodes run from after the header (16 bytes) up to nextFragment (the last 20 bytes)
            let tail = end - 20;
            let mut pos = stp + 16;
            while pos.saturating_add(4) <= tail {
                let hdr = self.r.u32_at(us(pos)?)?;
                let fid = hdr & 0x3FF;
                let size = ((hdr >> 10) & 0x1FFF) as u64;
                if fid == 0 || fid == 0xFF || size < 4 {
                    break;
                }
                self.nodes += 1;
                if self.nodes > MAX_NODES {
                    return Err("too many file nodes".to_string());
                }
                self.node(fid, us(pos.saturating_add(4))?, (hdr >> 23) & 3, (hdr >> 25) & 3, depth)?;
                pos = pos.saturating_add(size);
            }
            let tail = us(tail)?;
            let nstp = self.r.u64_at(tail)?;
            let ncb = self.r.u32_at(tail + 8)? as u64;
            if nstp == NIL64 {
                return Ok(());
            }
            stp = nstp;
            cb = ncb;
        }
    }

    fn node(&mut self, fid: u32, pos: usize, stp_f: u32, cb_f: u32, depth: u32) -> Result<(), String> {
        let r = self.r;
        match fid {
            0x04 => self.root_gosid = Some(self.eg(pos)?), // ObjectSpaceManifestRootFND
            // ObjectSpaceManifestListReference, RevisionManifestListReference,
            // ObjectGroupListReference, FileDataStoreListReference
            0x08 | 0x10 | 0xB0 | 0x90 => {
                let (s, c, _) = fcr(&r, pos, stp_f, cb_f)?;
                match (c, fid) {
                    (0, _) => {} // fcrZero or fcrNil: no list
                    // RevisionManifestListReference: followed at the end of the list (see read_list)
                    (_, 0x10) => self.rev_list_ref = Some((s, c)),
                    _ => self.read_list(s, c, depth + 1)?,
                }
            }
            0x0C => {
                // ObjectSpaceManifestListStartFND
                let g = self.eg(pos)?;
                self.spaces.entry(g).or_default();
                self.cur = Some(g);
            }
            0x1B | 0x1E | 0x1F => {
                // RevisionManifestStart4/6/7
                let rid = self.eg(pos)?;
                let dep = self.eg(pos + 20)?;
                let mut ctx_nil = true;
                if fid == 0x1F {
                    ctx_nil = self.eg(pos + 20 + 20 + 6)?.0 == [0u8; 16];
                }
                let cur = self.cur.ok_or("revision outside of an object space")?;
                let sb = self.spaces.entry(cur).or_default();
                let rev = Rev { dep, table: HashMap::new(), data: RevData { ctx_nil, ..RevData::default() } };
                sb.revs.push(rev);
                let i = sb.revs.len() - 1;
                sb.by_id.insert(rid, i);
                sb.order.push(rid);
                self.currev = Some((cur, i));
                self.table = Rc::new(RefCell::new(HashMap::new()));
                self.saved_tables.clear();
            }
            0x1C => {
                // RevisionManifestEndFND
                let t = self.table.borrow().clone();
                if self.currev.is_some() {
                    self.rev_mut()?.table = t;
                }
            }
            0x21 => {
                // GlobalIdTableStartFNDX
                self.table = Rc::new(RefCell::new(HashMap::new()));
                self.dep_table = self.dep_rev_table();
            }
            0x22 => {
                // GlobalIdTableStart2FND
                self.dep_table = self.dep_rev_table();
                self.table = Rc::new(RefCell::new(self.dep_table.clone()));
            }
            0x24 => {
                // GlobalIdTableEntryFNDX
                let idx = r.u32_at(pos)?;
                let g = r.guid_at(pos + 4)?;
                self.table.borrow_mut().insert(idx, g);
            }
            0x25 => {
                // GlobalIdTableEntry2FNDX
                let a = r.u32_at(pos)?;
                let b = r.u32_at(pos + 4)?;
                if let Some(g) = self.dep_table.get(&a) {
                    self.table.borrow_mut().insert(b, *g);
                }
            }
            0x26 => {
                // GlobalIdTableEntry3FNDX
                let a = r.u32_at(pos)?;
                let n = r.u32_at(pos + 4)?;
                let b = r.u32_at(pos + 8)?;
                // `n` comes from the file: walk the entries that exist, not the `n` indexes.
                for (k, g) in &self.dep_table {
                    if let Some(i) = k.checked_sub(a)
                        && i < n
                        && let Some(bi) = b.checked_add(i)
                    {
                        self.table.borrow_mut().insert(bi, *g);
                    }
                }
            }
            0xB4 => self.saved_tables.push(self.table.clone()), // ObjectGroupStartFND
            0xB8 => {
                // ObjectGroupEndFND
                if let Some(t) = self.saved_tables.pop() {
                    self.table = t;
                }
            }
            0xA4 | 0xA5 | 0xC4 | 0xC5 => {
                // ObjectDeclaration2(Large)RefCount (ReadOnly)
                let (s, _, p) = fcr(&r, pos, stp_f, cb_f)?;
                let raw = r.u32_at(p)?;
                let jcid = r.u32_at(p + 4)?;
                r.u8_at(p + 8)?; // flags
                let oid = self.cid(raw);
                if let Some(oid) = oid.filter(|_| (jcid >> 17) & 1 != 0) {
                    let table = self.table.clone();
                    let cidf = move |raw: u32| -> Option<ObjId> {
                        table.borrow().get(&(raw >> 8)).map(|g| (*g, raw & 0xFF))
                    };
                    // a damaged property set leaves the object with no properties
                    let props = us(s).and_then(|s| read_propset(&r, s, &cidf)).unwrap_or_default();
                    self.rev_mut()?.data.objects.insert(oid, Obj { jcid, props });
                }
            }
            0x72 | 0x73 => {
                // ObjectDeclarationFileData3(Large)RefCountFND
                let raw = r.u32_at(pos)?;
                r.u32_at(pos + 4)?; // jcid
                let oid = self.cid(raw);
                let p = pos + 8 + if fid == 0x72 { 1 } else { 4 };
                let text = |p: usize| -> Result<(String, usize), String> {
                    let cch = r.u32_at(p)? as usize;
                    let start = p + 4;
                    let end = start.saturating_add(cch.saturating_mul(2)).min(r.d.len());
                    let b = r.d.get(start..end).unwrap_or(&[]);
                    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c)).collect();
                    Ok((String::from_utf16_lossy(&units), start.saturating_add(cch.saturating_mul(2))))
                };
                let (rf, p) = text(p)?;
                let (ext, _) = text(p)?;
                if let Some(oid) = oid {
                    self.rev_mut()?.data.filedecl.insert(oid, (rf, ext));
                }
            }
            0x59 => {
                // RootObjectReference2FNDX
                let raw = r.u32_at(pos)?;
                let role = r.u32_at(pos + 4)?;
                if let Some(oid) = self.cid(raw) {
                    self.rev_mut()?.data.roots.insert(role, oid);
                }
            }
            0x5A => {
                // RootObjectReference3FND
                let oid = self.eg(pos)?;
                let role = r.u32_at(pos + 20)?;
                self.rev_mut()?.data.roots.insert(role, oid);
            }
            0x94 => {
                // FileDataStoreObjectReferenceFND
                let (s, _, p) = fcr(&r, pos, stp_f, cb_f)?;
                let g = r.guid_at(p)?;
                let s = us(s)?;
                let length = r.u64_at(s.checked_add(16).ok_or("bad file data offset")?)?;
                let start = s.saturating_add(36).min(r.d.len());
                let end = (start as u64).saturating_add(length).min(r.d.len() as u64) as usize;
                self.filedata.insert(guid_string(&g), r.d[start..end].to_vec());
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, s).unwrap()
    }

    #[test]
    fn fcr_formats() {
        // field widths in bytes per format 0..3
        let stp_len = [8usize, 4, 2, 4];
        let cb_len = [4usize, 8, 1, 2];
        for stp_f in 0..4u32 {
            for cb_f in 0..4u32 {
                let mut buf = vec![0xEEu8; 3];
                let mut stp_raw = vec![0x10u8, 0, 0, 0, 0, 0, 0, 0];
                stp_raw.truncate(stp_len[stp_f as usize]);
                let mut cb_raw = vec![0x05u8, 0, 0, 0, 0, 0, 0, 0];
                cb_raw.truncate(cb_len[cb_f as usize]);
                buf.extend(&stp_raw);
                buf.extend(&cb_raw);
                let r = Rd::new(&buf);
                let (stp, cb, next) = fcr(&r, 3, stp_f, cb_f).unwrap();
                assert_eq!(stp, if stp_f >= 2 { 0x10 * 8 } else { 0x10 }, "stp_f {stp_f}");
                assert_eq!(cb, if cb_f >= 2 { 5 * 8 } else { 5 }, "cb_f {cb_f}");
                assert_eq!(next, 3 + stp_len[stp_f as usize] + cb_len[cb_f as usize]);
            }
        }
    }

    fn cid(raw: u32) -> Option<ObjId> {
        Some(([raw as u8; 16], raw & 0xFF))
    }

    #[test]
    fn propset_types() {
        let mut b: Vec<u8> = Vec::new();
        // oid stream: 3 ids (1 for type 8, 2 for type 9); no osid stream (bit 31), not extended
        b.extend((3u32 | 1 << 31).to_le_bytes());
        for raw in [0x101u32, 0x202, 0x303] {
            b.extend(raw.to_le_bytes());
        }
        const P_NODATA: u32 = 0x0400_0001; // type 1 no data
        const P_BOOL: u32 = 0x0800_0002; // type 2 bool (bit 31 set below)
        const P_B1: u32 = 0x0C00_0003; // type 3 (1 byte)
        const P_B4: u32 = 0x1400_0005; // type 5 (4 bytes)
        const P_BLOB: u32 = 0x1C00_0007; // type 7 (length prefixed)
        const P_ONE: u32 = 0x2000_0008; // type 8 (one oid)
        const P_MANY: u32 = 0x2400_0009; // type 9 (cnt oids)
        const P_ARR: u32 = 0x4000_0010; // type 0x10 array
        const P_SET: u32 = 0x4400_0011; // type 0x11 nested set
        const P_INNER: u32 = 0x0C00_0042;
        const P_FALSE: u32 = 0x0800_0004; // type 2 bool, bit 31 clear
        let prids = [P_NODATA, P_BOOL | 0x8000_0000, P_FALSE, P_B1, P_B4, P_BLOB, P_ONE, P_MANY, P_ARR, P_SET];
        b.extend((prids.len() as u16).to_le_bytes());
        for p in prids {
            b.extend(p.to_le_bytes());
        }
        b.push(0xAB); // type 3
        b.extend([1, 2, 3, 4]); // type 5
        b.extend(3u32.to_le_bytes()); // type 7 cb = 3
        b.extend([7, 8, 9]);
        // type 8 consumes oid 0x101 without data bytes
        b.extend(2u32.to_le_bytes()); // type 9 cnt = 2 -> 0x202, 0x303
        b.extend(2u32.to_le_bytes()); // array of two
        b.extend(0x1400_0000u32.to_le_bytes()); // element type 5
        b.extend([10, 0, 0, 0]);
        b.extend([11, 0, 0, 0]);
        b.extend(1u16.to_le_bytes()); // nested set with one type-3 value
        b.extend(P_INNER.to_le_bytes());
        b.push(0x55);

        let r = Rd::new(&b);
        let ps = read_propset(&r, 0, &cid).unwrap();
        assert_eq!(ps.get(P_NODATA), Some(&PropValue::NoData));
        assert_eq!(ps.get(P_BOOL), Some(&PropValue::Bool(true)));
        assert!(ps.get_bool(P_BOOL));
        assert_eq!(ps.get(P_FALSE), Some(&PropValue::Bool(false)));
        assert!(!ps.get_bool(P_FALSE));
        assert_eq!(ps.get_bytes(P_B1), Some(&[0xABu8][..]));
        assert_eq!(ps.get_bytes(P_B4), Some(&[1u8, 2, 3, 4][..]));
        assert_eq!(ps.get_bytes(P_BLOB), Some(&[7u8, 8, 9][..]));
        assert_eq!(ps.get_ids(P_ONE), &[cid(0x101)]);
        assert_eq!(ps.get_ids(P_MANY), &[cid(0x202), cid(0x303)]);
        assert_eq!(ps.get_array(P_ARR), &[PropValue::Bytes(vec![10, 0, 0, 0]), PropValue::Bytes(vec![11, 0, 0, 0])]);
        match ps.get(P_SET) {
            Some(PropValue::Set(s)) => assert_eq!(s.get_bytes(P_INNER), Some(&[0x55u8][..])),
            other => panic!("expected nested set, got {other:?}"),
        }
        assert!(!ps.get_bool(P_NODATA));
        assert!(ps.get_ids(P_NODATA).is_empty());
    }

    #[test]
    fn times() {
        assert_eq!(time32(&[0, 0, 0, 0]), None);
        assert_eq!(time32(&60u32.to_le_bytes()), Some(dt(1980, 1, 1, 0, 1, 0)));
        assert_eq!(time32(&[1, 2]), None);
        assert_eq!(filetime(&116_444_736_000_000_000u64.to_le_bytes()), Some(dt(1970, 1, 1, 0, 0, 0)));
        assert_eq!(filetime(&0u64.to_le_bytes()), None);
        assert_eq!(filetime(&u64::MAX.to_le_bytes()), None);
    }

    #[test]
    fn guid_text() {
        let g: Guid = [0xE4, 0x52, 0x5C, 0x7B, 0x8C, 0xD8, 0xA7, 0x4D, 0xAE, 0xB1, 0x53, 0x78, 0xD0, 0x29, 0x96, 0xD3];
        assert_eq!(guid_string(&g), "7b5c52e4-d88c-4da7-aeb1-5378d02996d3");
    }

    /// Property set with no ids and one property `prid`, followed by `body`.
    fn one_prop(prid: u32, body: &[u8]) -> Vec<u8> {
        let mut b = (1u32 << 31).to_le_bytes().to_vec();
        b.extend(1u16.to_le_bytes());
        b.extend(prid.to_le_bytes());
        b.extend(body);
        b
    }

    #[test]
    fn deeply_nested_array_is_err() {
        let mut body = Vec::new();
        for _ in 0..200 {
            body.extend(1u32.to_le_bytes()); // one element
            body.extend(0x4000_0000u32.to_le_bytes()); // element type 0x10 again
        }
        let b = one_prop(0x4000_0010, &body);
        assert!(read_propset(&Rd::new(&b), 0, &cid).is_err());
    }

    #[test]
    fn oversized_zero_width_array_is_err() {
        // 100000 "no data" elements
        let mut body = 100_000u32.to_le_bytes().to_vec();
        body.extend(0x0400_0000u32.to_le_bytes());
        let b = one_prop(0x4000_0010, &body);
        assert!(read_propset(&Rd::new(&b), 0, &cid).is_err());
        // 1000 oid elements but the oid stream is empty
        let mut body = 1000u32.to_le_bytes().to_vec();
        body.extend(0x2000_0000u32.to_le_bytes());
        let b = one_prop(0x4000_0010, &body);
        assert!(read_propset(&Rd::new(&b), 0, &cid).is_err());
        // 1000 four-byte elements but only a few bytes follow
        let mut body = 1000u32.to_le_bytes().to_vec();
        body.extend(0x1400_0000u32.to_le_bytes());
        let b = one_prop(0x4000_0010, &body);
        assert!(read_propset(&Rd::new(&b), 0, &cid).is_err());
    }

    #[test]
    fn nested_arrays_over_the_value_budget_are_err() {
        // 100 arrays of 65536 "no data" elements each: every array is within its own limit,
        // together they are 6.5 million values
        let mut body = 100u32.to_le_bytes().to_vec();
        body.extend(0x4000_0000u32.to_le_bytes()); // elements are arrays
        for _ in 0..100 {
            body.extend(65_536u32.to_le_bytes());
            body.extend(0x0400_0000u32.to_le_bytes());
        }
        let b = one_prop(0x4000_0010, &body);
        let e = read_propset(&Rd::new(&b), 0, &cid).unwrap_err();
        assert!(e.contains("values"), "{e}");
        // 10 such arrays fit
        let mut body = 10u32.to_le_bytes().to_vec();
        body.extend(0x4000_0000u32.to_le_bytes());
        for _ in 0..10 {
            body.extend(65_536u32.to_le_bytes());
            body.extend(0x0400_0000u32.to_le_bytes());
        }
        let b = one_prop(0x4000_0010, &body);
        assert!(read_propset(&Rd::new(&b), 0, &cid).is_ok());
    }

    #[test]
    fn utf16_text() {
        assert_eq!(u16s(&[b'h', 0, b'i', 0, 0, 0]), "hi");
        assert_eq!(u16s(&[]), "");
    }

    #[test]
    fn short_buffer_is_err() {
        let b = [1u8, 2, 3];
        let r = Rd::new(&b);
        assert!(r.u8_at(3).is_err());
        assert!(r.u16_at(2).is_err());
        assert!(r.u32_at(0).is_err());
        assert!(r.u64_at(0).is_err());
        assert!(r.bytes(1, 3).is_err());
        assert!(r.bytes(usize::MAX, 2).is_err());
        assert!(r.guid_at(0).is_err());
        for (s, c) in [(0, 0), (1, 1), (2, 2), (3, 3)] {
            assert!(fcr(&r, 2, s, c).is_err());
        }
        assert!(read_propset(&r, 0, &cid).is_err());
        assert!(read_propset(&r, 10, &cid).is_err());
    }

    // --- file nodes, object spaces, revisions ---

    fn oid(n: u8) -> ObjId {
        ([n; 16], 0)
    }

    /// An object whose only properties are an optional `P_MOD32` stamp and a marker byte.
    fn marked(stamp: Option<u32>, marker: u8) -> Obj {
        let mut m = HashMap::new();
        if let Some(s) = stamp {
            m.insert(P_MOD32 & 0x7FFF_FFFF, PropValue::Bytes(s.to_le_bytes().to_vec()));
        }
        m.insert(P_PAGE_LEVEL & 0x7FFF_FFFF, PropValue::Bytes(vec![marker]));
        Obj { jcid: J_PAGE, props: PropSet(m) }
    }

    fn marker_of(o: &Obj) -> u8 {
        o.props.get_bytes(P_PAGE_LEVEL).unwrap()[0]
    }

    fn rev(objs: Vec<(ObjId, Obj)>, roots: Vec<(u32, ObjId)>, ctx_nil: bool) -> RevData {
        RevData { objects: objs.into_iter().collect(), filedecl: HashMap::new(), roots: roots.into_iter().collect(), ctx_nil }
    }

    #[test]
    fn finalize_latest_stamp_wins() {
        let a = rev(vec![(oid(1), marked(Some(200), 1)), (oid(9), marked(None, 10))], vec![(1, oid(1))], true);
        let b = rev(vec![(oid(1), marked(Some(100), 2)), (oid(9), marked(None, 20))], vec![(1, oid(2))], true);
        let sp = finalize(vec![a, b]);
        assert_eq!(marker_of(&sp.objects[&oid(1)]), 1);
        assert_eq!(marker_of(&sp.objects[&oid(9)]), 10);
        assert_eq!(sp.roots[&1], oid(1));
    }

    #[test]
    fn finalize_ties_keep_file_order() {
        let a = rev(vec![(oid(1), marked(Some(100), 1))], vec![(1, oid(1))], true);
        let b = rev(vec![(oid(1), marked(Some(100), 2))], vec![(1, oid(2))], true);
        let sp = finalize(vec![a, b]);
        assert_eq!(marker_of(&sp.objects[&oid(1)]), 2);
        assert_eq!(sp.roots[&1], oid(2));
    }

    #[test]
    fn finalize_ignores_non_nil_context() {
        let a = rev(vec![(oid(1), marked(Some(300), 1))], vec![(1, oid(1))], false);
        let b = rev(vec![(oid(1), marked(Some(100), 2))], vec![(1, oid(2))], true);
        let sp = finalize(vec![a, b]);
        assert_eq!(marker_of(&sp.objects[&oid(1)]), 2);
        assert_eq!(sp.roots[&1], oid(2));
        let only = finalize(vec![rev(vec![(oid(1), marked(Some(1), 1))], vec![], false)]);
        assert!(only.objects.is_empty() && only.roots.is_empty());
    }

    fn header() -> Vec<u8> {
        let mut h = vec![0xE4, 0x52, 0x5C, 0x7B];
        h.extend([0u8; 12]);
        h
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(OneFile::parse(&[]).is_err());
        assert_eq!(OneFile::parse(&[0u8; 200]).err().unwrap(), "not a .one section file");
        let mut d = header();
        d.extend([0xFFu8; 300]);
        assert!(OneFile::parse(&d).is_err());
        let mut x: u32 = 12345;
        let buf: Vec<u8> = (0..600)
            .map(|_| {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 24) as u8
            })
            .collect();
        for n in 0..=buf.len() {
            let _ = OneFile::parse(&buf[..n]);
            // the same bytes behind a valid header exercise the node walker
            let mut d = header();
            d.extend(&buf[..n]);
            let _ = OneFile::parse(&d);
        }
    }

    #[test]
    fn node_list_cycle_is_an_error() {
        // header, then a list at 184 whose "next" pointer points to itself
        let mut d = file_with_root(184, 100);
        d.extend(fragment_to(&[], 100, 184, 100));
        let err = OneFile::parse(&d).err().unwrap();
        assert!(err.contains("loop"), "{err}");
    }

    /// File header with the root file node list reference, padded to offset `stp`.
    fn file_with_root(stp: u64, cb: u32) -> Vec<u8> {
        let mut d = header();
        d.resize(172, 0);
        d.extend(stp.to_le_bytes());
        d.extend(cb.to_le_bytes());
        d.resize(stp as usize, 0);
        d
    }

    /// A file node list fragment of `cb` bytes holding `nodes`, with no next fragment.
    fn fragment(nodes: &[u8], cb: usize) -> Vec<u8> {
        fragment_to(nodes, cb, NIL64, 0)
    }

    /// A file node list fragment of `cb` bytes holding `nodes`, followed by the fragment at `next`.
    fn fragment_to(nodes: &[u8], cb: usize, next: u64, next_cb: u32) -> Vec<u8> {
        let mut f = FRAGMENT_HEADER_MAGIC.to_le_bytes().to_vec();
        f.extend([0u8; 8]); // list id, fragment sequence
        f.extend(nodes);
        f.resize(cb - 20, 0);
        f.extend(next.to_le_bytes());
        f.extend(next_cb.to_le_bytes());
        f.extend(FRAGMENT_FOOTER_MAGIC.to_le_bytes());
        f
    }

    #[test]
    fn fragment_markers_and_size_are_checked() {
        // a valid empty root list parses
        let mut ok = file_with_root(184, 36);
        ok.extend(fragment(&[], 36));
        assert!(OneFile::parse(&ok).is_ok());
        // sizes below 36 bytes, including those where the footer would overlap the header
        for cb in [0u32, 19, 20, 35] {
            let mut d = file_with_root(184, cb);
            d.extend(fragment(&[], 36));
            let err = OneFile::parse(&d).err().unwrap();
            assert!(err.contains("too short"), "cb {cb}: {err}");
        }
        // wrong header marker
        let mut d = ok.clone();
        d[184] ^= 1;
        let err = OneFile::parse(&d).err().unwrap();
        assert!(err.contains("no file node list"), "{err}");
        // wrong footer marker
        let mut d = ok.clone();
        let last = d.len() - 1;
        d[last] ^= 1;
        let err = OneFile::parse(&d).err().unwrap();
        assert!(err.contains("end marker"), "{err}");
        // a next fragment pointing at bytes that are not a fragment
        let mut d = file_with_root(184, 36);
        d.extend(fragment_to(&[], 36, 220, 36));
        d.extend([0u8; 36]);
        let err = OneFile::parse(&d).err().unwrap();
        assert!(err.contains("no file node list at offset 220"), "{err}");
    }

    /// A list reference node (8-byte position, 4-byte size).
    fn ref_node(fid: u32, stp: u64, cb: u32) -> Vec<u8> {
        let mut n = (fid | (16 << 10) | (2 << 27)).to_le_bytes().to_vec();
        n.extend(stp.to_le_bytes());
        n.extend(cb.to_le_bytes());
        n
    }

    /// A file whose one object space manifest list holds the given revision manifest list
    /// references (fid 0x10). Lists: root at 200, object space at 300, empty list at 400.
    fn file_with_rev_refs(refs: &[(u64, u32)]) -> Vec<u8> {
        const LEN: usize = 100;
        let mut d = file_with_root(200, LEN as u32);
        d.extend(fragment(&ref_node(0x08, 300, LEN as u32), LEN));
        let mut space = (0x0Cu32 | (24 << 10)).to_le_bytes().to_vec(); // ObjectSpaceManifestListStart
        space.extend([7u8; 16]);
        space.extend(1u32.to_le_bytes());
        for &(stp, cb) in refs {
            space.extend(ref_node(0x10, stp, cb));
        }
        d.extend(fragment(&space, LEN));
        d.extend(fragment(&[], LEN));
        d
    }

    #[test]
    fn only_the_last_revision_manifest_list_reference_is_followed() {
        // zero reference before the real one (written by OneNote)
        assert!(OneFile::parse(&file_with_rev_refs(&[(0, 0), (400, 100)])).is_ok());
        // stale reference to garbage before the real one
        assert!(OneFile::parse(&file_with_rev_refs(&[(100_000, 64), (400, 100)])).is_ok());
        // the last reference is the one that is read
        assert!(OneFile::parse(&file_with_rev_refs(&[(400, 100), (100_000, 64)])).is_err());
        // a lone zero reference means no revisions
        assert!(OneFile::parse(&file_with_rev_refs(&[(0, 0)])).is_ok());
    }

    #[test]
    fn global_id_entry3_is_bounded_by_the_table() {
        // a = 5, b = 50; `n` = 4, or 4 billion (which would otherwise mean 4 billion lookups)
        for n in [4u32, u32::MAX] {
            let mut buf = Vec::new();
            for v in [5u32, n, 50] {
                buf.extend(v.to_le_bytes());
            }
            let mut p = Parser {
                r: Rd::new(&buf),
                spaces: HashMap::new(),
                root_gosid: None,
                filedata: HashMap::new(),
                cur: None,
                currev: None,
                table: Rc::new(RefCell::new(HashMap::new())),
                saved_tables: Vec::new(),
                dep_table: HashMap::from([(5u32, [1u8; 16]), (7, [2; 16]), (100, [3; 16])]),
                active: HashSet::new(),
                nodes: 0,
                rev_list_ref: None,
            };
            let t = std::time::Instant::now();
            p.node(0x26, 0, 0, 0, 0).unwrap();
            assert!(t.elapsed().as_secs() < 5);
            let tab = p.table.borrow();
            assert_eq!(tab.get(&50), Some(&[1u8; 16]));
            assert_eq!(tab.get(&52), Some(&[2u8; 16]));
            assert_eq!(tab.get(&145).is_some(), n == u32::MAX, "entry 100 is in range only for a huge n");
        }
    }

    #[test]
    #[ignore]
    fn real_sample() {
        let path = std::env::var("ONENOTE_SAMPLE").expect("set ONENOTE_SAMPLE to a .one file");
        let data = std::fs::read(&path).unwrap();
        let f = OneFile::parse(&data).unwrap();
        let g = f.root_gosid.expect("root_gosid");
        let sp = f.spaces.get(&g).expect("root space");
        assert!(sp.roots.contains_key(&1), "root space has no role 1 root");
        eprintln!("{path}: {} spaces, {} file data, root objects {}", f.spaces.len(), f.filedata.len(), sp.objects.len());
    }
}
