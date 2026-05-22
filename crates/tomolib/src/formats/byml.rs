use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};

use crate::{Error, Result};

const MAX_DEPTH: u32 = 256;

pub const BYML_MAGIC_LE: [u8; 2] = *b"YB";
pub const BYML_MAGIC_BE: [u8; 2] = *b"BY";

const HEADER_SIZE: usize = 0x10;

pub use crate::formats::binio::ByteOrder as Endian;

use crate::formats::binio::align_up;

/// BYML node type tags, as stored in the file.
pub mod node {
    pub const HASH32: u8 = 0x20;
    pub const HASH64: u8 = 0x21;
    pub const STRING: u8 = 0xA0;
    pub const BINARY: u8 = 0xA1;
    pub const BINARY_ALIGN: u8 = 0xA2;
    pub const ARRAY: u8 = 0xC0;
    pub const DICT: u8 = 0xC1;
    pub const STRING_TABLE: u8 = 0xC2;
    pub const BOOL: u8 = 0xD0;
    pub const I32: u8 = 0xD1;
    pub const F32: u8 = 0xD2;
    pub const U32: u8 = 0xD3;
    pub const I64: u8 = 0xD4;
    pub const U64: u8 = 0xD5;
    pub const F64: u8 = 0xD6;
    pub const NULL: u8 = 0xFF;
}

#[must_use]
/// Whether a [`node`] type tag denotes a container (array, dict, or hash map).
pub fn is_container(t: u8) -> bool {
    matches!(t, node::ARRAY | node::DICT | node::HASH32 | node::HASH64)
}

#[must_use]
fn is_long(t: u8) -> bool {
    matches!(t, node::I64 | node::U64 | node::F64)
}

#[must_use]
fn is_non_inline(t: u8) -> bool {
    is_container(t) || is_long(t) || matches!(t, node::BINARY | node::BINARY_ALIGN)
}

/// A BYML value: a scalar, string, binary blob, or a container of further
/// values.
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Bool(bool),
    I32(i32),
    U32(u32),
    F32(f32),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Binary(Vec<u8>),
    BinaryAlign { data: Vec<u8>, align: u32 },
    Array(Vec<Value>),
    Dict(BTreeMap<String, Value>),
    Hash32(BTreeMap<u32, Value>),
    Hash64(BTreeMap<u64, Value>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::I32(a), Self::I32(b)) => a == b,
            (Self::U32(a), Self::U32(b)) => a == b,
            (Self::F32(a), Self::F32(b)) => a.to_bits() == b.to_bits(),
            (Self::I64(a), Self::I64(b)) => a == b,
            (Self::U64(a), Self::U64(b)) => a == b,
            (Self::F64(a), Self::F64(b)) => a.to_bits() == b.to_bits(),
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Binary(a), Self::Binary(b)) => a == b,
            (Self::BinaryAlign { data: a, align: x }, Self::BinaryAlign { data: b, align: y }) => {
                a == b && x == y
            }
            (Self::Array(a), Self::Array(b)) => a == b,
            (Self::Dict(a), Self::Dict(b)) => a == b,
            (Self::Hash32(a), Self::Hash32(b)) => a == b,
            (Self::Hash64(a), Self::Hash64(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for Value {}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Null => {}
            Self::Bool(b) => b.hash(state),
            Self::I32(n) => n.hash(state),
            Self::U32(n) => n.hash(state),
            Self::F32(f) => f.to_bits().hash(state),
            Self::I64(n) => n.hash(state),
            Self::U64(n) => n.hash(state),
            Self::F64(f) => f.to_bits().hash(state),
            Self::String(s) => s.hash(state),
            Self::Binary(b) => b.hash(state),
            Self::BinaryAlign { data, align } => {
                data.hash(state);
                align.hash(state);
            }
            Self::Array(a) => a.hash(state),
            Self::Dict(d) => d.hash(state),
            Self::Hash32(h) => h.hash(state),
            Self::Hash64(h) => h.hash(state),
        }
    }
}

