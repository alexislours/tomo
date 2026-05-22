use std::io::Write;

use crate::{Error, Result};

use super::shared::{hex_decode, hex_encode, write_quoted};

#[derive(Debug, Clone)]
pub(super) struct ParamInfo {
    pub(super) display: String,
    pub(super) ty: u8,
    pub(super) list: Vec<String>,
}

pub(super) fn flush_literal_bytes(bytes: &[u8], out: &mut String) {
    let units = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]));
    for r in char::decode_utf16(units) {
        match r.unwrap_or('\u{FFFD}') {
            '\\' => out.push_str("\\\\"),
            '<' => out.push_str("\\<"),
            c => out.push(c),
        }
    }
}

pub(super) fn push_char(units: &mut Vec<u16>, c: char) {
    let mut buf = [0u16; 2];
    for u in c.encode_utf16(&mut buf) {
        units.push(*u);
    }
}

pub(super) fn u16_len(n: usize) -> Result<u16> {
    u16::try_from(n).map_err(|_| Error::overflow("length exceeds u16"))
}

pub(super) fn open_bytes(g: u16, t: u16, params: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(8 + params.len());
    out.extend_from_slice(&0x000Eu16.to_le_bytes());
    out.extend_from_slice(&g.to_le_bytes());
    out.extend_from_slice(&t.to_le_bytes());
    out.extend_from_slice(&u16_len(params.len())?.to_le_bytes());
    out.extend_from_slice(params);
    Ok(out)
}

pub(super) fn strip_quotes(v: &str) -> String {
    v.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(v)
        .to_string()
}

pub(super) fn raw_open(g: u16, t: u16, params: &[u8]) -> String {
    format!("<#raw {g} {t} {}/>", hex_encode(params))
}

pub(super) fn raw_close(g: u16, t: u16) -> String {
    format!("<#close {g} {t}/>")
}

pub(super) fn parse_raw_open(rest: &str) -> Result<Vec<u8>> {
    let rest = rest.trim_end_matches('/').trim();
    let mut it = rest.split_whitespace();
    let g: u16 = it
        .next()
        .ok_or_else(|| Error::malformed("raw open: missing g"))?
        .parse()?;
    let t: u16 = it
        .next()
        .ok_or_else(|| Error::malformed("raw open: missing t"))?
        .parse()?;
    let hex = it.next().unwrap_or("");
    let params = hex_decode(hex)?;
    open_bytes(g, t, &params)
}

pub(super) fn parse_raw_close(rest: &str) -> Result<Vec<u8>> {
    let rest = rest.trim_end_matches('/').trim();
    let mut it = rest.split_whitespace();
    let g: u16 = it
        .next()
        .ok_or_else(|| Error::malformed("raw close: missing g"))?
        .parse()?;
    let t: u16 = it
        .next()
        .ok_or_else(|| Error::malformed("raw close: missing t"))?
        .parse()?;
    let mut out = Vec::new();
    out.extend_from_slice(&0x000Fu16.to_le_bytes());
    out.extend_from_slice(&g.to_le_bytes());
    out.extend_from_slice(&t.to_le_bytes());
    Ok(out)
}

pub(super) fn find_tag_end(chars: &[char], start: usize) -> Result<usize> {
    let mut in_quote = false;
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '"' => in_quote = !in_quote,
            '>' if !in_quote => return Ok(i),
            _ => {}
        }
        i += 1;
    }
    Err(Error::malformed("unterminated tag"))
}

