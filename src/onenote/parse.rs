//! Low-level readers for OneNote (.one, MS-ONESTORE / MS-ONE) files: bounds-checked byte
//! access, file chunk references, property sets and the time formats.
//! Every read returns `Err` instead of panicking, whatever the input.

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime};
use std::collections::HashMap;

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
pub const P_HYPERLINK: u32 = 0x0800_1E14;
pub const P_HIDDEN: u32 = 0x0800_1E16;
pub const P_FONT: u32 = 0x1C00_1C0A;
pub const P_PARA_STYLE: u32 = 0x2000_342C;
pub const P_PARA_STYLE_ID: u32 = 0x1C00_345A;
pub const P_LIST_NODES: u32 = 0x2400_1C26;
pub const P_NUMLIST_FMT: u32 = 0x1C00_1C1A;
pub const P_PICTURE: u32 = 0x2000_1C3F;
pub const P_IMG_ALT: u32 = 0x1C00_1E58;
pub const P_IMG_NAME: u32 = 0x1C00_1DD7;
pub const P_EMB_CONT: u32 = 0x2000_1D9B;
pub const P_EMB_NAME: u32 = 0x1C00_1D9C;
pub const P_ROWS: u32 = 0x1400_1D57;
pub const P_COLS: u32 = 0x1400_1D58;
pub const P_OFF_H: u32 = 0x1400_1C14;
pub const P_OFF_V: u32 = 0x1400_1C15;
pub const P_CHILD_GRAPH: u32 = 0x2C00_1D63;
pub const P_META_ABOVE: u32 = 0x2400_3442;
pub const P_CHARSET: u32 = 0x0C00_1D01;
pub const P_NOTETAG_STATES: u32 = 0x4000_3489;

// --- object type ids (jcid) ---
pub const J_SECTION: u32 = 0x0006_0007;
pub const J_PAGESERIES: u32 = 0x0006_0008;
pub const J_PAGE: u32 = 0x0006_000B;
pub const J_OUTLINE: u32 = 0x0006_000C;
pub const J_OE: u32 = 0x0006_000D;
pub const J_RICHTEXT: u32 = 0x0006_000E;
pub const J_IMAGE: u32 = 0x0006_0011;
pub const J_NUMLIST: u32 = 0x0006_0012;
pub const J_TABLE: u32 = 0x0006_0022;
pub const J_ROW: u32 = 0x0006_0023;
pub const J_CELL: u32 = 0x0006_0024;
pub const J_TITLE: u32 = 0x0006_002C;
pub const J_EMBEDDED: u32 = 0x0006_0035;
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
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
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

/// Reads the property set at `pos`: the object, object space and context id streams,
/// then the properties. `cid` turns a raw compact id into an object id.
pub fn read_propset(r: &Rd, pos: usize, cid: &dyn Fn(u32) -> Option<ObjId>) -> Result<PropSet, String> {
    // One id stream: (ids, position after it, extended flag, no-osid flag).
    let stream = |pos: usize| -> Result<(Vec<Option<ObjId>>, usize, bool, bool), String> {
        let v = r.u32_at(pos)?;
        let cnt = (v & 0xFF_FFFF) as usize;
        let raw = r.bytes(pos + 4, cnt * 4)?;
        let ids = raw.chunks_exact(4).map(|c| cid(u32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect();
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
    let mut st = PropState { r, oids, osids, ctxs, cur: [0; 3] };
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
}

impl PropState<'_, '_> {
    /// Takes the next `count` ids of stream `which` (0 oid, 1 osid, 2 context); short streams give fewer.
    fn take(&mut self, which: usize, count: usize) -> Vec<Option<ObjId>> {
        let list = match which {
            0 => &self.oids,
            1 => &self.osids,
            _ => &self.ctxs,
        };
        let start = self.cur[which].min(list.len());
        let end = start.saturating_add(count).min(list.len());
        self.cur[which] = self.cur[which].saturating_add(count);
        list[start..end].to_vec()
    }

    fn prop_set(&mut self, pos: usize, depth: u32) -> Result<(PropSet, usize), String> {
        if depth > MAX_DEPTH {
            return Err("property sets are nested too deeply".to_string());
        }
        let n = self.r.u16_at(pos)? as usize;
        let mut pos = pos + 2;
        let raw = self.r.bytes(pos, n * 4)?;
        let prids: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
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
            8 | 0xA | 0xC => (PropValue::Ids(self.take(((t - 8) / 2) as usize, 1)), pos),
            9 | 0xB | 0xD => {
                let cnt = r.u32_at(pos)? as usize;
                (PropValue::Ids(self.take(((t - 9) / 2) as usize, cnt)), pos + 4)
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
}
