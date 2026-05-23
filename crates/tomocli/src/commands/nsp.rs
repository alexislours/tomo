use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::nsp::PartitionFs;

use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::sanitize_relative;

const COPY_BUF_SIZE: usize = 8 << 20;

#[derive(Debug, Args)]
pub(crate) struct NspArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of an NSP (PFS0) package.
    Info {
        /// Path to the NSP file.
        input: PathBuf,
        /// List every entry instead of just a summary.
        #[arg(short, long)]
        list: bool,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Extract the files inside an NSP into a directory.
    Extract {
        /// Path to the NSP file.
        input: PathBuf,
        /// Destination directory. Defaults to <input> with the extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(args: NspArgs) -> Result<()> {
    match args.verb {
        Verb::Info {
            input,
            list,
            common,
        } => info(&input, list, common.json),
        Verb::Extract { input, out } => extract(&input, out),
    }
}

fn open(path: &Path) -> Result<(File, PartitionFs)> {
    let mut file = File::open(path).with_context(|| format!("open `{}`", path.display()))?;
    let fs = PartitionFs::read_header(&mut file)
        .with_context(|| format!("parse `{}`", path.display()))?;
    Ok((file, fs))
}

fn info(input: &Path, list: bool, json: bool) -> Result<()> {
    let (_, fs) = open(input)?;
    let entries = fs.entries();

    if json {
        let mut obj = serde_json::json!({
            "file": input.display().to_string(),
            "type": fs.kind().name(),
            "files": entries.len(),
            "header_size": fs.header_size(),
            "payload_size": fs.payload_size(),
        });
        if list {
            obj["entries"] = entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "size": e.size,
                        "offset": e.offset,
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
    row("Type", fs.kind().name().to_string(), String::new());
    row("Files", entries.len().to_string(), String::new());
    row(
        "Header size",
        format!("{:#x}", fs.header_size()),
        String::new(),
    );
    let payload = fs.payload_size();
    row("Payload bytes", fmt_bytes(payload), extra_bytes(payload));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    if list {
        println!();
        let mut table = Builder::default();
        table.push_record(["#", "size", "offset", "name"]);
        for (i, e) in entries.iter().enumerate() {
            table.push_record([
                i.to_string(),
                fmt_bytes(e.size),
                format!("{:#x}", e.offset),
                e.name.clone(),
            ]);
        }
        let mut table = table.build();
        table.with(Style::blank()).with(Padding::new(2, 2, 0, 0));
        println!("{table}");
    } else if !entries.is_empty() {
        println!();
        println!("  {}", "first entries:".dimmed());
        for e in entries.iter().take(5) {
            println!("    {}  {}", e.name, fmt_bytes(e.size).dimmed());
        }
        if entries.len() > 5 {
            println!(
                "    {}",
                format!("… and {} more", entries.len() - 5).dimmed()
            );
        }
    }

    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let (mut file, fs) = open(input)?;
    let out_dir = out.unwrap_or_else(|| input.with_extension(""));
    fs::create_dir_all(&out_dir).with_context(|| format!("create `{}`", out_dir.display()))?;

    let payload = fs.payload_size();
    let pb = ProgressBar::new(payload);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
        )
        .expect("valid template")
        .progress_chars("=> "),
    );

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    for entry in fs.entries() {
        let safe = sanitize_relative(&entry.name)
            .with_context(|| format!("unsafe entry path `{}`", entry.name))?;
        let dest = out_dir.join(&safe);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create `{}`", parent.display()))?;
        }
        pb.set_message(entry.name.clone());
        let mut writer =
            File::create(&dest).with_context(|| format!("create `{}`", dest.display()))?;

        file.seek(SeekFrom::Start(entry.offset))?;
        let mut remaining = entry.size;
        while remaining > 0 {
            let want = buf
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            file.read_exact(&mut buf[..want])
                .with_context(|| format!("read `{}`", input.display()))?;
            writer
                .write_all(&buf[..want])
                .with_context(|| format!("write `{}`", entry.name))?;
            remaining -= want as u64;
            pb.inc(want as u64);
        }
    }
    pb.finish_and_clear();

    println!(
        "extracted {} -> {} ({} files, {})",
        input.display(),
        out_dir.display(),
        fs.entries().len(),
        fmt_bytes(payload),
    );
    Ok(())
}
