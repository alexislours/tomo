use std::collections::HashMap;
use std::io::Write;

use crate::formats::msbp::Msbp;
use crate::{Error, Result};

use super::codec::{
    ParamInfo, decode_param, decode_utf16, encode_param, encode_scalar, find_tag_end,
    flush_literal_bytes, open_bytes, parse_open_inner, parse_raw_close, parse_raw_open, push_char,
    quote_string, raw_close, raw_open, rd_u16, strip_quotes, u16_len, unquote_string, write_scalar,
};
use super::shared::{hex_decode, hex_encode, write_quoted};

#[derive(Debug, Clone)]
struct AttrInfo {
    name: String,
    ty: u8,
    offset: usize,
    list: Vec<String>,
}

/// Tag and attribute definitions drawn from an [`Msbp`], used to render and
/// parse MSBT control tags by name instead of by raw index.
#[derive(Debug, Clone)]
pub struct Registry {
    group_tags: HashMap<u16, Vec<(String, Vec<u16>)>>,
    params: Vec<ParamInfo>,
    attrs: Vec<AttrInfo>,
    tag_lookup: HashMap<String, (u16, u16)>,
}

fn type_size(ty: u8) -> usize {
    match ty {
        0 | 3 | 9 => 1,
        1 | 4 => 2,
        2 | 5 | 6 => 4,
        7 => 8,
        _ => 0,
    }
}

