use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use saphyr::{LoadableYamlNode, Yaml};
use tomolib::formats::bnvib::{self, Bnvib};

use crate::commands::yaml::get;
use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, read_file, strip_ext, write_file};

#[derive(Debug, Args)]
pub(crate) struct BnvibArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Summarize a BNVIB vibration file.
    Info {
        /// Path to the BNVIB file.
        input: PathBuf,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Decompose a BNVIB into a YAML document.
    Extract {
        /// Path to the BNVIB file.
        input: PathBuf,
        /// Destination YAML file. Defaults to <input>.yml.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Rebuild a BNVIB from a YAML document produced by `extract`.
    Pack {
        /// Path to the YAML file.
        input: PathBuf,
        /// Destination BNVIB file. Defaults to <input> with the extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(args: BnvibArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input, common } => info(&input, common.json),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack { input, out } => pack(&input, out),
    }
}

fn load(path: &Path) -> Result<Bnvib> {
    let bytes = read_file(path)?;
    Bnvib::parse(&bytes).with_context(|| format!("parse `{}`", path.display()))
}

fn type_name(vib_type: u8) -> &'static str {
    match vib_type {
        bnvib::TYPE_NORMAL => "normal",
        bnvib::TYPE_LOOP => "loop",
        bnvib::TYPE_LOOP_WAIT => "loop+wait",
        _ => "unknown",
    }
}

fn duration_secs(vib: &Bnvib) -> f64 {
    if vib.sample_rate == 0 {
        0.0
    } else {
        let count = u32::try_from(vib.samples.len()).unwrap_or(u32::MAX);
        f64::from(count) / f64::from(vib.sample_rate)
    }
}

fn info(input: &Path, json: bool) -> Result<()> {
    let vib = load(input)?;
    let meta = fs::metadata(input).with_context(|| format!("stat `{}`", input.display()))?;
    let total = meta.len();

    if json {
        let obj = serde_json::json!({
            "file": input.display().to_string(),
            "type": vib.vib_type,
            "type_name": type_name(vib.vib_type),
            "version": vib.version,
            "sample_rate": vib.sample_rate,
            "sample_count": vib.samples.len(),
            "duration_secs": duration_secs(&vib),
            "loop": if vib.is_loop() {
                serde_json::json!({
                    "start": vib.loop_start,
                    "end": vib.loop_end,
                    "wait": if vib.has_wait() { Some(vib.loop_wait) } else { None },
                })
            } else {
                serde_json::Value::Null
            },
            "total_size": total,
        });
        return crate::fmt::print_json(&obj);
    }

    let mut t = tabled::builder::Builder::default();
    let mut row = |k: &str, v: String, extra: String| {
        t.push_record([label(k), value(v), extra]);
    };
    row(
        "Type",
        format!("{} ({:#04x})", type_name(vib.vib_type), vib.vib_type),
        String::new(),
    );
    row("Version", format!("{:#04x}", vib.version), String::new());
    row(
        "Sample rate",
        format!("{} Hz", vib.sample_rate),
        String::new(),
    );
    row("Samples", vib.samples.len().to_string(), String::new());
    row(
        "Duration",
        format!("{:.2}s", duration_secs(&vib)),
        String::new(),
    );
    if vib.is_loop() {
        row(
            "Loop",
            format!("{}..{}", vib.loop_start, vib.loop_end),
            String::new(),
        );
        if vib.has_wait() {
            row("Loop wait", vib.loop_wait.to_string(), String::new());
        }
    }
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));
    Ok(())
}

pub(crate) fn convert_to_yaml(bytes: &[u8]) -> Result<Vec<u8>> {
    let vib = Bnvib::parse(bytes)?;
    Ok(emit_yaml(&vib).into_bytes())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let vib = load(input)?;
    let out = out.unwrap_or_else(|| append_ext(input, "yml"));
    write_file(&out, emit_yaml(&vib).as_bytes())?;
    crate::fmt::report(
        "extracted",
        input,
        &out,
        &format!("{} samples", vib.samples.len()),
    );
    Ok(())
}

fn pack(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let text = fs::read_to_string(input).with_context(|| format!("read `{}`", input.display()))?;
    let vib = parse_yaml(&text).with_context(|| format!("parse `{}`", input.display()))?;
    let out = match out {
        Some(p) => p,
        None => strip_ext(input, &["yml", "yaml"])?,
    };
    let bytes = vib.to_bytes();
    write_file(&out, &bytes)?;
    crate::fmt::report("packed", input, &out, &fmt_bytes(bytes.len() as u64));
    Ok(())
}

