use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use saphyr::{LoadableYamlNode, Yaml};
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::bwav::{self, Bwav, BwavChannel, ByteOrder, PackChannel};

use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, read_file, write_file};

const MANIFEST: &str = "bwav.yml";

#[derive(Debug, Args)]
pub(crate) struct BwavArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of a BWAV waveform.
    Info {
        /// Path to the BWAV file.
        input: PathBuf,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Decompose a BWAV into a directory (YAML sidecar + raw channel blobs).
    Extract {
        /// Path to the BWAV file.
        input: PathBuf,
        /// Destination directory. Defaults to <input> with the extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Also decode every channel to a RIFF/WAVE file.
        #[arg(long)]
        wav: bool,
    },
    /// Rebuild a BWAV from a directory produced by `extract`.
    Pack {
        /// Directory containing bwav.yml and channel blobs.
        input: PathBuf,
        /// Destination BWAV file. Defaults to <input>.bwav.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(args: BwavArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input, common } => info(&input, common.json),
        Verb::Extract { input, out, wav } => extract(&input, out, wav),
        Verb::Pack { input, out } => pack(&input, out),
    }
}

fn load(path: &Path) -> Result<Bwav> {
    let bytes = read_file(path)?;
    Bwav::parse(bytes).with_context(|| format!("parse `{}`", path.display()))
}

fn codec_name(codec: u16) -> String {
    match codec {
        bwav::CODEC_PCM16 => "PCM16".to_string(),
        bwav::CODEC_DSP_ADPCM => "DSP-ADPCM".to_string(),
        other => format!("codec {other}"),
    }
}

fn channel_duration(ch: &BwavChannel) -> f64 {
    if ch.sample_rate == 0 {
        0.0
    } else {
        f64::from(ch.sample_count) / f64::from(ch.sample_rate)
    }
}

