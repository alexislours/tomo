use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use tabled::builder::Builder;
use tomolib::formats::binio::align_up;
use tomolib::formats::byml::{Byml, BymlReader, Endian, Value, is_container, node};

use crate::commands::rstbl;
use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, strip_ext, write_file};

#[derive(Debug, Args)]
pub(crate) struct BymlArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Summarize a BYML file.
    Info {
        input: PathBuf,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Decompose a BYML/BGYML into a YAML document.
    Extract {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Build a BYML/BGYML from a YAML document.
    Pack {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Update an existing RESTBL table with the packed file's size.
        ///
        /// Looks up the entry by `--resource-name` (or, if omitted, the
        /// output file name) and writes the new size in place.
        #[arg(long, value_name = "PATH")]
        update_rstbl: Option<PathBuf>,
        /// Resource name used when updating the RESTBL. Defaults to the
        /// output file's basename.
        #[arg(long, value_name = "NAME", requires = "update_rstbl")]
        resource_name: Option<String>,
    },
}

pub(crate) fn run(args: BymlArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input, common } => info(&input, common.json),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack {
            input,
            out,
            update_rstbl,
            resource_name,
        } => pack(&input, out, update_rstbl, resource_name),
    }
}

pub(crate) fn convert_bytes_to_yaml(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(bytes.len() * 2);
    emit_document_streaming(bytes, &mut out)?;
    Ok(out)
}

fn info(input: &Path, json: bool) -> Result<()> {
    let bytes = fs::read(input).with_context(|| format!("read `{}`", input.display()))?;
    let b = Byml::parse(&bytes).with_context(|| format!("parse `{}`", input.display()))?;

    let (kind, count) = root_summary(&b.root);
    let endian = match b.endian {
        Endian::Little => "little",
        Endian::Big => "big",
    };

    if json {
        let obj = serde_json::json!({
            "file": input.display().to_string(),
            "version": b.version,
            "endian": endian,
            "root_type": kind,
            "root_entries": count,
            "total_size": bytes.len(),
        });
        return crate::fmt::print_json(&obj);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String, extra: String| {
        t.push_record([label(k), value(v), extra]);
    };
    row("Version", b.version.to_string(), String::new());
    row("Endian", endian.to_string(), String::new());
    row("Root type", kind.to_string(), String::new());
    row("Root entries", count.to_string(), String::new());
    let total = bytes.len() as u64;
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));
    Ok(())
}

fn root_summary(v: &Value) -> (&'static str, usize) {
    match v {
        Value::Null => ("null", 0),
        Value::Array(a) => ("array", a.len()),
        Value::Dict(d) => ("dict", d.len()),
        Value::Hash32(h) => ("hash32", h.len()),
        Value::Hash64(h) => ("hash64", h.len()),
        _ => ("scalar", 1),
    }
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let bytes = fs::read(input).with_context(|| format!("read `{}`", input.display()))?;
    let out = out.unwrap_or_else(|| append_ext(input, "yml"));
    let mut w =
        BufWriter::new(File::create(&out).with_context(|| format!("create `{}`", out.display()))?);
    emit_document_streaming(&bytes, &mut w)
        .with_context(|| format!("write `{}`", out.display()))?;
    println!("extracted {} -> {}", input.display(), out.display());
    Ok(())
}

fn pack(
    input: &Path,
    out: Option<PathBuf>,
    update_rstbl: Option<PathBuf>,
    resource_name: Option<String>,
) -> Result<()> {
    let text = fs::read_to_string(input).with_context(|| format!("read `{}`", input.display()))?;
    let b = parse_document(&text).with_context(|| format!("parse `{}`", input.display()))?;
    let out = match out {
        Some(p) => p,
        None => strip_ext(input, &["yml", "yaml"])?,
    };
    let bytes = b
        .to_bytes()
        .with_context(|| format!("serialize `{}`", input.display()))?;
    write_file(&out, &bytes)?;
    println!(
        "packed {} -> {} ({})",
        input.display(),
        out.display(),
        fmt_bytes(bytes.len() as u64),
    );
    if let Some(rstbl_path) = update_rstbl {
        rstbl::maybe_update_rstbl(&rstbl_path, resource_name, &out, bytes.len())?;
    }
    Ok(())
}

fn write_indent<W: Write>(w: &mut W, depth: usize) -> Result<()> {
    const SPACES: &[u8] = &[b' '; 128];
    let mut remaining = depth.saturating_mul(2);
    while remaining > 0 {
        let take = remaining.min(SPACES.len());
        w.write_all(&SPACES[..take])?;
        remaining -= take;
    }
    Ok(())
}

