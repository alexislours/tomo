use std::collections::BTreeMap;

use crate::formats::lms::{
    self, Header, LabelTable, Section, byte, rd_u16, rd_u32, u16_len, u32_len,
};
use crate::{Error, Result};

pub const MSBP_MAGIC: [u8; 8] = *b"MsgPrjBn";

#[must_use]
pub(crate) fn type_name(t: u8) -> &'static str {
    match t {
        0 => "u8",
        1 => "u16",
        2 => "u32",
        3 => "i8",
        4 => "i16",
        5 => "i32",
        6 => "f32",
        7 => "f64",
        8 => "opt_string",
        9 => "enum",
        _ => "raw",
    }
}

#[must_use]
pub(crate) fn type_from_name(s: &str) -> Option<u8> {
    Some(match s {
        "u8" => 0,
        "u16" => 1,
        "u32" => 2,
        "i8" => 3,
        "i16" => 4,
        "i32" => 5,
        "f32" => 6,
        "f64" => 7,
        "opt_string" => 8,
        "enum" => 9,
        _ => return None,
    })
}

#[must_use]
fn type_size(ty: u8) -> usize {
    match ty {
        0 | 3 | 9 => 1,
        1 | 4 => 2,
        2 | 5 | 6 => 4,
        7 => 8,
        _ => 0,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    U8(u8),
    U16(u16),
    U32(u32),
    I8(i8),
    I16(i16),
    I32(i32),
    F32(f32),
    F64(f64),
    Enum { index: u8, label: Option<String> },
}

fn decode_attr_value(b: &[u8], off: usize, ty: u8, list: &[String]) -> Option<AttrValue> {
    let need = type_size(ty);
    if need == 0 || off.checked_add(need).is_none_or(|end| end > b.len()) {
        return None;
    }
    let u16v = || u16::from_le_bytes([b[off], b[off + 1]]);
    let u32v = || u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
    Some(match ty {
        0 => AttrValue::U8(b[off]),
        3 => AttrValue::I8(b[off].cast_signed()),
        1 => AttrValue::U16(u16v()),
        4 => AttrValue::I16(u16v().cast_signed()),
        2 => AttrValue::U32(u32v()),
        5 => AttrValue::I32(u32v().cast_signed()),
        6 => AttrValue::F32(f32::from_bits(u32v())),
        7 => AttrValue::F64(f64::from_bits(u64::from_le_bytes([
            b[off],
            b[off + 1],
            b[off + 2],
            b[off + 3],
            b[off + 4],
            b[off + 5],
            b[off + 6],
            b[off + 7],
        ]))),
        9 => {
            let index = b[off];
            AttrValue::Enum {
                index,
                label: list.get(index as usize).cloned(),
            }
        }
        _ => return None,
    })
}

/// A named color from the project's color palette.
#[derive(Debug, Clone)]
pub struct Color {
    pub(crate) name: Option<String>,
    pub(crate) rgba: [u8; 4],
}

impl Color {
    #[must_use]
    pub fn new(name: Option<String>, rgba: [u8; 4]) -> Self {
        Self { name, rgba }
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn set_name(&mut self, name: Option<String>) {
        self.name = name;
    }

    #[must_use]
    pub fn rgba(&self) -> [u8; 4] {
        self.rgba
    }

    pub fn set_rgba(&mut self, rgba: [u8; 4]) {
        self.rgba = rgba;
    }
}

/// A message attribute definition (its type and where its value is stored).
#[derive(Debug, Clone)]
pub struct Attribute {
    pub(crate) name: Option<String>,
    pub(crate) ty: u8,
    pub(crate) list_index: u16,
    pub(crate) offset: u32,
}

impl Attribute {
    #[must_use]
    pub fn new(name: Option<String>, ty: u8, list_index: u16, offset: u32) -> Self {
        Self {
            name,
            ty,
            list_index,
            offset,
        }
    }

    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn set_name(&mut self, name: Option<String>) {
        self.name = name;
    }

    #[must_use]
    pub fn ty(&self) -> u8 {
        self.ty
    }

    pub fn set_ty(&mut self, ty: u8) {
        self.ty = ty;
    }

    #[must_use]
    pub fn type_name(&self) -> &'static str {
        type_name(self.ty)
    }

    #[must_use]
    pub fn list_index(&self) -> u16 {
        self.list_index
    }

    pub fn set_list_index(&mut self, list_index: u16) {
        self.list_index = list_index;
    }

    #[must_use]
    pub fn offset(&self) -> u32 {
        self.offset
    }

    pub fn set_offset(&mut self, offset: u32) {
        self.offset = offset;
    }
}

/// A named group of control tags.
#[derive(Debug, Clone)]
pub struct TagGroup {
    pub(crate) id: u16,
    pub(crate) tag_indices: Vec<u16>,
    pub(crate) name: String,
}

impl TagGroup {
    #[must_use]
    pub fn new(id: u16, tag_indices: Vec<u16>, name: impl Into<String>) -> Self {
        Self {
            id,
            tag_indices,
            name: name.into(),
        }
    }

    #[must_use]
    pub fn id(&self) -> u16 {
        self.id
    }

    pub fn set_id(&mut self, id: u16) {
        self.id = id;
    }

    #[must_use]
    pub fn tag_indices(&self) -> &[u16] {
        &self.tag_indices
    }

    pub fn tag_indices_mut(&mut self) -> &mut Vec<u16> {
        &mut self.tag_indices
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
}

/// A control tag and the parameters it accepts.
#[derive(Debug, Clone)]
pub struct Tag {
    pub(crate) param_indices: Vec<u16>,
    pub(crate) name: String,
}

impl Tag {
    #[must_use]
    pub fn new(param_indices: Vec<u16>, name: impl Into<String>) -> Self {
        Self {
            param_indices,
            name: name.into(),
        }
    }

    #[must_use]
    pub fn param_indices(&self) -> &[u16] {
        &self.param_indices
    }

    pub fn param_indices_mut(&mut self) -> &mut Vec<u16> {
        &mut self.param_indices
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
}

/// A parameter of a control tag.
#[derive(Debug, Clone)]
pub struct TagParam {
    pub(crate) ty: u8,
    pub(crate) pad: u8,
    pub(crate) list_indices: Vec<u16>,
    pub(crate) name: String,
}

impl TagParam {
    #[must_use]
    pub fn new(ty: u8, list_indices: Vec<u16>, name: impl Into<String>) -> Self {
        Self {
            ty,
            pad: 0,
            list_indices,
            name: name.into(),
        }
    }

    #[must_use]
    pub fn ty(&self) -> u8 {
        self.ty
    }

    pub fn set_ty(&mut self, ty: u8) {
        self.ty = ty;
    }

    #[must_use]
    pub fn type_name(&self) -> &'static str {
        type_name(self.ty)
    }

    #[must_use]
    pub fn list_indices(&self) -> &[u16] {
        &self.list_indices
    }

    pub fn list_indices_mut(&mut self) -> &mut Vec<u16> {
        &mut self.list_indices
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: impl Into<String>) {
        self.name = name.into();
    }
}

/// Per-section bookkeeping preserved so a file can be rewritten byte-for-byte.
#[derive(Debug, Clone)]
pub struct SecMeta {
    pub(crate) reserved: [u8; 8],
    pub(crate) padding: u8,
    pub(crate) buckets: u32,
    pub(crate) pad: u16,
}

impl Default for SecMeta {
    fn default() -> Self {
        Self {
            reserved: [0; 8],
            padding: 0xAB,
            buckets: 0,
            pad: 0,
        }
    }
}

impl SecMeta {
    #[must_use]
    pub fn new(reserved: [u8; 8], padding: u8, buckets: u32, pad: u16) -> Self {
        Self {
            reserved,
            padding,
            buckets,
            pad,
        }
    }

    #[must_use]
    pub fn reserved(&self) -> [u8; 8] {
        self.reserved
    }

    pub fn set_reserved(&mut self, reserved: [u8; 8]) {
        self.reserved = reserved;
    }

    #[must_use]
    pub fn padding(&self) -> u8 {
        self.padding
    }

    pub fn set_padding(&mut self, padding: u8) {
        self.padding = padding;
    }

    #[must_use]
    pub fn buckets(&self) -> u32 {
        self.buckets
    }

    pub fn set_buckets(&mut self, buckets: u32) {
        self.buckets = buckets;
    }

    #[must_use]
    pub fn pad(&self) -> u16 {
        self.pad
    }

    pub fn set_pad(&mut self, pad: u16) {
        self.pad = pad;
    }
}

/// A parsed MSBP message project: the palette, attribute, and tag definitions
/// shared by a title's MSBT files.
///
/// A `Msbp` can be turned into a [`Registry`](crate::formats::lms::yaml::Registry)
/// to give MSBT control tags readable names.
#[derive(Debug, Clone)]
pub struct Msbp {
    pub header: Header,
    pub order: Vec<[u8; 4]>,
    pub meta: BTreeMap<[u8; 4], SecMeta>,
    pub colors: Vec<Color>,
    pub attributes: Vec<Attribute>,
    pub attribute_lists: Vec<Vec<String>>,
    pub tag_groups: Vec<TagGroup>,
    pub tags: Vec<Tag>,
    pub tag_params: Vec<TagParam>,
    pub tag_param_lists: Vec<String>,
    pub sources: Vec<String>,
    pub raw: BTreeMap<[u8; 4], Vec<u8>>,
}

fn cstr(b: &[u8], start: usize) -> String {
    if start >= b.len() {
        return String::new();
    }
    let end = b[start..]
        .iter()
        .position(|&c| c == 0)
        .map_or(b.len(), |p| start + p);
    String::from_utf8_lossy(&b[start..end]).into_owned()
}

fn count_offsets(b: &[u8]) -> Result<(u16, Vec<usize>)> {
    if b.len() < 4 {
        return Err(Error::malformed("count/offset block too small"));
    }
    let count = rd_u16(b, 0);
    let mut offs = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        if 4 + i * 4 + 4 > b.len() {
            return Err(Error::malformed("offset out of range"));
        }
        offs.push(rd_u32(b, 4 + i * 4) as usize);
    }
    Ok((count, offs))
}

impl Msbp {
    #[must_use]
    pub fn decode_attributes(&self, record: &[u8]) -> Vec<(Option<String>, AttrValue)> {
        let mut out = Vec::with_capacity(self.attributes.len());
        for a in &self.attributes {
            let off = a.offset as usize;
            if a.ty == 8
                || off
                    .checked_add(type_size(a.ty))
                    .is_none_or(|end| end > record.len())
            {
                continue;
            }
            let list = if a.ty == 9 {
                self.attribute_lists
                    .get(a.list_index as usize)
                    .map_or(&[][..], Vec::as_slice)
            } else {
                &[][..]
            };
            let Some(val) = decode_attr_value(record, off, a.ty, list) else {
                continue;
            };
            out.push((a.name.clone(), val));
        }
        out
    }

    /// Parses an MSBP file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let (header, count) = Header::parse(bytes, MSBP_MAGIC)?;
        let sections = lms::read_sections(bytes, count)?;

        let mut order = Vec::new();
        for s in &sections {
            order.push(s.magic);
        }

        let mut msbp = Self {
            header,
            order,
            meta: BTreeMap::new(),
            colors: Vec::new(),
            attributes: Vec::new(),
            attribute_lists: Vec::new(),
            tag_groups: Vec::new(),
            tags: Vec::new(),
            tag_params: Vec::new(),
            tag_param_lists: Vec::new(),
            sources: Vec::new(),
            raw: BTreeMap::new(),
        };

        let mut clb1: Option<LabelTable> = None;
        let mut alb1: Option<LabelTable> = None;

        for s in &sections {
            let mut m = SecMeta {
                reserved: s.reserved,
                padding: s.padding,
                ..Default::default()
            };
            match &s.magic {
                b"CLR1" => msbp.colors = parse_clr1(&s.body)?,
                b"CLB1" => {
                    let t = lms::read_label_table(&s.body)?;
                    m.buckets = t.buckets;
                    clb1 = Some(t);
                }
                b"ATI2" => msbp.attributes = parse_ati2(&s.body)?,
                b"ALB1" => {
                    let t = lms::read_label_table(&s.body)?;
                    m.buckets = t.buckets;
                    alb1 = Some(t);
                }
                b"ALI2" => {
                    msbp.attribute_lists = parse_ali2(&s.body)?;
                }
                b"TGG2" => {
                    m.pad = rd_u16(&s.body, 2);
                    msbp.tag_groups = parse_tgg2(&s.body)?;
                }
                b"TAG2" => {
                    m.pad = rd_u16(&s.body, 2);
                    msbp.tags = parse_tag2(&s.body)?;
                }
                b"TGP2" => {
                    m.pad = rd_u16(&s.body, 2);
                    msbp.tag_params = parse_tgp2(&s.body)?;
                }
                b"TGL2" => {
                    m.pad = rd_u16(&s.body, 2);
                    let (_, offs) = count_offsets(&s.body)?;
                    for start in offs {
                        msbp.tag_param_lists.push(cstr(&s.body, start));
                    }
                }
                b"CTI1" => {
                    let (_, offs) = count_offsets_u32(&s.body)?;
                    for start in offs {
                        msbp.sources.push(cstr(&s.body, start));
                    }
                }
                other => {
                    msbp.raw.insert(*other, s.body.clone());
                }
            }
            msbp.meta.insert(s.magic, m);
        }

        if let Some(t) = clb1 {
            for (name, idx) in t.entries {
                if let Some(c) = msbp.colors.get_mut(idx as usize) {
                    c.name = Some(name);
                }
            }
        }
        if let Some(t) = alb1 {
            for (name, idx) in t.entries {
                if let Some(a) = msbp.attributes.get_mut(idx as usize) {
                    a.name = Some(name);
                }
            }
        }

        Ok(msbp)
    }

    /// Serializes the project back to the binary MSBP format.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut sections = Vec::with_capacity(self.order.len());
        for magic in &self.order {
            let m = self.meta.get(magic).cloned().unwrap_or_default();
            let body = self.build_section(*magic, &m)?;
            sections.push(Section {
                magic: *magic,
                reserved: m.reserved,
                padding: m.padding,
                body,
            });
        }
        lms::write_file(MSBP_MAGIC, &self.header, &sections)
    }

    fn build_section(&self, magic: [u8; 4], m: &SecMeta) -> Result<Vec<u8>> {
        Ok(match &magic {
            b"CLR1" => {
                let mut b = Vec::new();
                b.extend_from_slice(&u32_len(self.colors.len())?.to_le_bytes());
                for c in &self.colors {
                    b.extend_from_slice(&c.rgba);
                }
                b
            }
            b"CLB1" => {
                let mut sorted = label_entries(self.colors.iter().map(|c| c.name.as_ref()))?;
                sorted.sort_by_key(|e| e.1);
                lms::write_label_table(m.buckets, &sorted)?
            }
            b"ATI2" => {
                let mut b = Vec::new();
                b.extend_from_slice(&u32_len(self.attributes.len())?.to_le_bytes());
                for a in &self.attributes {
                    b.push(a.ty);
                    b.push(0);
                    b.extend_from_slice(&a.list_index.to_le_bytes());
                    b.extend_from_slice(&a.offset.to_le_bytes());
                }
                b
            }
            b"ALB1" => {
                let mut entries = label_entries(self.attributes.iter().map(|a| a.name.as_ref()))?;
                entries.sort_by_key(|e| e.1);
                lms::write_label_table(m.buckets, &entries)?
            }
            b"ALI2" => build_ali2(&self.attribute_lists)?,
            b"TGG2" => build_count_offsets(m.pad, &build_tgg2(&self.tag_groups)?)?,
            b"TAG2" => build_count_offsets(m.pad, &build_tag2(&self.tags)?)?,
            b"TGP2" => build_count_offsets(m.pad, &build_tgp2(&self.tag_params)?)?,
            b"TGL2" => {
                let mut entries = Vec::with_capacity(self.tag_param_lists.len());
                for s in &self.tag_param_lists {
                    let mut e = Vec::new();
                    e.extend_from_slice(s.as_bytes());
                    e.push(0);
                    entries.push(e);
                }
                build_count_offsets_aligned(m.pad, &entries, false)?
            }
            b"CTI1" => {
                let mut entries = Vec::with_capacity(self.sources.len());
                for s in &self.sources {
                    let mut e = Vec::new();
                    e.extend_from_slice(s.as_bytes());
                    e.push(0);
                    entries.push(e);
                }
                build_count_offsets_u32(&entries)?
            }
            other => self
                .raw
                .get(other)
                .cloned()
                .ok_or_else(|| Error::unsupported("unknown MSBP section on rebuild"))?,
        })
    }
}