impl Value {
    #[must_use]
    pub(crate) fn node_type(&self) -> u8 {
        match self {
            Self::Null => node::NULL,
            Self::Bool(_) => node::BOOL,
            Self::I32(_) => node::I32,
            Self::U32(_) => node::U32,
            Self::F32(_) => node::F32,
            Self::I64(_) => node::I64,
            Self::U64(_) => node::U64,
            Self::F64(_) => node::F64,
            Self::String(_) => node::STRING,
            Self::Binary(_) => node::BINARY,
            Self::BinaryAlign { .. } => node::BINARY_ALIGN,
            Self::Array(_) => node::ARRAY,
            Self::Dict(_) => node::DICT,
            Self::Hash32(_) => node::HASH32,
            Self::Hash64(_) => node::HASH64,
        }
    }
}

/// A parsed BYML document: its version, byte order, and root [`Value`].
#[derive(Debug, Clone)]
pub struct Byml {
    pub version: u16,
    pub endian: Endian,
    pub root: Value,
}

struct Header {
    endian: Endian,
    version: u16,
    key_off: u32,
    str_off: u32,
    root_off: u32,
}

#[inline]
fn ensure(len: usize, off: usize, need: usize, ctx: &'static str) -> Result<()> {
    if off.checked_add(need).is_none_or(|end| end > len) {
        return Err(Error::malformed(ctx));
    }
    Ok(())
}

fn parse_header(bytes: &[u8]) -> Result<Header> {
    if bytes.len() < HEADER_SIZE {
        return Err(Error::malformed("file too short to be a BYML"));
    }
    let endian = match (bytes[0], bytes[1]) {
        (b'Y', b'B') => Endian::Little,
        (b'B', b'Y') => Endian::Big,
        _ => return Err(Error::bad_magic("BYML")),
    };
    let version = endian.read_u16(bytes, 2, "BYML version")?;
    if !(1..=10).contains(&version) {
        return Err(Error::malformed(format!("invalid BYML version {version}")));
    }
    Ok(Header {
        endian,
        version,
        key_off: endian.read_u32(bytes, 4, "BYML hash-key table offset")?,
        str_off: endian.read_u32(bytes, 8, "BYML string table offset")?,
        root_off: endian.read_u32(bytes, 12, "BYML root offset")?,
    })
}

impl Byml {
    /// Parses a BYML document into an owned tree of [`Value`]s.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let Header {
            endian,
            version,
            key_off,
            str_off,
            root_off,
        } = parse_header(bytes)?;

        let hash_key_table = StringTable::parse(bytes, endian, key_off)?;
        let string_table = StringTable::parse(bytes, endian, str_off)?;

        let parser = Parser {
            bytes,
            endian,
            hash_key_table,
            string_table,
        };

        let root = if root_off == 0 {
            Value::Null
        } else {
            parser.parse_container(root_off as usize, 0)?
        };

        Ok(Self {
            version,
            endian,
            root,
        })
    }

    /// Serializes the document back to the binary BYML format.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let ctx = WriteContext::new(self.endian, &self.root)?;
        ctx.write(self.version, &self.root)
    }
}

/// A zero-copy reader over a BYML buffer, exposing the header and string tables
/// without building an owned [`Value`] tree.
#[derive(Debug)]
pub struct BymlReader<'a> {
    pub bytes: &'a [u8],
    pub endian: Endian,
    pub version: u16,
    pub root_off: u32,
    keys: StringTable<'a>,
    strings: StringTable<'a>,
}

impl<'a> BymlReader<'a> {
    /// Reads the header and string tables of `bytes`, borrowing from it.
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let Header {
            endian,
            version,
            key_off,
            str_off,
            root_off,
        } = parse_header(bytes)?;
        let keys = StringTable::parse(bytes, endian, key_off)?;
        let strings = StringTable::parse(bytes, endian, str_off)?;
        Ok(Self {
            bytes,
            endian,
            version,
            root_off,
            keys,
            strings,
        })
    }

    /// Looks up a dictionary key by its index in the hash-key table.
    pub fn key(&self, idx: u32) -> Result<&'a str> {
        self.keys.get(idx)
    }

    /// Looks up a string by its index in the string table.
    pub fn string(&self, idx: u32) -> Result<&'a str> {
        self.strings.get(idx)
    }
}

#[derive(Debug)]
struct StringTable<'a> {
    entries: Vec<&'a str>,
}

