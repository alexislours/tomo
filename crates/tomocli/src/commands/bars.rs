use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use saphyr::{LoadableYamlNode, Yaml};
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::bars::{self, Bars, ByteOrder, PackEntry};

use crate::commands::yaml::{get, quote as yaml_quote};
use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::hex;
use crate::paths::{append_ext, read_file, write_file};

const MANIFEST: &str = "bars.yml";

#[derive(Debug, Args)]
pub(crate) struct BarsArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of a BARS audio archive.
    Info {
        /// Path to the BARS file.
        input: PathBuf,
        /// List every asset instead of just a summary.
        #[arg(short, long)]
        list: bool,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Extract a BARS into a directory of `.bwav` + `.bamta` files.
    Extract {
        /// Path to the BARS file.
        input: PathBuf,
        /// Destination directory. Defaults to <input> with the extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Rebuild a BARS from a directory produced by `extract`.
    Pack {
        /// Directory containing bars.yml and the asset/metadata files.
        input: PathBuf,
        /// Destination BARS file. Defaults to <input>.bars.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Replace a single asset's waveform and rebuild the container.
    Patch {
        /// Path to the BARS file.
        input: PathBuf,
        /// Name of the asset to replace.
        asset: String,
        /// New BWAV file to inject.
        bwav: PathBuf,
        /// Destination BARS file. Defaults to overwriting <input>.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(args: BarsArgs) -> Result<()> {
    match args.verb {
        Verb::Info {
            input,
            list,
            common,
        } => info(&input, list, common.json),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack { input, out } => pack(&input, out),
        Verb::Patch {
            input,
            asset,
            bwav,
            out,
        } => patch(&input, &asset, &bwav, out),
    }
}

fn load(path: &Path) -> Result<Bars> {
    let bytes = read_file(path)?;
    Bars::parse(bytes).with_context(|| format!("parse `{}`", path.display()))
}

fn info(input: &Path, list: bool, json: bool) -> Result<()> {
    let bars = load(input)?;
    let entries = bars.entries();

    let byte_order = match bars.byte_order() {
        ByteOrder::Little => "little",
        ByteOrder::Big => "big",
    };
    let with_asset = entries.iter().filter(|e| bars.asset(e).is_some()).count();

    if json {
        let mut obj = serde_json::json!({
            "file": input.display().to_string(),
            "byte_order": byte_order,
            "version": bars.version(),
            "assets": entries.len(),
            "with_waveform": with_asset,
            "total_size": bars.total_size(),
        });
        if list {
            obj["entries"] = entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "hash": e.hash,
                        "name": e.name,
                        "asset_size": bars.asset(e).map(<[u8]>::len),
                    })
                })
                .collect();
        }
        return crate::fmt::print_json(&obj);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String, extra: String| {
        t.push_record([label(k), value(v), extra]);
    };
    row("Byte order", byte_order.to_string(), String::new());
    row("Version", format!("{:#06x}", bars.version()), String::new());
    row("Assets", entries.len().to_string(), String::new());
    row("With waveform", with_asset.to_string(), String::new());
    let total = bars.total_size() as u64;
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    if list {
        println!();
        let mut b = Builder::default();
        b.push_record(["#", "hash", "asset", "name"]);
        for (i, e) in entries.iter().enumerate() {
            let asset = bars
                .asset(e)
                .map_or_else(|| "-".to_string(), |a| fmt_bytes(a.len() as u64));
            b.push_record([
                i.to_string(),
                format!("{:#010x}", e.hash),
                asset,
                e.name.clone(),
            ]);
        }
        let mut tt = b.build();
        tt.with(Style::blank()).with(Padding::new(2, 2, 0, 0));
        println!("{tt}");
    } else if !entries.is_empty() {
        println!();
        println!("  {}", "first assets:".dimmed());
        for e in entries.iter().take(5) {
            println!("    {}", e.name);
        }
        if entries.len() > 5 {
            println!(
                "    {}",
                format!("... and {} more", entries.len() - 5).dimmed()
            );
        }
    }

    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let bars = load(input)?;
    let out_dir = out.unwrap_or_else(|| append_ext(input, "d"));
    write_bundle(&bars, &out_dir, BundleOpts::default())?;
    crate::fmt::report(
        "extracted",
        input,
        &out_dir,
        &format!("{} assets", bars.entries().len()),
    );
    Ok(())
}

#[derive(Default, Clone, Copy)]
struct BundleOpts {
    recurse_bwav: bool,
    convert_meta: bool,
    wav: bool,
}

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_field_names)]
pub(crate) struct InnerConverts {
    pub bamta_files: u64,
    pub bamta_in: u64,
    pub bamta_out: u64,
    pub bamta_dur: Duration,
}