fn count_offsets_u32(b: &[u8]) -> Result<(u32, Vec<usize>)> {
    if b.len() < 4 {
        return Err(Error::malformed("u32 count/offset block too small"));
    }
    let count = rd_u32(b, 0);
    let n = count as usize;
    if n.checked_mul(4)
        .and_then(|x| x.checked_add(4))
        .is_none_or(|e| e > b.len())
    {
        return Err(Error::malformed("u32 offset table out of range"));
    }
    let mut offs = Vec::with_capacity(n);
    for i in 0..n {
        offs.push(rd_u32(b, 4 + i * 4) as usize);
    }
    Ok((count, offs))
}

fn parse_clr1(b: &[u8]) -> Result<Vec<Color>> {
    let n = rd_u32(b, 0) as usize;
    if n.checked_mul(4)
        .and_then(|x| x.checked_add(4))
        .is_none_or(|e| e > b.len())
    {
        return Err(Error::malformed("CLR1 color table out of range"));
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = 4 + i * 4;
        out.push(Color {
            name: None,
            rgba: [byte(b, o), byte(b, o + 1), byte(b, o + 2), byte(b, o + 3)],
        });
    }
    Ok(out)
}

fn parse_ati2(b: &[u8]) -> Result<Vec<Attribute>> {
    let n = rd_u32(b, 0) as usize;
    if n.checked_mul(8)
        .and_then(|x| x.checked_add(4))
        .is_none_or(|e| e > b.len())
    {
        return Err(Error::malformed("ATI2 attribute table out of range"));
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = 4 + i * 8;
        out.push(Attribute {
            name: None,
            ty: byte(b, o),
            list_index: rd_u16(b, o + 2),
            offset: rd_u32(b, o + 4),
        });
    }
    Ok(out)
}