impl<'a> StringTable<'a> {
    fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    fn parse(bytes: &'a [u8], endian: Endian, offset: u32) -> Result<Self> {
        if offset == 0 {
            return Ok(Self::empty());
        }
        let off = offset as usize;
        ensure(bytes.len(), off, 4, "string table header out of range")?;
        let t = bytes[off];
        if t != node::STRING_TABLE {
            return Err(Error::malformed(format!(
                "expected string table (0xC2) at {off:#x}, got {t:#x}"
            )));
        }
        let count = endian.read_u24(bytes, off + 1, "string table count")? as usize;
        let table_end = off
            .checked_add(4)
            .and_then(|n| n.checked_add(4 * (count + 1)))
            .ok_or_else(|| Error::overflow("string table size overflow"))?;
        if table_end > bytes.len() {
            return Err(Error::malformed("string table offsets truncated"));
        }
        let base = off;
        let mut entries: Vec<&'a str> = Vec::with_capacity(count);
        let mut prev_end = endian.read_u32(bytes, off + 4, "string table offset")? as usize;
        for i in 0..count {
            let end_rel =
                endian.read_u32(bytes, off + 4 + 4 * (i + 1), "string table offset")? as usize;
            let start = base
                .checked_add(prev_end)
                .ok_or_else(|| Error::overflow("string table entry offset overflow"))?;
            let end = base
                .checked_add(end_rel)
                .ok_or_else(|| Error::overflow("string table entry offset overflow"))?;
            if end > bytes.len() || start > end {
                return Err(Error::malformed("string table entry out of range"));
            }
            let slice = &bytes[start..end];
            let nul = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
            let s = std::str::from_utf8(&slice[..nul])
                .map_err(|_| Error::invalid_utf8("BYML string table entry"))?;
            entries.push(s);
            prev_end = end_rel;
        }
        Ok(Self { entries })
    }

    fn get(&self, idx: u32) -> Result<&'a str> {
        let i = idx as usize;
        self.entries
            .get(i)
            .copied()
            .ok_or_else(|| Error::out_of_range("string table index", i, self.entries.len()))
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    endian: Endian,
    hash_key_table: StringTable<'a>,
    string_table: StringTable<'a>,
}