fn emit_f32<W: Write>(w: &mut W, f: f32) -> Result<()> {
    if f.is_nan() {
        let mut buf = [0u8; 16];
        let s = format_nan_bits(&mut buf, u64::from(f.to_bits()), 8);
        w.write_all(s)?;
    } else if f.is_infinite() {
        w.write_all(if f.is_sign_negative() {
            b"-.inf"
        } else {
            b".inf"
        })?;
    } else {
        let mut rbuf = ryu::Buffer::new();
        let s = rbuf.format_finite(f);
        w.write_all(s.as_bytes())?;
    }
    Ok(())
}

fn emit_f64<W: Write>(w: &mut W, f: f64) -> Result<()> {
    if f.is_nan() {
        let mut buf = [0u8; 24];
        let s = format_nan_bits(&mut buf, f.to_bits(), 16);
        w.write_all(s)?;
    } else if f.is_infinite() {
        w.write_all(if f.is_sign_negative() {
            b"-.inf"
        } else {
            b".inf"
        })?;
    } else {
        let mut rbuf = ryu::Buffer::new();
        let s = rbuf.format_finite(f);
        w.write_all(s.as_bytes())?;
    }
    Ok(())
}

fn format_nan_bits(buf: &mut [u8], bits: u64, width: usize) -> &[u8] {
    const PREFIX: &[u8] = b"nan(0x";
    const SUFFIX: &[u8] = b")";
    let total = PREFIX.len() + width + SUFFIX.len();
    buf[..PREFIX.len()].copy_from_slice(PREFIX);
    let hex = b"0123456789abcdef";
    for i in 0..width {
        let shift = (width - 1 - i) * 4;
        buf[PREFIX.len() + i] = hex[((bits >> shift) & 0xF) as usize];
    }
    buf[PREFIX.len() + width..total].copy_from_slice(SUFFIX);
    &buf[..total]
}

fn write_hex_padded<W: Write>(w: &mut W, n: u64, width: usize) -> Result<()> {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    let hex = b"0123456789abcdef";
    for i in 0..width {
        let shift = (width - 1 - i) * 4;
        buf[2 + i] = hex[((n >> shift) & 0xF) as usize];
    }
    w.write_all(&buf[..2 + width])?;
    Ok(())
}

fn yaml_quote_to<W: Write>(w: &mut W, s: &str) -> Result<()> {
    w.write_all(b"\"")?;
    let bytes = s.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let esc: Option<&'static [u8]> = match b {
            b'\\' => Some(b"\\\\"),
            b'"' => Some(b"\\\""),
            b'\n' => Some(b"\\n"),
            b'\r' => Some(b"\\r"),
            b'\t' => Some(b"\\t"),
            _ => None,
        };
        if let Some(seq) = esc {
            if start < i {
                w.write_all(&bytes[start..i])?;
            }
            w.write_all(seq)?;
            i += 1;
            start = i;
        } else if b < 0x20 {
            if start < i {
                w.write_all(&bytes[start..i])?;
            }
            let hex = b"0123456789abcdef";
            let buf = [b'\\', b'x', hex[(b >> 4) as usize], hex[(b & 0xF) as usize]];
            w.write_all(&buf)?;
            i += 1;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < bytes.len() {
        w.write_all(&bytes[start..])?;
    }
    w.write_all(b"\"")?;
    Ok(())
}

fn emit_key<W: Write>(w: &mut W, s: &str) -> Result<()> {
    if is_safe_plain_key(s) {
        w.write_all(s.as_bytes())?;
        Ok(())
    } else {
        yaml_quote_to(w, s)
    }
}

fn is_safe_plain_key(s: &str) -> bool {
    let bytes = s.as_bytes();
    let Some((&first, rest)) = bytes.split_first() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == b'_') {
        return false;
    }
    for &b in rest {
        let ok = b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/');
        if !ok {
            return false;
        }
    }
    !matches!(
        s,
        "null" | "Null" | "NULL" | "~" | "true" | "True" | "TRUE" | "false" | "False" | "FALSE"
    )
}