fn parse_tgg2(b: &[u8]) -> Result<Vec<TagGroup>> {
    let (_, offs) = count_offsets(b)?;
    let mut out = Vec::with_capacity(offs.len());
    for start in offs {
        let id = rd_u16(b, start);
        let tc = rd_u16(b, start + 2) as usize;
        let tag_indices = (0..tc).map(|j| rd_u16(b, start + 4 + j * 2)).collect();
        let name = cstr(b, start + 4 + tc * 2);
        out.push(TagGroup {
            id,
            tag_indices,
            name,
        });
    }
    Ok(out)
}

fn parse_tag2(b: &[u8]) -> Result<Vec<Tag>> {
    let (_, offs) = count_offsets(b)?;
    let mut out = Vec::with_capacity(offs.len());
    for start in offs {
        let pc = rd_u16(b, start) as usize;
        let param_indices = (0..pc).map(|j| rd_u16(b, start + 2 + j * 2)).collect();
        let name = cstr(b, start + 2 + pc * 2);
        out.push(Tag {
            param_indices,
            name,
        });
    }
    Ok(out)
}

fn parse_tgp2(b: &[u8]) -> Result<Vec<TagParam>> {
    let (_, offs) = count_offsets(b)?;
    let mut out = Vec::with_capacity(offs.len());
    for start in offs {
        let ty = byte(b, start);
        let pad = byte(b, start + 1);
        let (list_indices, name) = if ty == 9 {
            let lc = rd_u16(b, start + 2) as usize;
            let indices = (0..lc).map(|j| rd_u16(b, start + 4 + j * 2)).collect();
            (indices, cstr(b, start + 4 + lc * 2))
        } else {
            (Vec::new(), cstr(b, start + 2))
        };
        out.push(TagParam {
            ty,
            pad,
            list_indices,
            name,
        });
    }
    Ok(out)
}