fn emit_yaml(vib: &Bnvib) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "type: {}", vib.vib_type);
    let _ = writeln!(s, "version: {}", vib.version);
    let _ = writeln!(s, "sample_rate: {}", vib.sample_rate);
    if vib.is_loop() {
        let _ = writeln!(s, "loop_start: {}", vib.loop_start);
        let _ = writeln!(s, "loop_end: {}", vib.loop_end);
    }
    if vib.has_wait() {
        let _ = writeln!(s, "loop_wait: {}", vib.loop_wait);
    }
    let _ = writeln!(s, "samples:");
    for sample in &vib.samples {
        let [lo_amp, lo_freq, hi_amp, hi_freq] = sample.to_le_bytes();
        let _ = writeln!(
            s,
            "  - {{ lo_amp: {lo_amp}, lo_freq: {lo_freq}, hi_amp: {hi_amp}, hi_freq: {hi_freq} }}"
        );
    }
    s
}

fn parse_yaml(text: &str) -> Result<Bnvib> {
    let docs = Yaml::load_from_str(text).context("parse bnvib YAML")?;
    let doc = docs.first().context("empty bnvib YAML")?;

    let vib_type: u8 = int_field(doc, "type")?;
    let version: u8 = int_field(doc, "version")?;
    let sample_rate: u16 = int_field(doc, "sample_rate")?;

    let is_loop = matches!(vib_type, bnvib::TYPE_LOOP | bnvib::TYPE_LOOP_WAIT);
    let has_wait = vib_type == bnvib::TYPE_LOOP_WAIT;
    let loop_start = if is_loop {
        int_field(doc, "loop_start")?
    } else {
        0
    };
    let loop_end = if is_loop {
        int_field(doc, "loop_end")?
    } else {
        0
    };
    let loop_wait = if has_wait {
        int_field(doc, "loop_wait")?
    } else {
        0
    };

    let seq = get(doc, "samples")
        .and_then(Yaml::as_sequence)
        .context("missing `samples` sequence")?;
    let mut samples = Vec::with_capacity(seq.len());
    for (i, v) in seq.iter().enumerate() {
        let lo_amp: u8 = int_field(v, "lo_amp").with_context(|| format!("sample {i}"))?;
        let lo_freq: u8 = int_field(v, "lo_freq").with_context(|| format!("sample {i}"))?;
        let hi_amp: u8 = int_field(v, "hi_amp").with_context(|| format!("sample {i}"))?;
        let hi_freq: u8 = int_field(v, "hi_freq").with_context(|| format!("sample {i}"))?;
        samples.push(u32::from_le_bytes([lo_amp, lo_freq, hi_amp, hi_freq]));
    }

    Ok(Bnvib {
        vib_type,
        version,
        sample_rate,
        loop_start,
        loop_end,
        loop_wait,
        samples,
    })
}

fn int_field<T: TryFrom<i64>>(map: &Yaml, key: &str) -> Result<T> {
    let v = get(map, key).with_context(|| format!("missing key `{key}`"))?;
    let n = v
        .as_integer()
        .with_context(|| format!("key `{key}`: expected integer"))?;
    T::try_from(n).map_err(|_| anyhow::anyhow!("key `{key}`: {n} out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(vib: &Bnvib) {
        let yaml = emit_yaml(vib);
        let parsed = parse_yaml(&yaml).unwrap();
        assert_eq!(&parsed, vib);
    }

    fn samples(n: u32) -> Vec<u32> {
        (0..n).map(|i| i.wrapping_mul(0x1234_5678)).collect()
    }

    #[test]
    fn normal_yaml_round_trip() {
        round_trip(&Bnvib {
            vib_type: bnvib::TYPE_NORMAL,
            version: bnvib::VERSION,
            sample_rate: 200,
            loop_start: 0,
            loop_end: 0,
            loop_wait: 0,
            samples: samples(58),
        });
    }

    #[test]
    fn loop_yaml_round_trip() {
        round_trip(&Bnvib {
            vib_type: bnvib::TYPE_LOOP,
            version: bnvib::VERSION,
            sample_rate: 200,
            loop_start: 4,
            loop_end: 453,
            loop_wait: 0,
            samples: samples(612),
        });
    }

    #[test]
    fn loop_wait_yaml_round_trip() {
        round_trip(&Bnvib {
            vib_type: bnvib::TYPE_LOOP_WAIT,
            version: bnvib::VERSION,
            sample_rate: 200,
            loop_start: 1,
            loop_end: 99,
            loop_wait: 7,
            samples: samples(100),
        });
    }

    #[test]
    fn sample_byte_lanes_preserved() {
        let yaml = emit_yaml(&Bnvib {
            vib_type: bnvib::TYPE_NORMAL,
            version: bnvib::VERSION,
            sample_rate: 200,
            loop_start: 0,
            loop_end: 0,
            loop_wait: 0,
            samples: vec![u32::from_le_bytes([1, 139, 5, 158])],
        });
        assert!(yaml.contains("lo_amp: 1, lo_freq: 139, hi_amp: 5, hi_freq: 158"));
    }
}
