use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::sarc::{self, ByteOrder, PackEntry, Sarc};

use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, read_file, sanitize_relative, unnamed_entry, write_file};

#[derive(Debug, Args)]
pub(crate) struct SarcArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of a SARC archive.
    Info {
        /// Path to the SARC file.
        input: PathBuf,
        /// List every entry instead of just a summary.
        #[arg(short, long)]
        list: bool,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Extract a SARC into a directory.
    Extract {
        /// Path to the SARC file.
        input: PathBuf,
        /// Destination directory. Defaults to <input> with the extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Build a SARC archive from a directory tree.
    Pack {
        /// Directory whose contents become the archive's files.
        input: PathBuf,
        /// Destination SARC file. Defaults to <input>.sarc.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Byte order to write. Tomodachi Life is little-endian.
        #[arg(long, value_enum, default_value_t = Endian::Little)]
        endian: Endian,
        /// Minimum alignment for entry data. Must be a power of two.
        #[arg(short, long, default_value_t = 4)]
        align: u32,
    },
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Endian {
    Little,
    Big,
}

impl From<Endian> for ByteOrder {
    fn from(e: Endian) -> Self {
        match e {
            Endian::Little => ByteOrder::Little,
            Endian::Big => ByteOrder::Big,
        }
    }
}

pub(crate) fn run(args: SarcArgs) -> Result<()> {
    match args.verb {
        Verb::Info {
            input,
            list,
            common,
        } => info(&input, list, common.json),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack {
            input,
            out,
            endian,
            align,
        } => pack(&input, out, endian.into(), align),
    }
}

fn load_sarc(path: &Path) -> Result<Sarc> {
    let bytes = read_file(path)?;
    Sarc::parse(bytes).with_context(|| format!("parse `{}`", path.display()))
}

fn info(input: &Path, list: bool, json: bool) -> Result<()> {
    let sarc = load_sarc(input)?;
    let entries = sarc.entries();

    let byte_order = match sarc.byte_order() {
        ByteOrder::Little => "little",
        ByteOrder::Big => "big",
    };
    let data_bytes: u64 = entries.iter().map(|e| u64::from(e.len())).sum();

    if json {
        let mut obj = serde_json::json!({
            "file": input.display().to_string(),
            "byte_order": byte_order,
            "version": sarc.version(),
            "files": entries.len(),
            "hash_multiplier": sarc.hash_multiplier(),
            "data_offset": sarc.data_offset(),
            "total_size": sarc.total_size(),
            "payload_bytes": data_bytes,
        });
        if list {
            obj["entries"] = entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "size": e.len(),
                        "offset": e.data_start,
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
    row("Version", format!("{:#06x}", sarc.version()), String::new());
    row("Files", entries.len().to_string(), String::new());
    row(
        "Hash multiplier",
        format!("{:#x}", sarc.hash_multiplier()),
        String::new(),
    );
    row(
        "Data offset",
        format!("{:#x}", sarc.data_offset()),
        String::new(),
    );
    let total = u64::from(sarc.total_size());
    row("Total size", fmt_bytes(total), extra_bytes(total));
    row(
        "Payload bytes",
        fmt_bytes(data_bytes),
        extra_bytes(data_bytes),
    );

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    if list {
        println!();
        let mut names = Builder::default();
        names.push_record(["#", "size", "offset", "name"]);
        for (i, e) in entries.iter().enumerate() {
            names.push_record([
                i.to_string(),
                fmt_bytes(u64::from(e.len())),
                format!("{:#x}", e.data_start),
                e.name.clone().unwrap_or_else(|| "<unnamed>".to_string()),
            ]);
        }
        let mut t = names.build();
        t.with(Style::blank()).with(Padding::new(2, 2, 0, 0));
        println!("{t}");
    } else if !entries.is_empty() {
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| e.name.as_deref())
            .take(5)
            .collect();
        if !names.is_empty() {
            println!();
            println!("  {}", "first entries:".dimmed());
            for n in &names {
                println!("    {n}");
            }
            if entries.len() > names.len() {
                println!(
                    "    {}",
                    format!("… and {} more", entries.len() - names.len()).dimmed()
                );
            }
        }
    }

    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let sarc = load_sarc(input)?;
    let out_dir = out.unwrap_or_else(|| default_extract_dir(input));
    fs::create_dir_all(&out_dir).with_context(|| format!("create `{}`", out_dir.display()))?;

    let mut written = 0u64;
    for (i, entry) in sarc.entries().iter().enumerate() {
        let rel = entry.name.clone().unwrap_or_else(|| unnamed_entry(i));
        let safe = sanitize_relative(&rel).with_context(|| format!("unsafe entry path `{rel}`"))?;
        let dest = out_dir.join(&safe);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create `{}`", parent.display()))?;
        }
        write_file(&dest, sarc.data(entry))?;
        written += u64::from(entry.len());
    }
    println!(
        "extracted {} -> {} ({} files, {})",
        input.display(),
        out_dir.display(),
        sarc.entries().len(),
        fmt_bytes(written),
    );
    Ok(())
}

fn pack(input: &Path, out: Option<PathBuf>, byte_order: ByteOrder, align: u32) -> Result<()> {
    if !input.is_dir() {
        bail!(
            "input `{}` must be a directory of files to archive",
            input.display()
        );
    }
    let out = out.unwrap_or_else(|| append_ext(input, "sarc"));

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    collect_files(input, input, &mut files)?;
    if files.is_empty() {
        bail!("no files found under `{}`", input.display());
    }

    let entries: Vec<PackEntry<'_>> = files
        .iter()
        .map(|(name, data)| PackEntry {
            name: name.as_str(),
            data: data.as_slice(),
        })
        .collect();

    let mut writer =
        BufWriter::new(File::create(&out).with_context(|| format!("create `{}`", out.display()))?);
    let total = sarc::write(&mut writer, &entries, byte_order, align)
        .with_context(|| format!("write `{}`", out.display()))?;
    println!(
        "packed {} files from {} -> {} ({})",
        entries.len(),
        input.display(),
        out.display(),
        fmt_bytes(total),
    );
    Ok(())
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir `{}`", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else if ft.is_file() {
            let rel_path = path.strip_prefix(root).expect("walked path is under root");
            let rel = rel_path.to_str().ok_or_else(|| {
                anyhow::anyhow!("entry path is not valid UTF-8: `{}`", rel_path.display())
            })?;
            let rel = rel.replace('\\', "/");
            let bytes = read_file(&path)?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

fn default_extract_dir(input: &Path) -> PathBuf {
    input.with_extension("")
}