pub(crate) fn parse_document(text: &str) -> Result<Byml> {
    use saphyr::{LoadableYamlNode, Yaml};
    let docs = Yaml::load_from_str(text).map_err(|e| anyhow::anyhow!("YAML parse: {e}"))?;
    let mut docs = docs.into_iter();
    let doc = docs
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty YAML document"))?;
    if docs.next().is_some() {
        bail!("expected a single YAML document");
    }
    let mapping = doc
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("expected top-level YAML mapping"))?;

    let mut version: Option<u16> = None;
    let mut endian: Option<Endian> = None;
    let mut root: Option<Value> = None;
    for (k, v) in mapping {
        let key = k
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("top-level keys must be strings"))?;
        match key {
            "version" => {
                let raw = v
                    .as_integer()
                    .ok_or_else(|| anyhow::anyhow!("version must be an integer"))?;
                version = Some(
                    u16::try_from(raw)
                        .map_err(|_| anyhow::anyhow!("version {raw} out of range"))?,
                );
            }
            "endian" => {
                let s = v
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("endian must be a string"))?;
                endian = Some(match s {
                    "little" | "le" => Endian::Little,
                    "big" | "be" => Endian::Big,
                    other => bail!("invalid endian `{other}`"),
                });
            }
            "root" => {
                root = Some(value_from_yaml(v)?);
            }
            other => bail!("unknown top-level key `{other}`"),
        }
    }
    let version = version.ok_or_else(|| anyhow::anyhow!("missing `version`"))?;
    let endian = endian.ok_or_else(|| anyhow::anyhow!("missing `endian`"))?;
    let root = root.unwrap_or(Value::Null);
    Ok(Byml {
        version,
        endian,
        root,
    })
}

fn value_from_yaml(y: &saphyr::Yaml) -> Result<Value> {
    use saphyr::Yaml;
    match y {
        Yaml::Tagged(tag, inner) => {
            let tag = tag.to_string();
            tagged_value(&tag, inner)
        }
        Yaml::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                out.push(value_from_yaml(it)?);
            }
            Ok(Value::Array(out))
        }
        Yaml::Mapping(m) => {
            let mut out = BTreeMap::new();
            for (k, v) in m {
                let key = k
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("dict keys must be strings"))?
                    .to_string();
                out.insert(key, value_from_yaml(v)?);
            }
            Ok(Value::Dict(out))
        }
        Yaml::Value(_) => {
            bail!(
                "untagged scalar `{}` is not allowed (use explicit type tags like !u32, !str, !bool, !null)",
                y.as_str().unwrap_or("?")
            );
        }
        Yaml::Representation(_, _, _) => {
            bail!("unexpected raw representation node");
        }
        Yaml::Alias(_) => bail!("YAML aliases are not supported"),
        Yaml::BadValue => bail!("YAML parse error in value"),
    }
}

fn tagged_value(tag: &str, inner: &saphyr::Yaml) -> Result<Value> {
    let short = tag.rsplit(':').next().unwrap_or(tag);
    let short = short.trim_start_matches('!');
    match short {
        "null" => Ok(Value::Null),
        "bool" => {
            if let Some(b) = inner.as_bool() {
                return Ok(Value::Bool(b));
            }
            let s = scalar_text(inner)?;
            match s.as_str() {
                "true" | "True" | "TRUE" => Ok(Value::Bool(true)),
                "false" | "False" | "FALSE" => Ok(Value::Bool(false)),
                _ => bail!("invalid !bool value `{s}`"),
            }
        }
        "i32" => Ok(Value::I32(parse_int::<i32>(inner)?)),
        "u32" => Ok(Value::U32(parse_int::<u32>(inner)?)),
        "i64" => Ok(Value::I64(parse_int::<i64>(inner)?)),
        "u64" => Ok(Value::U64(parse_int::<u64>(inner)?)),
        "f32" => Ok(Value::F32(parse_f32(inner)?)),
        "f64" => Ok(Value::F64(parse_f64(inner)?)),
        "str" => Ok(Value::String(scalar_text(inner)?)),
        "binary" => {
            let s = inner
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("!binary expects a base64 string"))?;
            let data = B64.decode(s).context("base64 decode")?;
            Ok(Value::Binary(data))
        }
        "binary_align" => {
            let seq = inner
                .as_sequence()
                .ok_or_else(|| anyhow::anyhow!("!binary_align expects [align, base64]"))?;
            if seq.len() != 2 {
                bail!("!binary_align expects two items");
            }
            let align = parse_int::<u32>(&seq[0])?;
            let b64 = seq[1]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("!binary_align second element must be a string"))?;
            let data = B64.decode(b64).context("base64 decode")?;
            Ok(Value::BinaryAlign { data, align })
        }
        "hash32" => {
            let m = inner
                .as_mapping()
                .ok_or_else(|| anyhow::anyhow!("!hash32 expects a mapping"))?;
            let mut out = BTreeMap::new();
            for (k, v) in m {
                let key = hash_key_u32(k)?;
                out.insert(key, value_from_yaml(v)?);
            }
            Ok(Value::Hash32(out))
        }
        "hash64" => {
            let m = inner
                .as_mapping()
                .ok_or_else(|| anyhow::anyhow!("!hash64 expects a mapping"))?;
            let mut out = BTreeMap::new();
            for (k, v) in m {
                let key = hash_key_u64(k)?;
                out.insert(key, value_from_yaml(v)?);
            }
            Ok(Value::Hash64(out))
        }
        other => bail!("unknown tag `!{other}`"),
    }
}