fn parse_ali2(b: &[u8]) -> Result<Vec<Vec<String>>> {
    let (_, outer) = count_offsets_u32(b)?;
    let mut out = Vec::with_capacity(outer.len());
    let mut ends = outer.clone();
    ends.push(b.len());
    for i in 0..outer.len() {
        if outer[i] > ends[i + 1] || ends[i + 1] > b.len() {
            return Err(Error::malformed("ALI2 sub-block out of range"));
        }
        let sub = &b[outer[i]..ends[i + 1]];
        if sub.is_empty() {
            out.push(Vec::new());
            continue;
        }
        let (_, inner) = count_offsets(sub)?;
        let mut values = Vec::with_capacity(inner.len());
        for start in inner {
            values.push(cstr(sub, start));
        }
        out.push(values);
    }
    Ok(out)
}

fn label_entries<'a>(
    names: impl Iterator<Item = Option<&'a String>>,
) -> Result<Vec<(String, u32)>> {
    let mut out = Vec::new();
    for (i, name) in names.enumerate() {
        if let Some(n) = name {
            out.push((n.clone(), u32_len(i)?));
        }
    }
    Ok(out)
}

fn build_tgg2(groups: &[TagGroup]) -> Result<Vec<Vec<u8>>> {
    let mut entries = Vec::with_capacity(groups.len());
    for g in groups {
        let mut e = Vec::new();
        e.extend_from_slice(&g.id.to_le_bytes());
        e.extend_from_slice(&u16_len(g.tag_indices.len())?.to_le_bytes());
        for t in &g.tag_indices {
            e.extend_from_slice(&t.to_le_bytes());
        }
        e.extend_from_slice(g.name.as_bytes());
        e.push(0);
        entries.push(e);
    }
    Ok(entries)
}