pub(super) fn parse_open_inner(inner: &str) -> Result<(String, Vec<(String, String)>)> {
    let toks = tokenize(inner);
    let mut it = toks.into_iter();
    let name = it.next().ok_or_else(|| Error::malformed("empty tag"))?;
    let mut params = Vec::new();
    for tok in it {
        let (k, v) = tok
            .split_once('=')
            .ok_or_else(|| Error::malformed(format!("tag param missing '=': {tok}")))?;
        params.push((k.to_string(), v.to_string()));
    }
    Ok((name, params))
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut has = false;
    for c in s.chars() {
        match c {
            '"' => {
                in_quote = !in_quote;
                cur.push(c);
                has = true;
            }
            c if c.is_whitespace() && !in_quote => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            _ => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

pub(super) fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(super) fn write_scalar<W: Write>(
    w: &mut W,
    b: &[u8],
    off: usize,
    ty: u8,
    list: &[String],
) -> std::io::Result<()> {
    if off >= b.len() {
        return w.write_all(b"\"\"");
    }
    let u32_at = || u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]);
    match ty {
        0 => write!(w, "\"{}\"", b[off]),
        3 => write!(w, "\"{}\"", b[off].cast_signed()),
        1 => write!(w, "\"{}\"", rd_u16(b, off)),
        4 => write!(w, "\"{}\"", rd_u16(b, off).cast_signed()),
        2 => write!(w, "\"{}\"", u32_at()),
        5 => write!(w, "\"{}\"", u32_at().cast_signed()),
        6 => write_quoted(w, &fmt_f32(f32::from_bits(u32_at()))),
        9 => {
            let idx = b[off] as usize;
            match list.get(idx) {
                Some(s) => write_quoted(w, s),
                None => write!(w, "\"#{idx}\""),
            }
        }
        _ => write!(w, "\"#raw{:02x}\"", b[off]),
    }
}

pub(super) fn encode_scalar(v: &str, ty: u8, list: &[String]) -> Result<Vec<u8>> {
    Ok(match ty {
        0 => vec![v.parse::<u8>()?],
        3 => vec![v.parse::<i8>()?.cast_unsigned()],
        1 => v.parse::<u16>()?.to_le_bytes().to_vec(),
        4 => v.parse::<i16>()?.cast_unsigned().to_le_bytes().to_vec(),
        2 => v.parse::<u32>()?.to_le_bytes().to_vec(),
        5 => v.parse::<i32>()?.cast_unsigned().to_le_bytes().to_vec(),
        6 => parse_f32(v)?.to_bits().to_le_bytes().to_vec(),
        9 => vec![enum_index(v, list)?],
        _ => return Err(Error::unsupported(format!("unsupported attr type {ty}"))),
    })
}

fn enum_index(v: &str, list: &[String]) -> Result<u8> {
    if let Some(rest) = v.strip_prefix('#') {
        return Ok(rest.parse::<u8>()?);
    }
    list.iter()
        .position(|s| s == v)
        .and_then(|p| u8::try_from(p).ok())
        .ok_or_else(|| Error::malformed(format!("enum value `{v}` not in list")))
}

pub(super) fn decode_param(params: &[u8], cursor: usize, p: &ParamInfo) -> Option<(String, usize)> {
    let rem = params.len().checked_sub(cursor)?;
    let b = &params[cursor..];
    Some(match p.ty {
        0 if rem >= 1 => (b[0].to_string(), 1),
        3 if rem >= 1 => (b[0].cast_signed().to_string(), 1),
        1 if rem >= 2 => (rd_u16(b, 0).to_string(), 2),
        4 if rem >= 2 => (rd_u16(b, 0).cast_signed().to_string(), 2),
        2 if rem >= 4 => (u32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_string(), 4),
        5 if rem >= 4 => (
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
                .cast_signed()
                .to_string(),
            4,
        ),
        6 if rem >= 4 => (
            fmt_f32(f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))),
            4,
        ),
        7 if rem >= 8 => {
            let bits = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
            (fmt_f64(f64::from_bits(bits)), 8)
        }
        8 if rem >= 2 => {
            let len = rd_u16(b, 0) as usize;
            if len == 0xFFFF {
                ("~".to_string(), 2)
            } else if rem >= 2 + len {
                let s = decode_utf16(&b[2..2 + len]);
                (quote_string(&s), 2 + len)
            } else {
                return None;
            }
        }
        9 if rem >= 1 => {
            let idx = b[0] as usize;
            let val = p.list.get(idx)?.clone();
            (val, 1)
        }
        _ => return None,
    })
}

pub(super) fn encode_param(out: &mut Vec<u8>, v: &str, p: &ParamInfo) -> Result<()> {
    match p.ty {
        0 => out.push(v.parse::<u8>()?),
        3 => out.push(v.parse::<i8>()?.cast_unsigned()),
        1 => out.extend_from_slice(&v.parse::<u16>()?.to_le_bytes()),
        4 => out.extend_from_slice(&v.parse::<i16>()?.cast_unsigned().to_le_bytes()),
        2 => out.extend_from_slice(&v.parse::<u32>()?.to_le_bytes()),
        5 => out.extend_from_slice(&v.parse::<i32>()?.cast_unsigned().to_le_bytes()),
        6 => out.extend_from_slice(&parse_f32(v)?.to_bits().to_le_bytes()),
        7 => out.extend_from_slice(&parse_f64(v)?.to_bits().to_le_bytes()),
        8 => {
            if v == "~" {
                out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            } else {
                let s = unquote_string(v)?;
                let u: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
                out.extend_from_slice(&u16_len(u.len())?.to_le_bytes());
                out.extend_from_slice(&u);
            }
        }
        9 => out.push(enum_index(v, &p.list)?),
        _ => {
            return Err(Error::unsupported(format!(
                "unsupported param type {}",
                p.ty
            )));
        }
    }
    Ok(())
}

pub(super) fn decode_utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

pub(super) fn quote_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(super) fn unquote_string(v: &str) -> Result<String> {
    let inner = v
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| Error::malformed(format!("expected quoted string, got `{v}`")))?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

pub(super) fn fmt_f32(f: f32) -> String {
    if f.is_finite() {
        let mut b = ryu::Buffer::new();
        b.format_finite(f).to_string()
    } else if f.is_nan() {
        format!("nan(0x{:08x})", f.to_bits())
    } else if f.is_sign_negative() {
        "-.inf".to_string()
    } else {
        ".inf".to_string()
    }
}

fn fmt_f64(f: f64) -> String {
    if f.is_finite() {
        let mut b = ryu::Buffer::new();
        b.format_finite(f).to_string()
    } else if f.is_nan() {
        format!("nan(0x{:016x})", f.to_bits())
    } else if f.is_sign_negative() {
        "-.inf".to_string()
    } else {
        ".inf".to_string()
    }
}

fn parse_f32(s: &str) -> Result<f32> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("nan(0x").and_then(|r| r.strip_suffix(')')) {
        return Ok(f32::from_bits(u32::from_str_radix(rest, 16)?));
    }
    Ok(match s {
        ".inf" | "+.inf" => f32::INFINITY,
        "-.inf" => f32::NEG_INFINITY,
        _ => s.parse::<f32>()?,
    })
}

fn parse_f64(s: &str) -> Result<f64> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("nan(0x").and_then(|r| r.strip_suffix(')')) {
        return Ok(f64::from_bits(u64::from_str_radix(rest, 16)?));
    }
    Ok(match s {
        ".inf" | "+.inf" => f64::INFINITY,
        "-.inf" => f64::NEG_INFINITY,
        _ => s.parse::<f64>()?,
    })
}