fn scalar_text(y: &saphyr::Yaml) -> Result<String> {
    if let Some(s) = y.as_str() {
        return Ok(s.to_string());
    }
    if let Some(n) = y.as_integer() {
        return Ok(n.to_string());
    }
    if let Some(f) = y.as_floating_point() {
        return Ok(format!("{f:?}"));
    }
    if let Some(b) = y.as_bool() {
        return Ok(b.to_string());
    }
    bail!("expected scalar value");
}

fn parse_int<T: std::str::FromStr>(y: &saphyr::Yaml) -> Result<T>
where
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    let s = scalar_text(y)?;
    s.parse::<T>()
        .map_err(|e| anyhow::anyhow!("integer parse `{s}`: {e}"))
}

fn parse_u32_hex(s: &str) -> Result<u32> {
    let s = s.trim();
    let (radix, body) = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, rest)
    } else {
        (10, s)
    };
    u32::from_str_radix(body, radix).map_err(|e| anyhow::anyhow!("invalid u32 `{s}`: {e}"))
}

fn parse_u64_hex(s: &str) -> Result<u64> {
    let s = s.trim();
    let (radix, body) = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, rest)
    } else {
        (10, s)
    };
    u64::from_str_radix(body, radix).map_err(|e| anyhow::anyhow!("invalid u64 `{s}`: {e}"))
}

fn hash_key_u32(y: &saphyr::Yaml) -> Result<u32> {
    if let Some(n) = y.as_integer() {
        return u32::try_from(n).map_err(|_| anyhow::anyhow!("u32 out of range: {n}"));
    }
    if let Some(s) = y.as_str() {
        return parse_u32_hex(s);
    }
    bail!("hash32 keys must be integers or hex strings");
}

fn hash_key_u64(y: &saphyr::Yaml) -> Result<u64> {
    if let Some(n) = y.as_integer() {
        return u64::try_from(n).map_err(|_| anyhow::anyhow!("u64 out of range: {n}"));
    }
    if let Some(s) = y.as_str() {
        return parse_u64_hex(s);
    }
    bail!("hash64 keys must be integers or hex strings");
}

fn parse_f32(y: &saphyr::Yaml) -> Result<f32> {
    let s = scalar_text(y)?;
    parse_float_str_f32(&s)
}

fn parse_f64(y: &saphyr::Yaml) -> Result<f64> {
    let s = scalar_text(y)?;
    parse_float_str_f64(&s)
}

fn parse_float_str_f32(s: &str) -> Result<f32> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("nan(0x").and_then(|r| r.strip_suffix(')')) {
        let bits = u32::from_str_radix(rest, 16)
            .map_err(|e| anyhow::anyhow!("invalid NaN bits `{rest}`: {e}"))?;
        return Ok(f32::from_bits(bits));
    }
    match s {
        ".inf" | "+.inf" | ".Inf" | "+.Inf" | ".INF" | "+.INF" | "inf" => Ok(f32::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" | "-inf" => Ok(f32::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" | "nan" => Ok(f32::NAN),
        _ => s
            .parse::<f32>()
            .map_err(|e| anyhow::anyhow!("invalid f32 `{s}`: {e}")),
    }
}

fn parse_float_str_f64(s: &str) -> Result<f64> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("nan(0x").and_then(|r| r.strip_suffix(')')) {
        let bits = u64::from_str_radix(rest, 16)
            .map_err(|e| anyhow::anyhow!("invalid NaN bits `{rest}`: {e}"))?;
        return Ok(f64::from_bits(bits));
    }
    match s {
        ".inf" | "+.inf" | ".Inf" | "+.Inf" | ".INF" | "+.INF" | "inf" => Ok(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" | "-inf" => Ok(f64::NEG_INFINITY),
        ".nan" | ".NaN" | ".NAN" | "nan" => Ok(f64::NAN),
        _ => s
            .parse::<f64>()
            .map_err(|e| anyhow::anyhow!("invalid f64 `{s}`: {e}")),
    }
}

