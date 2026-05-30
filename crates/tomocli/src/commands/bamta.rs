use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use saphyr::{LoadableYamlNode, Yaml};
use tomolib::formats::amta::{Amta, ByteOrder, EnvelopePoint};

use crate::commands::yaml::{get, quote as yaml_quote};
use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, order_str, value};
use crate::hex;
use crate::paths::{append_ext, read_file, strip_ext, write_file};

#[derive(Debug, Args)]
pub(crate) struct BamtaArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Summarize an AMTA audio-metadata blob.
    Info {
        /// Path to the `.bamta` file.
        input: PathBuf,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Decompose a `.bamta` into a YAML document.
    Extract {
        /// Path to the `.bamta` file.
        input: PathBuf,
        /// Destination YAML file. Defaults to <input>.yml.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Rebuild a `.bamta` from a YAML document produced by `extract`.
    Pack {
        /// Path to the YAML file.
        input: PathBuf,
        /// Destination `.bamta` file. Defaults to <input> with the extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(args: BamtaArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input, common } => info(&input, common.json),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack { input, out } => pack(&input, out),
    }
}

fn load(path: &Path) -> Result<Amta> {
    let bytes = read_file(path)?;
    Amta::parse(&bytes).with_context(|| format!("parse `{}`", path.display()))
}

fn info(input: &Path, json: bool) -> Result<()> {
    let amta = load(input)?;
    let meta = fs::metadata(input).with_context(|| format!("stat `{}`", input.display()))?;
    let total = meta.len();
    let byte_order = order_str(amta.byte_order);

    if json {
        let obj = serde_json::json!({
            "file": input.display().to_string(),
            "byte_order": byte_order,
            "version": format!("{:#06x}", amta.version),
            "name": amta.name,
            "channels": amta.channels,
            "flags": format!("{:#010x}", amta.flags),
            "params": amta.params.iter().copied().map(json_f32).collect::<Vec<_>>(),
            "envelope_points": amta.envelope.len(),
            "total_size": total,
        });
        return crate::fmt::print_json(&obj);
    }

    let mut t = tabled::builder::Builder::default();
    let mut row = |k: &str, v: String, extra: String| {
        t.push_record([label(k), value(v), extra]);
    };
    row("Byte order", byte_order.to_string(), String::new());
    row("Version", format!("{:#06x}", amta.version), String::new());
    row("Name", amta.name.clone(), String::new());
    row("Channels", amta.channels.to_string(), String::new());
    row("Flags", format!("{:#010x}", amta.flags), String::new());
    row(
        "Params",
        amta.params
            .iter()
            .map(|p| format!("{p:.3}"))
            .collect::<Vec<_>>()
            .join(", "),
        String::new(),
    );
    row(
        "Envelope",
        format!("{} points", amta.envelope.len()),
        String::new(),
    );
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));
    Ok(())
}

pub(crate) fn convert_to_yaml(bytes: &[u8]) -> Result<Vec<u8>> {
    let amta = Amta::parse(bytes)?;
    Ok(emit_yaml(&amta).into_bytes())
}

pub(crate) fn yaml_to_bytes(text: &str) -> Result<Vec<u8>> {
    parse_yaml(text)?.to_bytes().map_err(Into::into)
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let amta = load(input)?;
    let out = out.unwrap_or_else(|| append_ext(input, "yml"));
    write_file(&out, emit_yaml(&amta).as_bytes())?;
    crate::fmt::report("extracted", input, &out, &amta.name);
    Ok(())
}

fn pack(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let text = fs::read_to_string(input).with_context(|| format!("read `{}`", input.display()))?;
    let amta = parse_yaml(&text).with_context(|| format!("parse `{}`", input.display()))?;
    let out = match out {
        Some(p) => p,
        None => strip_ext(input, &["yml", "yaml"])?,
    };
    let bytes = amta
        .to_bytes()
        .with_context(|| format!("build AMTA from `{}`", input.display()))?;
    write_file(&out, &bytes)?;
    crate::fmt::report("packed", input, &out, &fmt_bytes(bytes.len() as u64));
    Ok(())
}

fn fmt_f32(v: f32) -> String {
    if v.is_finite() {
        format!("{v:?}")
    } else if v.is_nan() {
        "\".nan\"".to_string()
    } else if v.is_sign_positive() {
        "\".inf\"".to_string()
    } else {
        "\"-.inf\"".to_string()
    }
}

