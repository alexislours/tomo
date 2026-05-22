use std::collections::{BTreeMap, HashMap};

use saphyr::{LoadableYamlNode, Yaml};

use crate::formats::lms::Header;
use crate::formats::msbp::{
    self, Attribute, Color, Msbp, SecMeta as PSecMeta, Tag, TagGroup, TagParam,
};
use crate::formats::msbt::{Message, Msbt, SecMeta};
use crate::{Error, Result};

use super::registry::Registry;
use super::shared::hex_decode;

fn load(text: &str) -> Result<Vec<Yaml<'_>>> {
    Yaml::load_from_str(text).map_err(|e| Error::malformed(format!("YAML parse: {e}")))
}

fn doc_of<'a>(docs: &'a [Yaml<'a>]) -> Result<&'a Yaml<'a>> {
    docs.first().ok_or_else(|| Error::malformed("empty YAML"))
}

fn get<'a, 'b>(map: &'a Yaml<'b>, key: &str) -> Option<&'a Yaml<'b>> {
    let m = map.as_mapping()?;
    m.iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

fn req<'a, 'b>(map: &'a Yaml<'b>, key: &str) -> Result<&'a Yaml<'b>> {
    get(map, key).ok_or_else(|| Error::malformed(format!("missing key `{key}`")))
}

fn seq_of<'a, 'b>(doc: &'a Yaml<'b>, key: &str) -> &'a [Yaml<'b>] {
    get(doc, key)
        .and_then(Yaml::as_sequence)
        .map_or(&[][..], Vec::as_slice)
}

fn opt_name(e: &Yaml) -> Result<Option<String>> {
    match get(e, "name") {
        Some(n) => {
            let s = as_str(n)?;
            Ok((!s.is_empty()).then_some(s))
        }
        None => Ok(None),
    }
}

fn int_or<T: TryFrom<i64> + Copy>(e: &Yaml, key: &str, default: T) -> Result<T> {
    Ok(get(e, key).map(as_int).transpose()?.unwrap_or(default))
}

fn u16_list(e: &Yaml, key: &str) -> Result<Vec<u16>> {
    Ok(get(e, key)
        .and_then(Yaml::as_sequence)
        .map(|s| s.iter().map(as_int::<u16>).collect::<Result<Vec<u16>>>())
        .transpose()?
        .unwrap_or_default())
}

fn as_str(y: &Yaml) -> Result<String> {
    y.as_str()
        .map(str::to_string)
        .ok_or_else(|| Error::malformed("expected string"))
}

fn as_int<T: TryFrom<i64>>(y: &Yaml) -> Result<T> {
    let n = y
        .as_integer()
        .ok_or_else(|| Error::malformed("expected integer"))?;
    T::try_from(n).map_err(|_| Error::malformed(format!("integer {n} out of range")))
}

fn is_null(y: &Yaml) -> bool {
    matches!(y, Yaml::Value(saphyr::Scalar::Null))
}

fn hex_bytes(y: &Yaml) -> Result<Vec<u8>> {
    hex_decode(&as_str(y)?)
}

fn fixed<const N: usize>(y: &Yaml) -> Result<[u8; N]> {
    let v = hex_bytes(y)?;
    if v.len() != N {
        return Err(Error::malformed(format!(
            "expected {N} bytes, got {}",
            v.len()
        )));
    }
    let mut a = [0u8; N];
    a.copy_from_slice(&v);
    Ok(a)
}

fn pad_byte(y: &Yaml) -> Result<u8> {
    let [b] = fixed::<1>(y)?;
    Ok(b)
}