impl Registry {
    /// Builds a registry from a parsed [`Msbp`].
    #[must_use]
    pub fn from_msbp(m: &Msbp) -> Self {
        let params = m
            .tag_params
            .iter()
            .map(|p| {
                let display = if p.ty == 9 {
                    p.name.clone()
                } else {
                    let mut s = String::new();
                    s.push(char::from(p.pad));
                    s.push_str(&p.name);
                    s
                };
                let list = p
                    .list_indices
                    .iter()
                    .map(|&j| {
                        m.tag_param_lists
                            .get(j as usize)
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect();
                ParamInfo {
                    display,
                    ty: p.ty,
                    list,
                }
            })
            .collect();

        let attrs = m
            .attributes
            .iter()
            .map(|a| {
                let list = if a.ty == 9 {
                    m.attribute_lists
                        .get(a.list_index as usize)
                        .cloned()
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                AttrInfo {
                    name: a.name.clone().unwrap_or_default(),
                    ty: a.ty,
                    offset: a.offset as usize,
                    list,
                }
            })
            .collect();

        let mut group_tags: HashMap<u16, Vec<(String, Vec<u16>)>> = HashMap::new();
        let mut tag_lookup = HashMap::new();
        for g in &m.tag_groups {
            let mut tags = Vec::with_capacity(g.tag_indices.len());
            for (local, &gi) in g.tag_indices.iter().enumerate() {
                if let Some(t) = m.tags.get(gi as usize) {
                    tags.push((t.name.clone(), t.param_indices.clone()));
                    if let Ok(local) = u16::try_from(local) {
                        tag_lookup.entry(t.name.clone()).or_insert((g.id, local));
                    }
                } else {
                    tags.push((format!("tag{local}"), Vec::new()));
                }
            }
            group_tags.insert(g.id, tags);
        }

        Self {
            group_tags,
            params,
            attrs,
            tag_lookup,
        }
    }

    pub(super) fn write_attrs<W: Write>(&self, w: &mut W, record: &[u8]) -> std::io::Result<usize> {
        let mut boundary = record.len();
        for a in &self.attrs {
            let size = type_size(a.ty);
            if a.ty == 8 || a.offset + size > record.len() {
                boundary = a.offset.min(record.len());
                break;
            }
            if a.ty == 9 && (record[a.offset] as usize) >= a.list.len() {
                boundary = a.offset;
                break;
            }
            w.write_all(b"      ")?;
            write_quoted(w, &a.name)?;
            w.write_all(b": ")?;
            write_scalar(w, record, a.offset, a.ty, &a.list)?;
            w.write_all(b"\n")?;
            boundary = a.offset + size;
        }
        Ok(boundary)
    }

    pub(super) fn encode_attrs(
        &self,
        values: &HashMap<String, String>,
        trailing: &str,
        attr_size: usize,
    ) -> Result<Vec<u8>> {
        let mut rec = vec![0u8; attr_size];
        let mut boundary = attr_size;
        for a in &self.attrs {
            let Some(v) = values.get(&a.name) else {
                boundary = a.offset;
                break;
            };
            let bytes = encode_scalar(v, a.ty, &a.list)?;
            if a.offset + bytes.len() <= rec.len() {
                rec[a.offset..a.offset + bytes.len()].copy_from_slice(&bytes);
            }
            boundary = a.offset + bytes.len();
        }
        let tb = hex_decode(trailing)?;
        if boundary + tb.len() <= rec.len() {
            rec[boundary..boundary + tb.len()].copy_from_slice(&tb);
        } else if !tb.is_empty() {
            return Err(Error::malformed("attr_trailing too long for record"));
        }
        Ok(rec)
    }

    fn resolve_tag(&self, g: u16, t: u16) -> Option<(&str, &[u16])> {
        let tags = self.group_tags.get(&g)?;
        let (name, params) = tags.get(t as usize)?;
        Some((name, params))
    }

    fn render_open(&self, g: u16, t: u16, params: &[u8]) -> Option<String> {
        let (name, pidx) = self.resolve_tag(g, t)?;
        if name == "Ruby" {
            return self.render_ruby(name, params);
        }
        let mut s = String::from("<");
        s.push_str(name);
        let mut cursor = 0usize;
        for &pi in pidx {
            let p = self.params.get(pi as usize)?;
            let start = if p.ty == 8 && cursor % 2 == 1 && params.get(cursor) == Some(&0xCD) {
                cursor + 1
            } else {
                cursor
            };

            if start >= params.len() {
                break;
            }
            let (val, consumed) = decode_param(params, start, p)?;
            cursor = start + consumed;
            s.push(' ');
            s.push_str(&p.display);
            s.push('=');
            s.push_str(&val);
        }
        let remaining = &params[cursor.min(params.len())..];
        if !(remaining.is_empty() || remaining == [0xCD]) {
            return None;
        }
        s.push_str("/>");
        Some(s)
    }

    fn render_ruby(&self, name: &str, params: &[u8]) -> Option<String> {
        if params.len() < 4 {
            return None;
        }
        let span = rd_u16(params, 0);
        let len = rd_u16(params, 2) as usize;
        if 4 + len != params.len() {
            return None;
        }
        let text = decode_utf16(&params[4..4 + len]);
        let pname = self
            .params
            .first()
            .map_or_else(|| "rt".into(), |p| p.display.clone());
        Some(format!("<{name} {pname}={span}:{}/>", quote_string(&text)))
    }

    fn render_payload(&self, g: u16, t: u16, params: &[u8]) -> Option<String> {
        let (name, _) = self.resolve_tag(g, t)?;
        Some(format!("<{name} payload=\"{}\"/>", hex_encode(params)))
    }

    fn render_close(&self, g: u16, t: u16) -> Option<String> {
        let (name, _) = self.resolve_tag(g, t)?;
        Some(format!("</{name}>"))
    }

    fn encode_open(&self, name: &str, tokens: &[(String, String)]) -> Result<Vec<u8>> {
        let &(grp, tag) = self
            .tag_lookup
            .get(name)
            .ok_or_else(|| Error::malformed(format!("unknown tag `{name}`")))?;

        if let [(key, val)] = tokens
            && key == "payload"
        {
            let bytes = hex_decode(&strip_quotes(val))?;
            return open_bytes(grp, tag, &bytes);
        }

        if name == "Ruby" {
            let (_, val) = tokens
                .first()
                .ok_or_else(|| Error::malformed("Ruby missing rt"))?;
            let (span_s, str_s) = val
                .split_once(':')
                .ok_or_else(|| Error::malformed("bad Ruby rt"))?;
            let span: u16 = span_s.parse()?;
            let text = unquote_string(str_s)?;
            let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let mut params = Vec::new();
            params.extend_from_slice(&span.to_le_bytes());
            params.extend_from_slice(&u16_len(utf16.len())?.to_le_bytes());
            params.extend_from_slice(&utf16);
            return open_bytes(grp, tag, &params);
        }

        let (_, pidx) = self
            .resolve_tag(grp, tag)
            .ok_or_else(|| Error::malformed(format!("unresolved tag `{name}`")))?;
        let mut params = Vec::new();
        for (i, &pi) in pidx.iter().enumerate() {
            let Some((_, valstr)) = tokens.get(i) else {
                break;
            };
            let p = self
                .params
                .get(pi as usize)
                .ok_or_else(|| Error::malformed("bad param index"))?;
            if p.ty == 8 && params.len() % 2 == 1 {
                params.push(0xCD);
            }
            encode_param(&mut params, valstr, p)?;
        }
        if params.len() % 2 != 0 {
            params.push(0xCD);
        }
        open_bytes(grp, tag, &params)
    }

    fn encode_close(&self, name: &str) -> Result<Vec<u8>> {
        let &(g, t) = self
            .tag_lookup
            .get(name)
            .ok_or_else(|| Error::malformed(format!("unknown close tag `{name}`")))?;
        let mut out = Vec::with_capacity(6);
        out.extend_from_slice(&0x000Fu16.to_le_bytes());
        out.extend_from_slice(&g.to_le_bytes());
        out.extend_from_slice(&t.to_le_bytes());
        Ok(out)
    }

    pub(super) fn decode_text(&self, raw: &[u8]) -> String {
        let nunits = raw.len() / 2;
        let unit = |i: usize| u16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]);
        let mut out = String::with_capacity(raw.len());
        let mut lit_start = 0usize;
        let mut i = 0usize;
        while i < nunits {
            match unit(i) {
                0x000E if i + 3 < nunits => {
                    let grp = unit(i + 1);
                    let tag = unit(i + 2);
                    let plen = unit(i + 3) as usize;
                    let pstart = (i + 4) * 2;
                    let pend = pstart + plen;
                    if pend > raw.len() {
                        i += 1;
                        continue;
                    }
                    flush_literal_bytes(&raw[lit_start * 2..i * 2], &mut out);
                    let params = &raw[pstart..pend];
                    let orig = &raw[i * 2..pend];
                    match self.render_open(grp, tag, params) {
                        Some(s) if self.verify_open(&s, orig) => out.push_str(&s),
                        _ => match self.render_payload(grp, tag, params) {
                            Some(s) => out.push_str(&s),
                            None => out.push_str(&raw_open(grp, tag, params)),
                        },
                    }
                    i += 4 + plen / 2;
                    lit_start = i;
                }
                0x000F if i + 2 < nunits => {
                    flush_literal_bytes(&raw[lit_start * 2..i * 2], &mut out);
                    let grp = unit(i + 1);
                    let tag = unit(i + 2);
                    match self.render_close(grp, tag) {
                        Some(s) if self.encode_close_name(&s) == Some((grp, tag)) => {
                            out.push_str(&s);
                        }
                        _ => out.push_str(&raw_close(grp, tag)),
                    }
                    i += 3;
                    lit_start = i;
                }
                0x0000 if i == nunits - 1 => break,
                _ => i += 1,
            }
        }
        flush_literal_bytes(&raw[lit_start * 2..i * 2], &mut out);
        out
    }

    fn encode_close_name(&self, rendered: &str) -> Option<(u16, u16)> {
        let name = rendered.strip_prefix("</")?.strip_suffix('>')?;
        self.tag_lookup.get(name).copied()
    }

    fn verify_open(&self, rendered: &str, orig: &[u8]) -> bool {
        let Some(inner) = rendered
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix("/>"))
        else {
            return false;
        };
        match parse_open_inner(inner) {
            Ok((name, tokens)) => match self.encode_open(&name, &tokens) {
                Ok(bytes) => bytes == orig,
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    pub(super) fn encode_text(&self, text: &str) -> Result<Vec<u8>> {
        let mut units: Vec<u16> = Vec::new();
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if c == '\\' {
                i += 1;
                if i < chars.len() {
                    push_char(&mut units, chars[i]);
                    i += 1;
                }
            } else if c == '<' {
                let end = find_tag_end(&chars, i)?;
                let inner: String = chars[i + 1..end].iter().collect();
                let tag_bytes = self.encode_tag_token(&inner)?;
                for ch in tag_bytes.chunks_exact(2) {
                    units.push(u16::from_le_bytes([ch[0], ch[1]]));
                }
                i = end + 1;
            } else {
                push_char(&mut units, c);
                i += 1;
            }
        }
        units.push(0x0000);
        let mut out = Vec::with_capacity(units.len() * 2);
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
        Ok(out)
    }

    fn encode_tag_token(&self, inner: &str) -> Result<Vec<u8>> {
        if let Some(rest) = inner.strip_prefix("#raw ") {
            return parse_raw_open(rest);
        }
        if let Some(rest) = inner.strip_prefix("#close ") {
            return parse_raw_close(rest);
        }
        if let Some(name) = inner.strip_prefix('/') {
            return self.encode_close(name);
        }
        let stripped = inner.strip_suffix('/').unwrap_or(inner);
        let (name, tokens) = parse_open_inner(stripped.trim_end())?;
        self.encode_open(&name, &tokens)
    }
}