fn write_bundle(bars: &Bars, dir: &Path, opts: BundleOpts) -> Result<(u64, InnerConverts)> {
    fs::create_dir_all(dir).with_context(|| format!("create `{}`", dir.display()))?;
    let mut total = 0u64;
    let mut inner = InnerConverts::default();

    let mut manifest = String::new();
    let order = match bars.byte_order() {
        ByteOrder::Little => "little",
        ByteOrder::Big => "big",
    };
    let _ = writeln!(manifest, "version: {}", bars.version());
    let _ = writeln!(manifest, "byte_order: {order}");
    let _ = writeln!(manifest, "reset_table: {}", hex::encode(bars.reset_table()));
    let _ = writeln!(manifest, "entries:");

    let mut used: HashSet<String> = HashSet::new();
    for (i, e) in bars.entries().iter().enumerate() {
        let base = sanitize(&e.name);
        let mut stem = base.clone();
        let mut suffix = i;
        while !used.insert(stem.clone()) {
            stem = format!("{base}_{suffix}");
            suffix += 1;
        }
        let meta = bars.meta(e);
        let converted = if opts.convert_meta {
            let started = Instant::now();
            super::bamta::convert_to_yaml(meta)
                .ok()
                .map(|body| (body, started.elapsed()))
        } else {
            None
        };
        let meta_name = if let Some((body, dur)) = converted {
            let yml = format!("{stem}.bamta.yml");
            inner.bamta_dur += dur;
            write_file(&dir.join(&yml), &body)?;
            total += body.len() as u64;
            inner.bamta_files += 1;
            inner.bamta_in += meta.len() as u64;
            inner.bamta_out += body.len() as u64;
            yml
        } else {
            let bamta = format!("{stem}.bamta");
            write_file(&dir.join(&bamta), meta)?;
            total += meta.len() as u64;
            bamta
        };
        let _ = writeln!(manifest, "  - name: {}", yaml_quote(&e.name));
        let _ = writeln!(manifest, "    meta: {}", yaml_quote(&meta_name));

        if let Some(asset) = bars.asset(e) {
            let is_bwav = asset.starts_with(&tomolib::formats::bwav::BWAV_MAGIC);
            if opts.recurse_bwav && is_bwav {
                let bundle = format!("{stem}.bwav.d");
                let parsed = tomolib::formats::bwav::Bwav::parse(asset.to_vec())
                    .with_context(|| format!("parse BWAV asset `{}`", e.name))?;
                total += super::bwav::write_bundle(&parsed, &dir.join(&bundle), opts.wav)?;
                let _ = writeln!(manifest, "    asset: {}", yaml_quote(&bundle));
            } else {
                let bwav = format!("{stem}.bwav");
                write_file(&dir.join(&bwav), asset)?;
                total += asset.len() as u64;
                let _ = writeln!(manifest, "    asset: {}", yaml_quote(&bwav));
            }
        }
    }

    write_file(&dir.join(MANIFEST), manifest.as_bytes())?;
    total += manifest.len() as u64;
    Ok((total, inner))
}

pub(crate) fn convert_to_bundle(
    bytes: &[u8],
    dir: &Path,
    wav: bool,
) -> Result<(u64, InnerConverts)> {
    let bars = Bars::parse(bytes.to_vec())?;
    write_bundle(
        &bars,
        dir,
        BundleOpts {
            recurse_bwav: true,
            convert_meta: true,
            wav,
        },
    )
}

fn pack(input: &Path, out: Option<PathBuf>) -> Result<()> {
    if !input.is_dir() {
        bail!(
            "input `{}` must be a directory from `extract`",
            input.display()
        );
    }
    let manifest_path = input.join(MANIFEST);
    let text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read `{}`", manifest_path.display()))?;
    let doc = parse_manifest(&text)?;

    let mut metas = Vec::with_capacity(doc.entries.len());
    let mut assets = Vec::with_capacity(doc.entries.len());
    for e in &doc.entries {
        let meta_path = input.join(&e.meta);
        let is_yaml = meta_path
            .extension()
            .is_some_and(|x| x == "yml" || x == "yaml");
        metas.push(if is_yaml {
            let text = fs::read_to_string(&meta_path)
                .with_context(|| format!("read `{}`", meta_path.display()))?;
            super::bamta::yaml_to_bytes(&text)
                .with_context(|| format!("rebuild AMTA from `{}`", meta_path.display()))?
        } else {
            read_file(&meta_path)?
        });
        assets.push(match &e.asset {
            Some(p) => {
                let path = input.join(p);
                if path.is_dir() {
                    Some(super::bwav::pack_bundle(&path)?)
                } else {
                    Some(read_file(&path)?)
                }
            }
            None => None,
        });
    }

    let entries: Vec<PackEntry<'_>> = doc
        .entries
        .iter()
        .zip(metas.iter().zip(&assets))
        .map(|(e, (meta, asset))| PackEntry {
            name: &e.name,
            meta: meta.as_slice(),
            asset: asset.as_deref(),
        })
        .collect();

    let out = out.unwrap_or_else(|| strip_d(input, "bars"));
    let total = write_bars(
        &out,
        &entries,
        doc.byte_order,
        doc.version,
        &doc.reset_table,
        false,
    )?;
    crate::fmt::report(
        "packed",
        input,
        &out,
        &format!("{} assets, {}", entries.len(), fmt_bytes(total)),
    );
    Ok(())
}