impl Parser<'_> {
    fn parse_container(&self, offset: usize, depth: u32) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(Error::malformed(format!(
                "container nesting exceeds {MAX_DEPTH} levels at {offset:#x}"
            )));
        }
        ensure(self.bytes.len(), offset, 4, "container header out of range")?;
        let t = self.bytes[offset];
        let count = self
            .endian
            .read_u24(self.bytes, offset + 1, "container header")? as usize;
        match t {
            node::ARRAY => self.parse_array(offset, count, depth),
            node::DICT => self.parse_dict(offset, count, depth),
            node::HASH32 => self.parse_hash32(offset, count, depth),
            node::HASH64 => self.parse_hash64(offset, count, depth),
            _ => Err(Error::malformed(format!(
                "expected container node at {offset:#x}, got type {t:#x}"
            ))),
        }
    }

    fn parse_value(&self, offset: usize, t: u8) -> Result<Value> {
        ensure(self.bytes.len(), offset, 4, "value node out of range")?;
        let raw = self.endian.read_u32(self.bytes, offset, "value node")?;
        match t {
            node::NULL => Ok(Value::Null),
            node::BOOL => Ok(Value::Bool(raw != 0)),
            node::I32 => Ok(Value::I32(raw.cast_signed())),
            node::U32 => Ok(Value::U32(raw)),
            node::F32 => Ok(Value::F32(f32::from_bits(raw))),
            node::I64 => {
                let o = raw as usize;
                ensure(self.bytes.len(), o, 8, "i64 value out of range")?;
                Ok(Value::I64(
                    self.endian
                        .read_u64(self.bytes, o, "i64 value")?
                        .cast_signed(),
                ))
            }
            node::U64 => {
                let o = raw as usize;
                ensure(self.bytes.len(), o, 8, "u64 value out of range")?;
                Ok(Value::U64(self.endian.read_u64(
                    self.bytes,
                    o,
                    "u64 value",
                )?))
            }
            node::F64 => {
                let o = raw as usize;
                ensure(self.bytes.len(), o, 8, "f64 value out of range")?;
                Ok(Value::F64(f64::from_bits(self.endian.read_u64(
                    self.bytes,
                    o,
                    "f64 value",
                )?)))
            }
            node::STRING => Ok(Value::String(self.string_table.get(raw)?.to_owned())),
            node::BINARY => {
                let o = raw as usize;
                ensure(self.bytes.len(), o, 4, "binary header out of range")?;
                let size = self.endian.read_u32(self.bytes, o, "binary size")? as usize;
                ensure(self.bytes.len(), o + 4, size, "binary data out of range")?;
                Ok(Value::Binary(self.bytes[o + 4..o + 4 + size].to_vec()))
            }
            node::BINARY_ALIGN => {
                let o = raw as usize;
                ensure(self.bytes.len(), o, 8, "binary_align header out of range")?;
                let size = self.endian.read_u32(self.bytes, o, "binary_align size")? as usize;
                let align = self
                    .endian
                    .read_u32(self.bytes, o + 4, "binary_align alignment")?;
                ensure(
                    self.bytes.len(),
                    o + 8,
                    size,
                    "binary_align data out of range",
                )?;
                Ok(Value::BinaryAlign {
                    data: self.bytes[o + 8..o + 8 + size].to_vec(),
                    align,
                })
            }
            _ => Err(Error::malformed(format!(
                "unexpected value node type {t:#x} at {offset:#x}"
            ))),
        }
    }

    fn parse_child(&self, offset: usize, t: u8, depth: u32) -> Result<Value> {
        if is_container(t) {
            let target = self.endian.read_u32(self.bytes, offset, "child offset")?;
            self.parse_container(target as usize, depth + 1)
        } else {
            self.parse_value(offset, t)
        }
    }

    fn parse_array(&self, offset: usize, count: usize, depth: u32) -> Result<Value> {
        let types_off = offset + 4;
        let values_off = align_up(types_off + count, 4);
        ensure(
            self.bytes.len(),
            values_off,
            4 * count,
            "array values out of range",
        )?;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let t = self.bytes[types_off + i];
            let v = self.parse_child(values_off + 4 * i, t, depth)?;
            out.push(v);
        }
        Ok(Value::Array(out))
    }

    fn parse_dict(&self, offset: usize, count: usize, depth: u32) -> Result<Value> {
        let mut out = BTreeMap::new();
        for i in 0..count {
            let e = offset + 4 + 8 * i;
            ensure(self.bytes.len(), e, 8, "dict entry out of range")?;
            let name_idx = self.endian.read_u24(self.bytes, e, "dict entry")?;
            let t = self.bytes[e + 3];
            let name = self.hash_key_table.get(name_idx)?.to_owned();
            let v = self.parse_child(e + 4, t, depth)?;
            out.insert(name, v);
        }
        Ok(Value::Dict(out))
    }

    fn parse_hash32(&self, offset: usize, count: usize, depth: u32) -> Result<Value> {
        let types_off = offset + 4 + 8 * count;
        ensure(
            self.bytes.len(),
            types_off,
            count,
            "hash32 types out of range",
        )?;
        let mut out = BTreeMap::new();
        for i in 0..count {
            let entry = offset + 4 + 8 * i;
            let hash = self.endian.read_u32(self.bytes, entry, "hash32 entry")?;
            let t = self.bytes[types_off + i];
            let v = self.parse_child(entry + 4, t, depth)?;
            out.insert(hash, v);
        }
        Ok(Value::Hash32(out))
    }

    fn parse_hash64(&self, offset: usize, count: usize, depth: u32) -> Result<Value> {
        let types_off = offset + 4 + 12 * count;
        ensure(
            self.bytes.len(),
            types_off,
            count,
            "hash64 types out of range",
        )?;
        let mut out = BTreeMap::new();
        for i in 0..count {
            let entry = offset + 4 + 12 * i;
            let hash = self.endian.read_u64(self.bytes, entry, "hash64 entry")?;
            let t = self.bytes[types_off + i];
            let v = self.parse_child(entry + 8, t, depth)?;
            out.insert(hash, v);
        }
        Ok(Value::Hash64(out))
    }
}

struct StringPool {
    sorted: Vec<String>,
    index: HashMap<String, u32>,
}

impl StringPool {
    fn new() -> Self {
        Self {
            sorted: Vec::new(),
            index: HashMap::new(),
        }
    }

    fn add(&mut self, s: &str) {
        if !self.index.contains_key(s) {
            self.index.insert(s.to_owned(), u32::MAX);
        }
    }