fn info(input: &Path, json: bool) -> Result<()> {
    let bwav = load(input)?;
    let meta = fs::metadata(input).with_context(|| format!("stat `{}`", input.display()))?;
    let byte_order = match bwav.byte_order() {
        ByteOrder::Little => "little",
        ByteOrder::Big => "big",
    };

    if json {
        let channels: Vec<_> = bwav
            .channels()
            .iter()
            .map(|ch| {
                serde_json::json!({
                    "codec": ch.codec,
                    "codec_name": codec_name(ch.codec),
                    "sample_rate": ch.sample_rate,
                    "sample_count": ch.sample_count,
                    "duration_secs": channel_duration(ch),
                    "loop": if ch.loop_flag != 0 {
                        serde_json::json!({ "start": ch.loop_start, "end": ch.loop_end })
                    } else {
                        serde_json::Value::Null
                    },
                })
            })
            .collect();
        let obj = serde_json::json!({
            "file": input.display().to_string(),
            "byte_order": byte_order,
            "version": bwav.version(),
            "prefetch": bwav.prefetch(),
            "sample_hash": bwav.hash(),
            "total_size": meta.len(),
            "channels": channels,
        });
        return crate::fmt::print_json(&obj);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String, extra: String| {
        t.push_record([label(k), value(v), extra]);
    };
    row("Byte order", byte_order.to_string(), String::new());
    row("Version", format!("{:#06x}", bwav.version()), String::new());
    row("Channels", bwav.channels().len().to_string(), String::new());
    row("Prefetch", bwav.prefetch().to_string(), String::new());
    row(
        "Sample hash",
        format!("{:#010x}", bwav.hash()),
        String::new(),
    );
    let total = meta.len();
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    println!();
    let mut b = Builder::default();
    b.push_record(["#", "codec", "rate", "samples", "duration", "loop"]);
    for (i, ch) in bwav.channels().iter().enumerate() {
        let secs = channel_duration(ch);
        let looping = if ch.loop_flag != 0 {
            format!("{}..{}", ch.loop_start, ch.loop_end)
        } else {
            "no".to_string()
        };
        b.push_record([
            i.to_string(),
            codec_name(ch.codec),
            format!("{} Hz", ch.sample_rate),
            ch.sample_count.to_string(),
            format!("{secs:.2}s"),
            looping,
        ]);
    }
    let mut tt = b.build();
    tt.with(Style::blank()).with(Padding::new(2, 2, 0, 0));
    println!("{tt}");

    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>, wav: bool) -> Result<()> {
    let bwav = load(input)?;
    let out_dir = out.unwrap_or_else(|| append_ext(input, "d"));
    write_bundle(&bwav, &out_dir, wav)?;
    println!(
        "extracted {} -> {} ({} channels{})",
        input.display(),
        out_dir.display(),
        bwav.channels().len(),
        if wav { ", +wav" } else { "" },
    );
    Ok(())
}

pub(crate) fn write_bundle(bwav: &Bwav, dir: &Path, wav: bool) -> Result<u64> {
    fs::create_dir_all(dir).with_context(|| format!("create `{}`", dir.display()))?;
    let mut total = 0u64;

    for (i, ch) in bwav.channels().iter().enumerate() {
        let data = bwav.channel_data(ch);
        write_file(&dir.join(channel_blob_name(i)), data)?;
        total += data.len() as u64;
    }

    let yaml = emit_yaml(bwav);
    write_file(&dir.join(MANIFEST), yaml.as_bytes())?;
    total += yaml.len() as u64;

    if wav {
        let mut decoded = Vec::with_capacity(bwav.channels().len());
        for (i, ch) in bwav.channels().iter().enumerate() {
            decoded.push((ch.sample_rate, bwav.decode_channel(i)?));
        }
        let wav_bytes = bwav::build_wav(&decoded);
        write_file(&dir.join(wav_name(dir)), &wav_bytes)?;
        total += wav_bytes.len() as u64;
    }

    Ok(total)
}

pub(crate) fn convert_to_bundle(bytes: &[u8], dir: &Path, wav: bool) -> Result<u64> {
    let bwav = Bwav::parse(bytes.to_vec())?;
    write_bundle(&bwav, dir, wav)
}

fn strip_d(dir: &Path, ext: &str) -> PathBuf {
    if dir.extension().is_some_and(|e| e == "d") {
        dir.with_extension("")
    } else {
        append_ext(dir, ext)
    }
}

fn wav_name(dir: &Path) -> String {
    let mut stem = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("audio")
        .to_string();
    for ext in [".d", ".bwav"] {
        if let Some(p) = stem.strip_suffix(ext) {
            stem = p.to_string();
        }
    }
    format!("{stem}.wav")
}

fn pack(input: &Path, out: Option<PathBuf>) -> Result<()> {
    if !input.is_dir() {
        bail!(
            "input `{}` must be a directory from `extract`",
            input.display()
        );
    }
    let bytes = pack_bundle(input)?;
    let out = out.unwrap_or_else(|| strip_d(input, "bwav"));
    write_file(&out, &bytes)?;
    let n = bytes.len() as u64;
    println!(
        "packed {} -> {} ({})",
        input.display(),
        out.display(),
        fmt_bytes(n),
    );
    Ok(())
}

pub(crate) fn pack_bundle(dir: &Path) -> Result<Vec<u8>> {
    let manifest_path = dir.join(MANIFEST);
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read `{}`", manifest_path.display()))?;
    let doc = parse_yaml(&text)?;

    let mut datas = Vec::with_capacity(doc.channels.len());
    for ch in &doc.channels {
        datas.push(read_file(&dir.join(&ch.data))?);
    }
    let channels: Vec<PackChannel<'_>> = doc
        .channels
        .iter()
        .zip(&datas)
        .map(|(c, d)| PackChannel {
            info: c.info.clone(),
            data: d.as_slice(),
        })
        .collect();

    let mut buf = Vec::new();
    bwav::write(
        &mut buf,
        doc.byte_order,
        doc.version,
        doc.hash,
        doc.prefetch,
        &channels,
    )
    .with_context(|| format!("build BWAV from `{}`", dir.display()))?;
    Ok(buf)
}

fn channel_blob_name(i: usize) -> String {
    format!("channel{i:02}.bin")
}

fn emit_yaml(bwav: &Bwav) -> String {
    let mut s = String::new();
    let order = match bwav.byte_order() {
        ByteOrder::Little => "little",
        ByteOrder::Big => "big",
    };
    let _ = writeln!(s, "version: {}", bwav.version());
    let _ = writeln!(s, "byte_order: {order}");
    let _ = writeln!(s, "hash: {}", bwav.hash());
    let _ = writeln!(s, "prefetch: {}", bwav.prefetch());
    let _ = writeln!(s, "channels:");
    for (i, ch) in bwav.channels().iter().enumerate() {
        let coefs = ch
            .coefficients
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(s, "  - codec: {}", ch.codec);
        let _ = writeln!(s, "    channel_pan: {}", ch.channel_pan);
        let _ = writeln!(s, "    sample_rate: {}", ch.sample_rate);
        let _ = writeln!(s, "    sample_count_full: {}", ch.sample_count_full);
        let _ = writeln!(s, "    sample_count: {}", ch.sample_count);
        let _ = writeln!(s, "    data_offset_full: {}", ch.data_offset_full);
        let _ = writeln!(s, "    data_offset: {}", ch.data_offset);
        let _ = writeln!(s, "    loop_flag: {}", ch.loop_flag);
        let _ = writeln!(s, "    loop_end: {}", ch.loop_end);
        let _ = writeln!(s, "    loop_start: {}", ch.loop_start);
        let _ = writeln!(s, "    predictor_scale: {}", ch.predictor_scale);
        let _ = writeln!(s, "    history1: {}", ch.history1);
        let _ = writeln!(s, "    history2: {}", ch.history2);
        let _ = writeln!(s, "    reserved: {}", ch.reserved);
        let _ = writeln!(s, "    coefficients: [{coefs}]");
        let _ = writeln!(s, "    data: {}", channel_blob_name(i));
    }
    s
}

struct ChannelDoc {
    info: BwavChannel,
    data: String,
}

struct BwavDoc {
    version: u16,
    byte_order: ByteOrder,
    hash: u32,
    prefetch: u16,
    channels: Vec<ChannelDoc>,
}

fn parse_yaml(text: &str) -> Result<BwavDoc> {
    let docs = Yaml::load_from_str(text).context("parse bwav.yml")?;
    let doc = docs.first().context("empty bwav.yml")?;

    let version = int_field(doc, "version")?;
    let byte_order = match str_field(doc, "byte_order")?.as_str() {
        "little" => ByteOrder::Little,
        "big" => ByteOrder::Big,
        other => bail!("unknown byte_order `{other}`"),
    };
    let hash = int_field(doc, "hash")?;
    let prefetch = int_field(doc, "prefetch")?;

    let seq = get(doc, "channels")
        .and_then(Yaml::as_sequence)
        .context("missing `channels` sequence")?;
    let mut channels = Vec::with_capacity(seq.len());
    for (i, c) in seq.iter().enumerate() {
        let coefs_seq = get(c, "coefficients")
            .and_then(Yaml::as_sequence)
            .with_context(|| format!("channel {i}: missing coefficients"))?;
        if coefs_seq.len() != 16 {
            bail!(
                "channel {i}: expected 16 coefficients, got {}",
                coefs_seq.len()
            );
        }
        let mut coefficients = [0i16; 16];
        for (slot, v) in coefficients.iter_mut().zip(coefs_seq) {
            *slot = as_int(v)?;
        }
        let info = BwavChannel {
            codec: int_field(c, "codec")?,
            channel_pan: int_field(c, "channel_pan")?,
            sample_rate: int_field(c, "sample_rate")?,
            sample_count_full: int_field(c, "sample_count_full")?,
            sample_count: int_field(c, "sample_count")?,
            coefficients,
            data_offset_full: int_field(c, "data_offset_full")?,
            data_offset: int_field(c, "data_offset")?,
            loop_flag: int_field(c, "loop_flag")?,
            loop_end: int_field(c, "loop_end")?,
            loop_start: int_field(c, "loop_start")?,
            predictor_scale: int_field(c, "predictor_scale")?,
            history1: int_field(c, "history1")?,
            history2: int_field(c, "history2")?,
            reserved: int_field(c, "reserved")?,
        };
        channels.push(ChannelDoc {
            info,
            data: str_field(c, "data")?,
        });
    }

    Ok(BwavDoc {
        version,
        byte_order,
        hash,
        prefetch,
        channels,
    })
}

fn get<'a, 'b>(map: &'a Yaml<'b>, key: &str) -> Option<&'a Yaml<'b>> {
    map.as_mapping()?
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .map(|(_, v)| v)
}

fn as_int<T: TryFrom<i64>>(y: &Yaml) -> Result<T> {
    let n = y.as_integer().context("expected integer")?;
    T::try_from(n).map_err(|_| anyhow::anyhow!("integer {n} out of range"))
}

fn int_field<T: TryFrom<i64>>(map: &Yaml, key: &str) -> Result<T> {
    let v = get(map, key).with_context(|| format!("missing key `{key}`"))?;
    as_int(v).with_context(|| format!("key `{key}`"))
}

fn str_field(map: &Yaml, key: &str) -> Result<String> {
    get(map, key)
        .and_then(Yaml::as_str)
        .map(ToString::to_string)
        .with_context(|| format!("missing string key `{key}`"))
}