/// Parses the YAML produced by [`emit_msbt`](super::emit::emit_msbt) back into
/// an [`Msbt`]. The same [`Registry`] used to emit must be supplied to resolve
/// named control tags.
pub fn parse_msbt(text: &str, reg: Option<&Registry>) -> Result<Msbt> {
    let docs = load(text)?;
    let doc = doc_of(&docs)?;
    let meta = req(doc, "_meta")?;

    let header = parse_header(doc, meta)?;

    let sections_y = req(meta, "sections")?;
    let order: Vec<[u8; 4]> = sections_y
        .as_sequence()
        .ok_or_else(|| Error::malformed("sections must be a list"))?
        .iter()
        .map(|s| magic4(&as_str(s)?))
        .collect::<Result<_>>()?;

    let mut secmeta: HashMap<[u8; 4], SecMeta> = HashMap::new();
    let mut lbl1_buckets = 0u32;
    let mut attr_size = 0u32;
    let mut atr1_pool = Vec::new();
    let mut ato1 = Vec::new();
    let mut raw: BTreeMap<[u8; 4], Vec<u8>> = BTreeMap::new();

    for magic in &order {
        let key = String::from_utf8_lossy(magic).to_ascii_lowercase();
        if let Some(entry) = get(meta, &key) {
            let reserved = fixed::<8>(req(entry, "reserved")?)?;
            let padding = pad_byte(req(entry, "padding")?)?;
            secmeta.insert(*magic, SecMeta { reserved, padding });
            if magic == b"LBL1" {
                lbl1_buckets = as_int(req(entry, "buckets")?)?;
            }
            if magic == b"ATR1" {
                attr_size = as_int(req(entry, "attr_size")?)?;
                if let Some(sd) = get(entry, "string_data") {
                    atr1_pool = hex_bytes(sd)?;
                }
            }
        }
    }

    if let Some(rs) = get(meta, "raw_sections") {
        for item in rs.as_sequence().map_or(&[][..], Vec::as_slice) {
            let ty = magic4(&as_str(req(item, "type")?)?)?;
            let reserved = fixed::<8>(req(item, "reserved")?)?;
            let padding = pad_byte(req(item, "padding")?)?;
            secmeta.insert(ty, SecMeta { reserved, padding });
            if let Some(ints) = get(item, "ints") {
                for v in ints.as_sequence().map_or(&[][..], Vec::as_slice) {
                    let n: u32 = if is_null(v) { 0xFFFF_FFFF } else { as_int(v)? };
                    ato1.extend_from_slice(&n.to_le_bytes());
                }
            } else if let Some(b) = get(item, "bytes") {
                raw.insert(ty, hex_bytes(b)?);
            }
        }
    }

    let messages = parse_messages(doc, reg, attr_size)?;

    Ok(Msbt {
        header,
        order,
        meta: secmeta.into_iter().collect(),
        lbl1_buckets,
        ato1,
        attr_size,
        atr1_pool,
        messages,
        raw,
    })
}

fn parse_messages(doc: &Yaml, reg: Option<&Registry>, attr_size: u32) -> Result<Vec<Message>> {
    let msgs = req(doc, "messages")?
        .as_sequence()
        .ok_or_else(|| Error::malformed("messages must be a list"))?;
    let mut messages = Vec::with_capacity(msgs.len());
    for msg in msgs {
        let label = as_str(req(msg, "label")?)?;
        let style: u32 = as_int(req(msg, "style")?)?;
        let (attr, mtext) = if let Some(raw_attr) = get(msg, "attr_raw") {
            (hex_bytes(raw_attr)?, hex_bytes(req(msg, "text_raw")?)?)
        } else {
            let reg = reg.ok_or_else(|| {
                Error::malformed("attrs present but no MSBP registry available to encode them")
            })?;
            let m = req(msg, "attrs")?
                .as_mapping()
                .ok_or_else(|| Error::malformed("attrs must be a map"))?;
            let mut values = HashMap::new();
            let mut trailing = String::new();
            for (k, v) in m {
                let key = k.as_str().unwrap_or("");
                if key == "attr_trailing" {
                    trailing = as_str(v)?;
                } else {
                    values.insert(key.to_string(), as_str(v)?);
                }
            }
            let attr = reg.encode_attrs(&values, &trailing, attr_size as usize)?;
            let text = match get(msg, "text") {
                Some(t) => as_str(t)?,
                None => String::new(),
            };
            (attr, reg.encode_text(&text)?)
        };
        messages.push(Message {
            label,
            style,
            attr,
            text: mtext,
        });
    }
    Ok(messages)
}

fn parse_header(doc: &Yaml, meta: &Yaml) -> Result<Header> {
    let encoding = encoding_byte(&as_str(req(doc, "encoding")?)?);
    let version: u8 = as_int(req(doc, "version")?)?;
    let reserved_a = fixed::<2>(req(meta, "reserved_a")?)?;
    let reserved_b = fixed::<2>(req(meta, "reserved_b")?)?;
    let reserved_tail = fixed::<10>(req(meta, "reserved_tail")?)?;
    Ok(Header {
        reserved_a,
        encoding,
        version,
        reserved_b,
        reserved_tail,
    })
}

fn encoding_byte(s: &str) -> u8 {
    match s {
        "utf8" => 0,
        "utf16le" => 1,
        "utf16be" => 2,
        other => other
            .strip_prefix("raw")
            .and_then(|n| n.parse().ok())
            .unwrap_or(1),
    }
}

fn magic4(s: &str) -> Result<[u8; 4]> {
    let b = s.as_bytes();
    if b.len() != 4 {
        return Err(Error::malformed(format!(
            "section magic `{s}` must be 4 bytes"
        )));
    }
    Ok([b[0], b[1], b[2], b[3]])
}