    fn build(&mut self) -> Result<()> {
        let map = std::mem::take(&mut self.index);
        let mut keys: Vec<String> = map.into_keys().collect();
        keys.sort();
        self.index.reserve(keys.len());
        for (i, k) in keys.iter().enumerate() {
            let idx = u32::try_from(i).map_err(|_| Error::overflow("string pool exceeds u32"))?;
            self.index.insert(k.clone(), idx);
        }
        self.sorted = keys;
        Ok(())
    }

    fn get(&self, s: &str) -> Result<u32> {
        self.index
            .get(s)
            .copied()
            .ok_or_else(|| Error::malformed(format!("string `{s}` missing from pool")))
    }

    fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }

    fn len(&self) -> usize {
        self.sorted.len()
    }
}

struct WriteContext {
    endian: Endian,
    keys: StringPool,
    strings: StringPool,
    out: Vec<u8>,
    dedup: HashMap<Value, u32>,
}

#[derive(Debug)]
struct PendingChild {
    container_slot: usize,
    value: Value,
}

impl WriteContext {
    fn new(endian: Endian, root: &Value) -> Result<Self> {
        let mut ctx = Self {
            endian,
            keys: StringPool::new(),
            strings: StringPool::new(),
            out: Vec::new(),
            dedup: HashMap::new(),
        };
        ctx.collect_strings(root);
        ctx.keys.build()?;
        ctx.strings.build()?;
        Ok(ctx)
    }

    fn collect_strings(&mut self, v: &Value) {
        match v {
            Value::String(s) => self.strings.add(s),
            Value::Array(a) => {
                for x in a {
                    self.collect_strings(x);
                }
            }
            Value::Dict(d) => {
                for (k, x) in d {
                    self.keys.add(k);
                    self.collect_strings(x);
                }
            }
            Value::Hash32(h) => {
                for x in h.values() {
                    self.collect_strings(x);
                }
            }
            Value::Hash64(h) => {
                for x in h.values() {
                    self.collect_strings(x);
                }
            }
            _ => {}
        }
    }

    fn align_to(&mut self, a: usize) {
        let new_len = align_up(self.out.len(), a);
        self.out.resize(new_len, 0);
    }

    fn write(mut self, version: u16, root: &Value) -> Result<Vec<u8>> {
        let endian = self.endian;
        match endian {
            Endian::Little => self.out.extend_from_slice(b"YB"),
            Endian::Big => self.out.extend_from_slice(b"BY"),
        }
        endian.put_u16(&mut self.out, version);
        endian.put_u32(&mut self.out, 0);
        endian.put_u32(&mut self.out, 0);
        endian.put_u32(&mut self.out, 0);
        debug_assert_eq!(self.out.len(), HEADER_SIZE);

        if matches!(root, Value::Null) {
            return Ok(self.out);
        }

        if !self.keys.is_empty() {
            let off =
                u32::try_from(self.out.len()).map_err(|_| Error::overflow("output too large"))?;
            endian.write_u32_at(&mut self.out, 4, off);
            self.write_string_table(true)?;
        }
        if !self.strings.is_empty() {
            let off =
                u32::try_from(self.out.len()).map_err(|_| Error::overflow("output too large"))?;
            endian.write_u32_at(&mut self.out, 8, off);
            self.write_string_table(false)?;
        }

        let root_off =
            u32::try_from(self.out.len()).map_err(|_| Error::overflow("output too large"))?;
        endian.write_u32_at(&mut self.out, 12, root_off);
        self.write_container(root)?;
        self.align_to(4);

        Ok(self.out)
    }

    fn write_string_table(&mut self, is_keys: bool) -> Result<()> {
        let endian = self.endian;
        let base = self.out.len();
        self.out.push(node::STRING_TABLE);
        let pool = if is_keys { &self.keys } else { &self.strings };
        let count_u32 =
            u32::try_from(pool.len()).map_err(|_| Error::overflow("string count exceeds u32"))?;
        endian.put_u24(&mut self.out, count_u32);

        let offset_table_pos = self.out.len();
        let table_bytes = 4 * (pool.len() + 1);
        self.out.resize(self.out.len() + table_bytes, 0);

        for (i, s) in pool.sorted.iter().enumerate() {
            let rel = u32::try_from(self.out.len() - base)
                .map_err(|_| Error::overflow("string offset exceeds u32"))?;
            endian.write_u32_at(&mut self.out, offset_table_pos + 4 * i, rel);
            self.out.extend_from_slice(s.as_bytes());
            self.out.push(0);
        }
        let rel = u32::try_from(self.out.len() - base)
            .map_err(|_| Error::overflow("string offset exceeds u32"))?;
        endian.write_u32_at(&mut self.out, offset_table_pos + 4 * pool.len(), rel);
        self.align_to(4);
        Ok(())
    }