pub(crate) fn emit_document_streaming<W: Write>(bytes: &[u8], w: &mut W) -> Result<()> {
    let r = BymlReader::parse(bytes).with_context(|| "parse BYML")?;
    w.write_all(b"---\nversion: ")?;
    let mut ibuf = itoa::Buffer::new();
    w.write_all(ibuf.format(r.version).as_bytes())?;
    let endian_line: &[u8] = match r.endian {
        Endian::Little => b"\nendian: little\nroot:",
        Endian::Big => b"\nendian: big\nroot:",
    };
    w.write_all(endian_line)?;
    if r.root_off == 0 {
        w.write_all(b" !null ~")?;
    } else {
        emit_container_at(&r, r.root_off as usize, 0, true, w)?;
    }
    w.write_all(b"\n")?;
    Ok(())
}

fn emit_container_at<W: Write>(
    r: &BymlReader<'_>,
    offset: usize,
    depth: usize,
    after_key: bool,
    w: &mut W,
) -> Result<()> {
    if offset + 4 > r.bytes.len() {
        bail!("container header out of range at {offset:#x}");
    }
    let t = r.bytes[offset];
    let count = r.endian.read_u24(r.bytes, offset + 1, "container header")? as usize;
    match t {
        node::ARRAY => emit_array_stream(r, offset, count, depth, after_key, w),
        node::DICT => emit_dict_stream(r, offset, count, depth, after_key, w),
        node::HASH32 => emit_hash32_stream(r, offset, count, depth, after_key, w),
        node::HASH64 => emit_hash64_stream(r, offset, count, depth, after_key, w),
        _ => bail!("expected container at {offset:#x}, got type {t:#x}"),
    }
}

fn emit_child_stream<W: Write>(
    r: &BymlReader<'_>,
    child_off: usize,
    t: u8,
    depth: usize,
    after_key: bool,
    w: &mut W,
) -> Result<()> {
    if is_container(t) {
        if child_off + 4 > r.bytes.len() {
            bail!("child offset out of range");
        }
        let target = r.endian.read_u32(r.bytes, child_off, "child offset")? as usize;
        emit_container_at(r, target, depth, after_key, w)
    } else {
        emit_value_stream(r, child_off, t, w)
    }
}

fn emit_value_stream<W: Write>(r: &BymlReader<'_>, off: usize, t: u8, w: &mut W) -> Result<()> {
    if off + 4 > r.bytes.len() {
        bail!("value node out of range");
    }
    let endian = r.endian;
    let bytes = r.bytes;
    let raw = endian.read_u32(bytes, off, "value node")?;
    let mut ibuf = itoa::Buffer::new();
    match t {
        node::NULL => w.write_all(b" !null ~")?,
        node::BOOL => {
            w.write_all(if raw != 0 {
                b" !bool true"
            } else {
                b" !bool false"
            })?;
        }
        node::I32 => {
            w.write_all(b" !i32 ")?;
            w.write_all(ibuf.format(raw.cast_signed()).as_bytes())?;
        }
        node::U32 => {
            w.write_all(b" !u32 ")?;
            w.write_all(ibuf.format(raw).as_bytes())?;
        }
        node::F32 => {
            w.write_all(b" !f32 ")?;
            emit_f32(w, f32::from_bits(raw))?;
        }
        node::I64 => {
            let data_off = raw as usize;
            if data_off + 8 > bytes.len() {
                bail!("i64 out of range");
            }
            let num = endian.read_u64(bytes, data_off, "i64 value")?.cast_signed();
            w.write_all(b" !i64 \"")?;
            w.write_all(ibuf.format(num).as_bytes())?;
            w.write_all(b"\"")?;
        }
        node::U64 => {
            let data_off = raw as usize;
            if data_off + 8 > bytes.len() {
                bail!("u64 out of range");
            }
            let num = endian.read_u64(bytes, data_off, "u64 value")?;
            w.write_all(b" !u64 \"")?;
            w.write_all(ibuf.format(num).as_bytes())?;
            w.write_all(b"\"")?;
        }
        node::F64 => {
            let data_off = raw as usize;
            if data_off + 8 > bytes.len() {
                bail!("f64 out of range");
            }
            let num = endian.read_u64(bytes, data_off, "f64 value")?;
            w.write_all(b" !f64 ")?;
            emit_f64(w, f64::from_bits(num))?;
        }
        node::STRING => {
            let s = r.string(raw)?;
            w.write_all(b" !str ")?;
            yaml_quote_to(w, s)?;
        }
        node::BINARY => {
            let data_off = raw as usize;
            if data_off + 4 > bytes.len() {
                bail!("binary header out of range");
            }
            let size = endian.read_u32(bytes, data_off, "binary size")? as usize;
            if data_off + 4 + size > bytes.len() {
                bail!("binary data out of range");
            }
            w.write_all(b" !binary ")?;
            yaml_quote_to(w, &B64.encode(&bytes[data_off + 4..data_off + 4 + size]))?;
        }
        node::BINARY_ALIGN => {
            let data_off = raw as usize;
            if data_off + 8 > bytes.len() {
                bail!("binary_align header out of range");
            }
            let size = endian.read_u32(bytes, data_off, "binary_align size")? as usize;
            let align = endian.read_u32(bytes, data_off + 4, "binary_align alignment")?;
            if data_off + 8 + size > bytes.len() {
                bail!("binary_align data out of range");
            }
            w.write_all(b" !binary_align [")?;
            w.write_all(ibuf.format(align).as_bytes())?;
            w.write_all(b", ")?;
            yaml_quote_to(w, &B64.encode(&bytes[data_off + 8..data_off + 8 + size]))?;
            w.write_all(b"]")?;
        }
        _ => bail!("unexpected value type {t:#x} at {off:#x}"),
    }
    Ok(())
}