fn build_tag2(tags: &[Tag]) -> Result<Vec<Vec<u8>>> {
    let mut entries = Vec::with_capacity(tags.len());
    for t in tags {
        let mut e = Vec::new();
        e.extend_from_slice(&u16_len(t.param_indices.len())?.to_le_bytes());
        for p in &t.param_indices {
            e.extend_from_slice(&p.to_le_bytes());
        }
        e.extend_from_slice(t.name.as_bytes());
        e.push(0);
        entries.push(e);
    }
    Ok(entries)
}

fn build_tgp2(params: &[TagParam]) -> Result<Vec<Vec<u8>>> {
    let mut entries = Vec::with_capacity(params.len());
    for p in params {
        let mut e = Vec::new();
        e.push(p.ty);
        e.push(p.pad);
        if p.ty == 9 {
            e.extend_from_slice(&u16_len(p.list_indices.len())?.to_le_bytes());
            for l in &p.list_indices {
                e.extend_from_slice(&l.to_le_bytes());
            }
        }
        e.extend_from_slice(p.name.as_bytes());
        e.push(0);
        entries.push(e);
    }
    Ok(entries)
}

fn build_ali2(lists: &[Vec<String>]) -> Result<Vec<u8>> {
    let n = lists.len();
    let mut subs = Vec::with_capacity(n);
    for list in lists {
        let header = 4 + list.len() * 4;
        let mut data = Vec::new();
        let mut offsets = Vec::with_capacity(list.len());
        for s in list {
            offsets.push(u32_len(header + data.len())?);
            data.extend_from_slice(s.as_bytes());
            data.push(0);
        }
        let mut sub = Vec::with_capacity(header + data.len());
        sub.extend_from_slice(&u16_len(list.len())?.to_le_bytes());
        sub.extend_from_slice(&0u16.to_le_bytes());
        for o in offsets {
            sub.extend_from_slice(&o.to_le_bytes());
        }
        sub.extend_from_slice(&data);
        let target = lms::align4(sub.len());
        sub.resize(target, 0);
        subs.push(sub);
    }
    let outer_header = 4 + n * 4;
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(n);
    for sub in &subs {
        offsets.push(u32_len(outer_header + data.len())?);
        data.extend_from_slice(sub);
    }
    let mut out = Vec::with_capacity(outer_header + data.len());
    out.extend_from_slice(&u32_len(n)?.to_le_bytes());
    for o in offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&data);
    Ok(out)
}