    fn write_value_inline(&mut self, v: &Value) -> Result<()> {
        let endian = self.endian;
        match v {
            Value::Null => endian.put_u32(&mut self.out, 0),
            Value::Bool(b) => endian.put_u32(&mut self.out, u32::from(*b)),
            Value::I32(n) => endian.put_u32(&mut self.out, n.cast_unsigned()),
            Value::U32(n) => endian.put_u32(&mut self.out, *n),
            Value::F32(f) => endian.put_u32(&mut self.out, f.to_bits()),
            Value::String(s) => {
                let idx = self.strings.get(s)?;
                endian.put_u32(&mut self.out, idx);
            }
            _ => panic!("write_value_inline called with non-inline type"),
        }
        Ok(())
    }

    fn write_value_non_inline(&mut self, v: &Value) -> Result<()> {
        let endian = self.endian;
        match v {
            Value::I64(n) => endian.put_u64(&mut self.out, n.cast_unsigned()),
            Value::U64(n) => endian.put_u64(&mut self.out, *n),
            Value::F64(f) => endian.put_u64(&mut self.out, f.to_bits()),
            Value::Binary(b) => {
                endian.put_u32(
                    &mut self.out,
                    u32::try_from(b.len())
                        .map_err(|_| Error::overflow("binary size exceeds u32"))?,
                );
                self.out.extend_from_slice(b);
            }
            Value::BinaryAlign { data, align } => {
                endian.put_u32(
                    &mut self.out,
                    u32::try_from(data.len())
                        .map_err(|_| Error::overflow("binary size exceeds u32"))?,
                );
                endian.put_u32(&mut self.out, *align);
                self.out.extend_from_slice(data);
            }
            _ => panic!("write_value_non_inline called with wrong type"),
        }
        Ok(())
    }

    fn write_container(&mut self, v: &Value) -> Result<()> {
        let mut pending: Vec<PendingChild> = Vec::new();
        self.write_container_header(v, &mut pending)?;
        self.flush_pending(pending)
    }

    fn write_item(&mut self, item: &Value, pending: &mut Vec<PendingChild>) -> Result<()> {
        if is_non_inline(item.node_type()) {
            let slot = self.out.len();
            self.endian.put_u32(&mut self.out, 0);
            pending.push(PendingChild {
                container_slot: slot,
                value: item.clone(),
            });
            Ok(())
        } else {
            self.write_value_inline(item)
        }
    }

    fn write_container_header(&mut self, v: &Value, pending: &mut Vec<PendingChild>) -> Result<()> {
        let endian = self.endian;
        match v {
            Value::Array(arr) => {
                self.out.push(node::ARRAY);
                let n = u32::try_from(arr.len())
                    .map_err(|_| Error::overflow("array length exceeds u32"))?;
                endian.put_u24(&mut self.out, n);
                for item in arr {
                    self.out.push(item.node_type());
                }
                self.align_to(4);
                for item in arr {
                    self.write_item(item, pending)?;
                }
            }
            Value::Dict(d) => {
                self.out.push(node::DICT);
                let n =
                    u32::try_from(d.len()).map_err(|_| Error::overflow("dict size exceeds u32"))?;
                endian.put_u24(&mut self.out, n);
                for (k, item) in d {
                    let idx = self.keys.get(k)?;
                    endian.put_u24(&mut self.out, idx);
                    self.out.push(item.node_type());
                    self.write_item(item, pending)?;
                }
            }
            Value::Hash32(h) => {
                self.out.push(node::HASH32);
                let n =
                    u32::try_from(h.len()).map_err(|_| Error::overflow("hash size exceeds u32"))?;
                endian.put_u24(&mut self.out, n);
                for (k, item) in h {
                    endian.put_u32(&mut self.out, *k);
                    self.write_item(item, pending)?;
                }
                for item in h.values() {
                    self.out.push(item.node_type());
                }
                self.align_to(4);
            }
            Value::Hash64(h) => {
                self.out.push(node::HASH64);
                let n =
                    u32::try_from(h.len()).map_err(|_| Error::overflow("hash size exceeds u32"))?;
                endian.put_u24(&mut self.out, n);
                for (k, item) in h {
                    endian.put_u64(&mut self.out, *k);
                    self.write_item(item, pending)?;
                }
                for item in h.values() {
                    self.out.push(item.node_type());
                }
                self.align_to(4);
            }
            _ => return Err(Error::malformed("write_container called on non-container")),
        }
        Ok(())
    }