fn emit_array_stream<W: Write>(
    r: &BymlReader<'_>,
    offset: usize,
    count: usize,
    depth: usize,
    after_key: bool,
    w: &mut W,
) -> Result<()> {
    if count == 0 {
        w.write_all(b" []")?;
        return Ok(());
    }
    let types_off = offset + 4;
    let values_off = align_up(types_off + count, 4);
    if values_off + 4 * count > r.bytes.len() {
        bail!("array values out of range");
    }
    writeln!(w)?;
    for i in 0..count {
        let t = r.bytes[types_off + i];
        write_indent(w, depth + 1)?;
        w.write_all(b"-")?;
        emit_child_stream(r, values_off + 4 * i, t, depth + 1, false, w)?;
        if i + 1 < count || after_key {
            writeln!(w)?;
        }
    }
    Ok(())
}

fn emit_dict_stream<W: Write>(
    r: &BymlReader<'_>,
    offset: usize,
    count: usize,
    depth: usize,
    after_key: bool,
    w: &mut W,
) -> Result<()> {
    if count == 0 {
        w.write_all(b" {}")?;
        return Ok(());
    }
    writeln!(w)?;
    for i in 0..count {
        let e = offset + 4 + 8 * i;
        if e + 8 > r.bytes.len() {
            bail!("dict entry out of range");
        }
        let name_idx = r.endian.read_u24(r.bytes, e, "dict entry")?;
        let t = r.bytes[e + 3];
        let name = r.key(name_idx)?;
        write_indent(w, depth + 1)?;
        emit_key(w, name)?;
        w.write_all(b":")?;
        emit_child_stream(r, e + 4, t, depth + 1, true, w)?;
        if i + 1 < count || after_key {
            writeln!(w)?;
        }
    }
    Ok(())
}

fn emit_hash32_stream<W: Write>(
    r: &BymlReader<'_>,
    offset: usize,
    count: usize,
    depth: usize,
    after_key: bool,
    w: &mut W,
) -> Result<()> {
    w.write_all(b" !hash32")?;
    if count == 0 {
        w.write_all(b" {}")?;
        return Ok(());
    }
    let types_off = offset + 4 + 8 * count;
    if types_off + count > r.bytes.len() {
        bail!("hash32 types out of range");
    }
    writeln!(w)?;
    for i in 0..count {
        let entry = offset + 4 + 8 * i;
        let hash = r.endian.read_u32(r.bytes, entry, "hash32 entry")?;
        let t = r.bytes[types_off + i];
        write_indent(w, depth + 1)?;
        write_hex_padded(w, u64::from(hash), 8)?;
        w.write_all(b":")?;
        emit_child_stream(r, entry + 4, t, depth + 1, true, w)?;
        if i + 1 < count || after_key {
            writeln!(w)?;
        }
    }
    Ok(())
}