/// Parses the YAML produced by [`emit_msbp`](super::emit::emit_msbp) back into
/// an [`Msbp`].
pub fn parse_msbp(text: &str) -> Result<Msbp> {
    let docs = load(text)?;
    let doc = doc_of(&docs)?;
    let meta = req(doc, "_meta")?;
    let header = parse_header(doc, meta)?;

    let sections_y = req(meta, "sections")?;
    let order: Vec<[u8; 4]> = sections_y
        .as_sequence()
        .ok_or_else(|| Error::malformed("sections must be a list"))?
        .iter()
        .map(|s| magic4(&as_str(s)?))
        .collect::<Result<_>>()?;

    let secmeta = parse_psecmeta(meta, &order)?;
    let colors = parse_colors(doc)?;
    let attributes = parse_attributes(doc)?;
    let attribute_lists = parse_attribute_lists(doc)?;
    let (tag_groups, tags) = parse_tags(doc)?;
    let tag_params = parse_tag_params(doc)?;
    let tag_param_lists = seq_of(doc, "tag_param_lists")
        .iter()
        .map(|e| as_str(req(e, "item")?))
        .collect::<Result<_>>()?;
    let sources = seq_of(doc, "sources")
        .iter()
        .map(as_str)
        .collect::<Result<_>>()?;

    Ok(Msbp {
        header,
        order,
        meta: secmeta,
        colors,
        attributes,
        attribute_lists,
        tag_groups,
        tags,
        tag_params,
        tag_param_lists,
        sources,
        raw: BTreeMap::new(),
    })
}

fn parse_psecmeta(meta: &Yaml, order: &[[u8; 4]]) -> Result<BTreeMap<[u8; 4], PSecMeta>> {
    let mut secmeta = BTreeMap::new();
    for magic in order {
        let key = String::from_utf8_lossy(magic).to_ascii_lowercase();
        let entry =
            get(meta, &key).ok_or_else(|| Error::malformed(format!("missing meta for {key}")))?;
        secmeta.insert(
            *magic,
            PSecMeta {
                reserved: fixed::<8>(req(entry, "reserved")?)?,
                padding: pad_byte(req(entry, "padding")?)?,
                buckets: int_or(entry, "buckets", 0)?,
                pad: int_or(entry, "pad", 0)?,
            },
        );
    }
    Ok(secmeta)
}

fn parse_colors(doc: &Yaml) -> Result<Vec<Color>> {
    seq_of(doc, "colors")
        .iter()
        .map(|e| {
            Ok(Color {
                name: opt_name(e)?,
                rgba: fixed::<4>(req(e, "rgba")?)?,
            })
        })
        .collect()
}

fn parse_attributes(doc: &Yaml) -> Result<Vec<Attribute>> {
    seq_of(doc, "attributes")
        .iter()
        .map(|e| {
            Ok(Attribute {
                name: opt_name(e)?,
                ty: msbp::type_from_name(&as_str(req(e, "type")?)?)
                    .ok_or_else(|| Error::malformed("bad attr type"))?,
                list_index: int_or(e, "list_index", 0)?,
                offset: as_int(req(e, "offset")?)?,
            })
        })
        .collect()
}

fn parse_attribute_lists(doc: &Yaml) -> Result<Vec<Vec<String>>> {
    seq_of(doc, "attribute_lists")
        .iter()
        .map(|e| {
            seq_of(e, "items")
                .iter()
                .map(as_str)
                .collect::<Result<Vec<_>>>()
        })
        .collect()
}

fn parse_tags(doc: &Yaml) -> Result<(Vec<TagGroup>, Vec<Tag>)> {
    let mut tag_groups = Vec::new();
    let mut max_tag = 0usize;
    let mut tags_by_index: HashMap<u16, Tag> = HashMap::new();
    for grp in seq_of(doc, "tag_groups") {
        let name = as_str(req(grp, "name")?)?;
        let id = int_or(grp, "id", 0)?;
        let tag_indices = u16_list(grp, "tag_indices")?;
        let tags_seq = get(grp, "tags")
            .and_then(Yaml::as_sequence)
            .cloned()
            .unwrap_or_default();
        for (local, &gi) in tag_indices.iter().enumerate() {
            if let Some(te) = tags_seq.get(local) {
                tags_by_index.insert(
                    gi,
                    Tag {
                        param_indices: u16_list(te, "param_indices")?,
                        name: as_str(req(te, "name")?)?,
                    },
                );
                max_tag = max_tag.max(usize::from(gi) + 1);
            }
        }
        tag_groups.push(TagGroup {
            id,
            tag_indices,
            name,
        });
    }
    let tags = (0..max_tag)
        .map(|i| {
            let key = u16::try_from(i).unwrap_or(u16::MAX);
            tags_by_index.remove(&key).unwrap_or(Tag {
                param_indices: vec![],
                name: String::new(),
            })
        })
        .collect();
    Ok((tag_groups, tags))
}

fn parse_tag_params(doc: &Yaml) -> Result<Vec<TagParam>> {
    seq_of(doc, "tag_params")
        .iter()
        .map(|e| {
            Ok(TagParam {
                ty: msbp::type_from_name(&as_str(req(e, "type")?)?)
                    .ok_or_else(|| Error::malformed("bad param type"))?,
                pad: int_or(e, "pad", 0)?,
                list_indices: u16_list(e, "list_indices")?,
                name: as_str(req(e, "name")?)?,
            })
        })
        .collect()
}