fn build_count_offsets(pad: u16, entries: &[Vec<u8>]) -> Result<Vec<u8>> {
    build_count_offsets_aligned(pad, entries, true)
}

fn build_count_offsets_aligned(pad: u16, entries: &[Vec<u8>], align4: bool) -> Result<Vec<u8>> {
    let n = entries.len();
    let header = 4 + n * 4;
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(n);
    for e in entries {
        offsets.push(u32_len(header + data.len())?);
        data.extend_from_slice(e);
        if align4 {
            let target = lms::align4(data.len());
            data.resize(target, 0);
        }
    }
    let mut out = Vec::with_capacity(header + data.len());
    out.extend_from_slice(&u16_len(n)?.to_le_bytes());
    out.extend_from_slice(&pad.to_le_bytes());
    for o in offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&data);
    Ok(out)
}

fn build_count_offsets_u32(entries: &[Vec<u8>]) -> Result<Vec<u8>> {
    let n = entries.len();
    let header = 4 + n * 4;
    let mut data = Vec::new();
    let mut offsets = Vec::with_capacity(n);
    for e in entries {
        offsets.push(u32_len(header + data.len())?);
        data.extend_from_slice(e);
    }
    let mut out = Vec::with_capacity(header + data.len());
    out.extend_from_slice(&u32_len(n)?.to_le_bytes());
    for o in offsets {
        out.extend_from_slice(&o.to_le_bytes());
    }
    out.extend_from_slice(&data);
    Ok(out)
}