fn patch(input: &Path, asset_name: &str, new_bwav: &Path, out: Option<PathBuf>) -> Result<()> {
    let bars = load(input)?;
    if !bars.entries().iter().any(|e| e.name == asset_name) {
        bail!("asset `{asset_name}` not found in `{}`", input.display());
    }
    let new_bytes = read_file(new_bwav)?;

    let entries: Vec<PackEntry<'_>> = bars
        .entries()
        .iter()
        .map(|e| {
            let asset = if e.name == asset_name {
                Some(new_bytes.as_slice())
            } else {
                bars.asset(e)
            };
            PackEntry {
                name: &e.name,
                meta: bars.meta(e),
                asset,
            }
        })
        .collect();

    let out = out.unwrap_or_else(|| input.to_path_buf());
    let in_place = out == input;
    let total = write_bars(
        &out,
        &entries,
        bars.byte_order(),
        bars.version(),
        bars.reset_table(),
        in_place,
    )?;
    println!(
        "patched `{asset_name}` in {} -> {} ({})",
        input.display(),
        out.display(),
        fmt_bytes(total),
    );
    Ok(())
}

fn write_bars(
    out: &Path,
    entries: &[PackEntry<'_>],
    byte_order: ByteOrder,
    version: u16,
    reset_table: &[u8],
    overwrite: bool,
) -> Result<u64> {
    let file = if overwrite {
        crate::paths::create_overwrite(out)?
    } else {
        crate::paths::create(out)?
    };
    let mut writer = BufWriter::new(file);
    bars::write(&mut writer, entries, byte_order, version, reset_table)
        .with_context(|| format!("write `{}`", out.display()))
}

fn strip_d(dir: &Path, ext: &str) -> PathBuf {
    if dir.extension().is_some_and(|e| e == "d") {
        dir.with_extension("")
    } else {
        append_ext(dir, ext)
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect()
}

struct EntryDoc {
    name: String,
    meta: String,
    asset: Option<String>,
}

struct Manifest {
    version: u16,
    byte_order: ByteOrder,
    reset_table: Vec<u8>,
    entries: Vec<EntryDoc>,
}

fn parse_manifest(text: &str) -> Result<Manifest> {
    let docs = Yaml::load_from_str(text).context("parse bars.yml")?;
    let doc = docs.first().context("empty bars.yml")?;

    let version = get(doc, "version")
        .and_then(Yaml::as_integer)
        .and_then(|n| u16::try_from(n).ok())
        .context("missing or invalid `version`")?;
    let byte_order = match get(doc, "byte_order").and_then(Yaml::as_str) {
        Some("little") => ByteOrder::Little,
        Some("big") => ByteOrder::Big,
        _ => bail!("missing or invalid `byte_order`"),
    };
    let reset_table = match get(doc, "reset_table").and_then(Yaml::as_str) {
        Some(s) => hex::decode(s).context("invalid reset_table hex")?,
        None => Vec::new(),
    };

    let seq = get(doc, "entries")
        .and_then(Yaml::as_sequence)
        .context("missing `entries` sequence")?;
    let mut entries = Vec::with_capacity(seq.len());
    for (i, e) in seq.iter().enumerate() {
        let name = get(e, "name")
            .and_then(Yaml::as_str)
            .map(ToString::to_string)
            .with_context(|| format!("entry {i}: missing `name`"))?;
        let meta = get(e, "meta")
            .and_then(Yaml::as_str)
            .map(ToString::to_string)
            .with_context(|| format!("entry {i}: missing `meta`"))?;
        let asset = get(e, "asset")
            .and_then(Yaml::as_str)
            .map(ToString::to_string);
        entries.push(EntryDoc { name, meta, asset });
    }

    Ok(Manifest {
        version,
        byte_order,
        reset_table,
        entries,
    })
}