fn emit_hash64_stream<W: Write>(
    r: &BymlReader<'_>,
    offset: usize,
    count: usize,
    depth: usize,
    after_key: bool,
    w: &mut W,
) -> Result<()> {
    w.write_all(b" !hash64")?;
    if count == 0 {
        w.write_all(b" {}")?;
        return Ok(());
    }
    let types_off = offset + 4 + 12 * count;
    if types_off + count > r.bytes.len() {
        bail!("hash64 types out of range");
    }
    writeln!(w)?;
    for i in 0..count {
        let entry = offset + 4 + 12 * i;
        let hash = r.endian.read_u64(r.bytes, entry, "hash64 entry")?;
        let t = r.bytes[types_off + i];
        write_indent(w, depth + 1)?;
        write_hex_padded(w, hash, 16)?;
        w.write_all(b":")?;
        emit_child_stream(r, entry + 8, t, depth + 1, true, w)?;
        if i + 1 < count || after_key {
            writeln!(w)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip_small() {
        let mut d = BTreeMap::new();
        d.insert("name".into(), Value::String("hello world".into()));
        d.insert("count".into(), Value::U32(42));
        d.insert("scale".into(), Value::F32(1.5));
        d.insert("on".into(), Value::Bool(true));
        d.insert("noval".into(), Value::Null);
        let mut h = BTreeMap::new();
        h.insert(0xDEAD_BEEF, Value::I32(-5));
        d.insert("hashes".into(), Value::Hash32(h));
        d.insert(
            "items".into(),
            Value::Array(vec![Value::U32(1), Value::U32(2), Value::U32(3)]),
        );
        let b = Byml {
            version: 7,
            endian: Endian::Little,
            root: Value::Dict(d),
        };
        let v1 = b.to_bytes().unwrap();
        let mut out = Vec::new();
        emit_document_streaming(&v1, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let parsed = parse_document(&text).expect("parse");
        assert_eq!(parsed.version, b.version);
        let v2 = parsed.to_bytes().unwrap();
        assert_eq!(v1, v2);
    }

    fn yaml_round_trip(root: Value) {
        let b = Byml {
            version: 7,
            endian: Endian::Little,
            root,
        };
        let v1 = b.to_bytes().expect("serialize original");
        let mut out = Vec::new();
        emit_document_streaming(&v1, &mut out).expect("emit yaml");
        let text = String::from_utf8(out).expect("utf-8 yaml");
        let parsed = parse_document(&text).expect("parse yaml");
        let v2 = parsed.to_bytes().expect("serialize round-tripped");
        assert_eq!(v1, v2, "round trip mismatch for:\n{text}");
    }

    #[test]
    fn round_trip_wide_value_types() {
        let mut d = BTreeMap::new();
        d.insert("i64".into(), Value::I64(-9_000_000_000));
        d.insert("u64".into(), Value::U64(18_000_000_000));
        d.insert("f64".into(), Value::F64(2.5));
        d.insert("bin".into(), Value::Binary(vec![1, 2, 3, 4]));
        d.insert(
            "bin_align".into(),
            Value::BinaryAlign {
                data: vec![9, 8, 7],
                align: 16,
            },
        );
        d.insert("empty_arr".into(), Value::Array(vec![]));
        d.insert("empty_dict".into(), Value::Dict(BTreeMap::new()));
        let mut h = BTreeMap::new();
        h.insert(0x1234_5678_9abc_def0_u64, Value::String("x".into()));
        d.insert("h64".into(), Value::Hash64(h));
        yaml_round_trip(Value::Dict(d));
    }

    #[test]
    fn round_trip_special_floats() {
        let mut d = BTreeMap::new();
        d.insert("pos_inf".into(), Value::F32(f32::INFINITY));
        d.insert("neg_inf".into(), Value::F32(f32::NEG_INFINITY));
        d.insert(
            "nan_payload".into(),
            Value::F32(f32::from_bits(0x7fc0_0001)),
        );
        d.insert("d_inf".into(), Value::F64(f64::NEG_INFINITY));
        d.insert(
            "d_nan".into(),
            Value::F64(f64::from_bits(0x7ff8_0000_0000_0042)),
        );
        yaml_round_trip(Value::Dict(d));
    }

    #[test]
    fn round_trip_nested_containers() {
        let inner = Value::Dict(BTreeMap::from([
            ("a".into(), Value::U32(1)),
            (
                "b".into(),
                Value::Array(vec![Value::I32(-1), Value::I32(2)]),
            ),
        ]));
        yaml_round_trip(Value::Array(vec![inner, Value::Null, Value::Bool(false)]));
    }

    #[test]
    fn round_trip_null_root() {
        yaml_round_trip(Value::Null);
    }

    #[test]
    fn parse_document_rejects_malformed_input() {
        assert!(parse_document("").is_err(), "empty document");
        assert!(
            parse_document("- 1\n- 2").is_err(),
            "top level must be a mapping"
        );
        assert!(
            parse_document("version: 7\nendian: little\nroot: !u32 1\n---\nversion: 7").is_err(),
            "multiple documents"
        );
        assert!(
            parse_document("endian: little\nroot: !null ~").is_err(),
            "missing version"
        );
        assert!(
            parse_document("version: 7\nroot: !null ~").is_err(),
            "missing endian"
        );
        assert!(
            parse_document("version: 7\nendian: sideways\nroot: !null ~").is_err(),
            "bad endian"
        );
        assert!(
            parse_document("version: 7\nendian: little\nbogus: 1").is_err(),
            "unknown top-level key"
        );
        assert!(
            parse_document("version: 7\nendian: little\nroot: 5").is_err(),
            "untagged scalar root"
        );
        assert!(
            parse_document("version: 7\nendian: little\nroot: !nope 1").is_err(),
            "unknown tag"
        );
    }

    #[test]
    fn emit_does_not_panic_on_truncation() {
        let mut d = BTreeMap::new();
        d.insert("s".into(), Value::String("hello".into()));
        d.insert("n".into(), Value::U64(42));
        d.insert(
            "arr".into(),
            Value::Array(vec![Value::F32(1.0), Value::Binary(vec![1, 2, 3])]),
        );
        let valid = Byml {
            version: 7,
            endian: Endian::Little,
            root: Value::Dict(d),
        }
        .to_bytes()
        .unwrap();

        for cut in 0..valid.len() {
            let mut out = Vec::new();
            // Truncated input must be reported as an error, never panic.
            let _ = emit_document_streaming(&valid[..cut], &mut out);
        }
        let mut out = Vec::new();
        assert!(emit_document_streaming(&valid, &mut out).is_ok());
    }

    #[test]
    fn is_safe_plain_key_rules() {
        assert!(is_safe_plain_key("Name"));
        assert!(is_safe_plain_key("_x"));
        assert!(is_safe_plain_key("a.b/c-d_1"));
        assert!(!is_safe_plain_key(""));
        assert!(!is_safe_plain_key("1abc"));
        assert!(!is_safe_plain_key("has space"));
        assert!(!is_safe_plain_key("true"));
        assert!(!is_safe_plain_key("null"));
    }

    #[test]
    fn yaml_quote_escapes_control_and_specials() {
        let mut o = Vec::new();
        yaml_quote_to(&mut o, "a\"b\\c\n\td\u{1}").unwrap();
        assert_eq!(o, b"\"a\\\"b\\\\c\\n\\td\\x01\"");
    }

    #[test]
    fn float_string_parsing() {
        assert!(parse_float_str_f32(".inf").unwrap().is_infinite());
        assert!(parse_float_str_f32(".inf").unwrap().is_sign_positive());
        assert!(parse_float_str_f32("-.inf").unwrap().is_infinite());
        assert!(parse_float_str_f32("-.inf").unwrap().is_sign_negative());
        assert!(parse_float_str_f32(".nan").unwrap().is_nan());
        assert_eq!(
            parse_float_str_f32("nan(0x7fc00001)").unwrap().to_bits(),
            0x7fc0_0001
        );
        assert_eq!(
            parse_float_str_f64("1.5").unwrap().to_bits(),
            1.5_f64.to_bits()
        );
        assert!(parse_float_str_f64("not a float").is_err());
    }

    #[test]
    fn hex_string_parsing() {
        assert_eq!(parse_u32_hex("0xdeadbeef").unwrap(), 0xdead_beef);
        assert_eq!(parse_u32_hex("0XCAFE").unwrap(), 0xcafe);
        assert_eq!(parse_u32_hex("255").unwrap(), 255);
        assert!(parse_u32_hex("0xZZ").is_err());
        assert_eq!(
            parse_u64_hex("0x1234567890abcdef").unwrap(),
            0x1234_5678_90ab_cdef
        );
    }

    #[test]
    fn nan_and_hex_formatting() {
        let mut buf = [0u8; 16];
        assert_eq!(
            format_nan_bits(&mut buf, 0x7fc0_0001, 8),
            b"nan(0x7fc00001)"
        );

        let mut o = Vec::new();
        write_hex_padded(&mut o, 0xdead_beef, 8).unwrap();
        assert_eq!(o, b"0xdeadbeef");
    }

    #[test]
    fn root_summary_kinds() {
        assert_eq!(root_summary(&Value::Null), ("null", 0));
        assert_eq!(
            root_summary(&Value::Array(vec![Value::U32(1)])),
            ("array", 1)
        );
        assert_eq!(root_summary(&Value::U32(5)), ("scalar", 1));
    }
}