    fn flush_pending(&mut self, pending: Vec<PendingChild>) -> Result<()> {
        for child in pending {
            if let Some(&cached) = self.dedup.get(&child.value) {
                self.endian
                    .write_u32_at(&mut self.out, child.container_slot, cached);
                continue;
            }
            let t = child.value.node_type();
            if is_container(t) {
                self.align_to(4);
            } else if let Value::BinaryAlign { align, .. } = &child.value {
                let a = *align as usize;
                if a == 0 || !a.is_power_of_two() {
                    return Err(Error::malformed(format!(
                        "binary alignment {align} must be a power of two"
                    )));
                }
                let target = align_up(self.out.len() + 8, a) - 8;
                self.out.resize(target, 0);
            }
            let off = u32::try_from(self.out.len())
                .map_err(|_| Error::overflow("output offset exceeds u32"))?;
            self.endian
                .write_u32_at(&mut self.out, child.container_slot, off);
            self.dedup.insert(child.value.clone(), off);
            if is_container(t) {
                self.write_container(&child.value)?;
            } else {
                self.write_value_non_inline(&child.value)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trip() {
        let b = Byml {
            version: 7,
            endian: Endian::Little,
            root: Value::Null,
        };
        let bytes = b.to_bytes().unwrap();
        assert_eq!(bytes.len(), HEADER_SIZE);
        let parsed = Byml::parse(&bytes).unwrap();
        assert!(matches!(parsed.root, Value::Null));
    }

    #[test]
    fn small_dict_round_trip() {
        let mut d = BTreeMap::new();
        d.insert("Foo".to_string(), Value::I32(42));
        d.insert("Bar".to_string(), Value::String("hello".to_string()));
        let b = Byml {
            version: 7,
            endian: Endian::Little,
            root: Value::Dict(d),
        };
        let bytes = b.to_bytes().unwrap();
        let parsed = Byml::parse(&bytes).unwrap();
        let bytes2 = parsed.to_bytes().unwrap();
        assert_eq!(bytes, bytes2);
    }

    #[test]
    fn rejects_string_table_with_huge_offsets() {
        let mut b = vec![0u8; HEADER_SIZE];
        b[0] = b'Y';
        b[1] = b'B';
        b[2] = 7;
        let key_off = u32::try_from(b.len()).unwrap();
        b[4..8].copy_from_slice(&key_off.to_le_bytes());
        b.push(node::STRING_TABLE);
        b.extend_from_slice(&[1, 0, 0]);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(Byml::parse(&b).is_err());
    }

    #[test]
    fn rejects_value_offset_past_end() {
        let mut d = BTreeMap::new();
        d.insert("K".to_string(), Value::I64(1));
        let bytes = Byml {
            version: 7,
            endian: Endian::Little,
            root: Value::Dict(d),
        }
        .to_bytes()
        .unwrap();
        let truncated = &bytes[..bytes.len() - 4];
        assert!(Byml::parse(truncated).is_err());
    }

    #[test]
    fn hash32_round_trip() {
        let mut h = BTreeMap::new();
        h.insert(0xDEAD_BEEF, Value::U32(1));
        h.insert(0x0000_0001, Value::F32(1.5));
        let b = Byml {
            version: 7,
            endian: Endian::Little,
            root: Value::Hash32(h),
        };
        let bytes = b.to_bytes().unwrap();
        let parsed = Byml::parse(&bytes).unwrap();
        let bytes2 = parsed.to_bytes().unwrap();
        assert_eq!(bytes, bytes2);
    }
}
