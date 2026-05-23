use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use tabled::builder::Builder;

use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, strip_ext};

#[derive(Debug, Args)]
pub(crate) struct ZsArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of a .zs (zstd) file.
    Info {
        /// Path to the .zs file to inspect.
        input: PathBuf,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Decompress a .zs file.
    Extract {
        /// Path to the .zs file to decompress.
        input: PathBuf,
        /// Destination path. Defaults to <input> with the `.zs` suffix stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Compress a file into a .zs (zstd) frame.
    Pack {
        /// Path to the file to compress.
        input: PathBuf,
        /// Destination path. Defaults to <input> with `.zs` appended.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Compression level (1..=22). Default to level 9.
        #[arg(short, long, default_value_t = tomolib::formats::zs::DEFAULT_LEVEL)]
        level: i32,
    },
}

pub(crate) fn run(args: ZsArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input, common } => info(&input, common.json),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack { input, out, level } => pack(&input, out, level),
    }
}

fn info(input: &Path, json: bool) -> Result<()> {
    let meta = fs::metadata(input).with_context(|| format!("stat `{}`", input.display()))?;
    let file = File::open(input).with_context(|| format!("open `{}`", input.display()))?;
    let info = tomolib::formats::zs::info(BufReader::new(file), meta.len())
        .with_context(|| format!("inspect `{}`", input.display()))?;

    if json {
        let obj = serde_json::json!({
            "file": input.display().to_string(),
            "compressed_size": info.compressed_size,
            "decompressed_size": info.decompressed_size,
        });
        return crate::fmt::print_json(&obj);
    }

    let size = |n: u64| value(fmt_bytes(n));

    let mut t = Builder::default();
    t.push_record([
        label("Compressed"),
        size(info.compressed_size),
        extra_bytes(info.compressed_size),
    ]);
    match info.decompressed_size {
        Some(n) => {
            t.push_record([label("Decompressed"), size(n), extra_bytes(n)]);
            if info.compressed_size > 0 && n > 0 {
                let nd = u128::from(n);
                let cd = u128::from(info.compressed_size);
                let ratio_milli = (nd * 1000 + cd / 2) / cd;
                let saved_centi =
                    (i128::from(n) - i128::from(info.compressed_size)) * 10_000 / i128::from(n);
                let sign = if saved_centi < 0 { "-" } else { "" };
                let saved_abs = saved_centi.unsigned_abs();
                t.push_record([
                    label("Ratio"),
                    value(format!(
                        "{}.{:03} ×",
                        ratio_milli / 1000,
                        ratio_milli % 1000
                    )),
                    format!("saved {sign}{}.{:02}%", saved_abs / 100, saved_abs % 100)
                        .green()
                        .to_string(),
                ]);
            }
        }
        None => t.push_record([
            label("Decompressed"),
            "unknown".to_string(),
            "not stored in frame header".dimmed().to_string(),
        ]),
    }

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));
    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let out = match out {
        Some(p) => p,
        None => strip_ext(input, &["zs", "szs"])?,
    };
    let reader =
        BufReader::new(File::open(input).with_context(|| format!("open `{}`", input.display()))?);
    let writer = BufWriter::new(crate::paths::create(&out)?);
    let n = tomolib::formats::zs::decompress(reader, writer)
        .with_context(|| format!("decompress `{}`", input.display()))?;
    crate::fmt::report("extracted", input, &out, &fmt_bytes(n));
    Ok(())
}

fn pack(input: &Path, out: Option<PathBuf>, level: i32) -> Result<()> {
    let out = out.unwrap_or_else(|| append_ext(input, "zs"));
    let meta = fs::metadata(input).with_context(|| format!("stat `{}`", input.display()))?;
    let reader =
        BufReader::new(File::open(input).with_context(|| format!("open `{}`", input.display()))?);
    let writer = BufWriter::new(crate::paths::create(&out)?);
    let n = tomolib::formats::zs::compress(reader, writer, level, Some(meta.len()))
        .with_context(|| format!("compress `{}`", input.display()))?;
    crate::fmt::report(
        "packed",
        input,
        &out,
        &format!("{} in, level {level}", fmt_bytes(n)),
    );
    Ok(())
}