fn json_f32(v: f32) -> serde_json::Value {
    if v.is_finite() {
        serde_json::json!(v)
    } else if v.is_nan() {
        serde_json::json!(".nan")
    } else if v.is_sign_positive() {
        serde_json::json!(".inf")
    } else {
        serde_json::json!("-.inf")
    }
}

fn emit_yaml(amta: &Amta) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "version: \"{:#06x}\"", amta.version);
    let _ = writeln!(s, "byte_order: {}", order_str(amta.byte_order));
    let _ = writeln!(s, "channels: {}", amta.channels);
    let _ = writeln!(s, "flags: \"{:#010x}\"", amta.flags);
    let _ = writeln!(s, "marker: \"{:#010x}\"", amta.marker);
    let _ = writeln!(s, "kind: {}", amta.kind);
    let _ = writeln!(s, "reserved: {}", amta.reserved);
    if amta.section_offsets.iter().any(Option::is_some) {
        let list = amta
            .section_offsets
            .iter()
            .map(|o| match o {
                Some(rel) => format!("\"{rel:#x}\""),
                None => "none".to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(s, "section_offsets: [{list}]");
    }
    let _ = writeln!(s, "name: {}", yaml_quote(&amta.name));
    let _ = writeln!(s, "params:");
    for p in &amta.params {
        let _ = writeln!(s, "  - {}", fmt_f32(*p));
    }
    let _ = writeln!(s, "envelope:");
    for e in &amta.envelope {
        let _ = writeln!(
            s,
            "  - {{ position: {}, value: {} }}",
            e.position,
            fmt_f32(e.value)
        );
    }
    if !amta.pre.is_empty() {
        let _ = writeln!(s, "pre: \"{}\"", hex::encode(&amta.pre));
    }
    if !amta.sections.is_empty() {
        let _ = writeln!(s, "sections: \"{}\"", hex::encode(&amta.sections));
    }
    if !amta.trailing.is_empty() {
        let _ = writeln!(s, "trailing: \"{}\"", hex::encode(&amta.trailing));
    }
    s
}

fn parse_yaml(text: &str) -> Result<Amta> {
    let docs = Yaml::load_from_str(text).context("parse bamta YAML")?;
    let doc = docs.first().context("empty bamta YAML")?;

    let version = u16::try_from(hex_field(doc, "version")?)
        .map_err(|_| anyhow::anyhow!("key `version`: out of range"))?;
    let byte_order = match str_field(doc, "byte_order")?.as_str() {
        "little" => ByteOrder::Little,
        "big" => ByteOrder::Big,
        other => bail!("unknown byte_order `{other}`"),
    };
    let channels = int_field(doc, "channels")?;
    let flags = hex_field(doc, "flags")?;
    let marker = hex_field(doc, "marker")?;
    let kind = int_field(doc, "kind")?;
    let reserved = int_field(doc, "reserved")?;
    let mut section_offsets = [None; 5];
    if let Some(seq) = get(doc, "section_offsets").and_then(Yaml::as_sequence) {
        if seq.len() != 5 {
            bail!("expected 5 section_offsets, got {}", seq.len());
        }
        for (slot, v) in section_offsets.iter_mut().zip(seq) {
            let s = v
                .as_str()
                .context("section_offsets entry must be a string")?;
            if s == "none" {
                continue;
            }
            let digits = s.strip_prefix("0x").unwrap_or(s);
            *slot = Some(
                u32::from_str_radix(digits, 16)
                    .with_context(|| format!("invalid section offset `{s}`"))?,
            );
        }
    }
    let name = str_field(doc, "name")?;

    let params_seq = get(doc, "params")
        .and_then(Yaml::as_sequence)
        .context("missing `params` sequence")?;
    if params_seq.len() != 4 {
        bail!("expected 4 params, got {}", params_seq.len());
    }
    let mut params = [0.0f32; 4];
    for (slot, v) in params.iter_mut().zip(params_seq) {
        *slot = as_f32(v)?;
    }

    let env_seq = get(doc, "envelope")
        .and_then(Yaml::as_sequence)
        .context("missing `envelope` sequence")?;
    let mut envelope = Vec::with_capacity(env_seq.len());
    for (i, e) in env_seq.iter().enumerate() {
        let position = int_field(e, "position").with_context(|| format!("envelope {i}"))?;
        let value = get(e, "value")
            .map(as_f32)
            .with_context(|| format!("envelope {i}: missing `value`"))??;
        envelope.push(EnvelopePoint { position, value });
    }

    let pre = hex_blob(doc, "pre")?;
    let sections = hex_blob(doc, "sections")?;
    let trailing = hex_blob(doc, "trailing")?;

    Ok(Amta {
        byte_order,
        version,
        channels,
        flags,
        section_offsets,
        marker,
        kind,
        reserved,
        params,
        envelope,
        name,
        pre,
        sections,
        trailing,
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn as_f32(y: &Yaml) -> Result<f32> {
    if let Some(f) = y.as_floating_point() {
        return Ok(f as f32);
    }
    if let Some(i) = y.as_integer() {
        return Ok(i as f32);
    }
    if let Some(s) = y.as_str() {
        return match s {
            ".nan" | "nan" | "NaN" => Ok(f32::NAN),
            ".inf" | "inf" | "+.inf" => Ok(f32::INFINITY),
            "-.inf" | "-inf" => Ok(f32::NEG_INFINITY),
            _ => bail!("expected number, got `{s}`"),
        };
    }
    bail!("expected number")
}

fn int_field<T: TryFrom<i64>>(map: &Yaml, key: &str) -> Result<T> {
    let v = get(map, key).with_context(|| format!("missing key `{key}`"))?;
    let n = v
        .as_integer()
        .with_context(|| format!("key `{key}`: expected integer"))?;
    T::try_from(n).map_err(|_| anyhow::anyhow!("key `{key}`: {n} out of range"))
}

fn str_field(map: &Yaml, key: &str) -> Result<String> {
    get(map, key)
        .and_then(Yaml::as_str)
        .map(ToString::to_string)
        .with_context(|| format!("missing string key `{key}`"))
}

fn hex_field(map: &Yaml, key: &str) -> Result<u32> {
    let s = str_field(map, key)?;
    let digits = s.strip_prefix("0x").unwrap_or(&s);
    u32::from_str_radix(digits, 16).with_context(|| format!("key `{key}`: invalid hex `{s}`"))
}

fn hex_blob(map: &Yaml, key: &str) -> Result<Vec<u8>> {
    match get(map, key).and_then(Yaml::as_str) {
        Some(s) => hex::decode(s).with_context(|| format!("key `{key}`: invalid hex")),
        None => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Amta {
        Amta {
            byte_order: ByteOrder::Little,
            version: 0x0500,
            channels: 2,
            flags: 0x0400_0101,
            section_offsets: [None; 5],
            marker: 0x6f,
            kind: 2,
            reserved: 0,
            params: [0.35, 0.08, -22.8, -27.4],
            envelope: vec![
                EnvelopePoint {
                    position: 0,
                    value: 1.0,
                },
                EnvelopePoint {
                    position: 4800,
                    value: 0.5,
                },
            ],
            name: "SE_Test_Track".to_string(),
            pre: Vec::new(),
            sections: Vec::new(),
            trailing: Vec::new(),
        }
    }

    #[test]
    fn yaml_round_trip_preserves_floats() {
        let amta = sample();
        let parsed = parse_yaml(&emit_yaml(&amta)).unwrap();
        assert_eq!(
            parsed.params.map(f32::to_bits),
            amta.params.map(f32::to_bits)
        );
        assert_eq!(parsed.envelope.len(), amta.envelope.len());
        for (a, b) in parsed.envelope.iter().zip(&amta.envelope) {
            assert_eq!(a.position, b.position);
            assert_eq!(a.value.to_bits(), b.value.to_bits());
        }
    }

    #[test]
    fn yaml_round_trip_handles_non_finite_params() {
        let mut amta = sample();
        amta.params = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0];
        let parsed = parse_yaml(&emit_yaml(&amta)).unwrap();
        assert!(parsed.params[0].is_nan());
        assert!(parsed.params[1].is_infinite() && parsed.params[1].is_sign_positive());
        assert!(parsed.params[2].is_infinite() && parsed.params[2].is_sign_negative());
        assert_eq!(parsed.params[3].to_bits(), 1.0f32.to_bits());
    }
}
