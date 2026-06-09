use std::fs::{self, File};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::nca::{KeySet, Nca};

use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::sanitize_relative;

#[derive(Debug, Args)]
pub(crate) struct NcaArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Args)]
pub(crate) struct KeyOpts {
    /// Path to `prod.keys`. Defaults to `$HOME/.switch/prod.keys`.
    #[arg(short, long, global = true, env = "TOMO_KEYS")]
    keys: Option<PathBuf>,
    /// Path to `title.keys` (for NCAs that carry a rights id).
    #[arg(short, long, global = true)]
    title_keys: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of an NCA (header and partitions).
    Info {
        /// Path to the NCA file.
        input: PathBuf,
        /// List every file in each partition.
        #[arg(short, long)]
        list: bool,
        #[command(flatten)]
        keys: KeyOpts,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Extract the partitions of an NCA into a directory.
    Extract {
        /// Path to the NCA file.
        input: PathBuf,
        /// Destination directory. Defaults to <input> with the extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Verify each partition against its hash tree before extracting.
        #[arg(long)]
        verify: bool,
        #[command(flatten)]
        keys: KeyOpts,
    },
    /// Verify the integrity of each partition against its hash tree.
    Verify {
        /// Path to the NCA file.
        input: PathBuf,
        #[command(flatten)]
        keys: KeyOpts,
    },
}

pub(crate) fn run(args: NcaArgs) -> Result<()> {
    match args.verb {
        Verb::Info {
            input,
            list,
            keys,
            common,
        } => info(&input, list, &keys, common.json),
        Verb::Extract {
            input,
            out,
            verify,
            keys,
        } => extract(&input, out, verify, &keys),
        Verb::Verify { input, keys } => verify(&input, &keys),
    }
}

pub(crate) fn load_keys(opts: &KeyOpts) -> Result<KeySet> {
    let prod_path = opts.keys.clone().or_else(default_prod_keys).context(
        "could not locate prod.keys; pass --keys or set $TOMO_KEYS / ~/.switch/prod.keys",
    )?;
    let prod = fs::read_to_string(&prod_path)
        .with_context(|| format!("read keys `{}`", prod_path.display()))?;

    let title_path = opts
        .title_keys
        .clone()
        .or_else(|| default_keys_dir().map(|d| d.join("title.keys")))
        .filter(|p| p.is_file());
    let title = match &title_path {
        Some(p) => Some(
            fs::read_to_string(p).with_context(|| format!("read title keys `{}`", p.display()))?,
        ),
        None => None,
    };

    KeySet::parse(&prod, title.as_deref()).context("parse keys")
}

fn default_keys_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".switch"))
}

fn default_prod_keys() -> Option<PathBuf> {
    default_keys_dir()
        .map(|d| d.join("prod.keys"))
        .filter(|p| p.is_file())
}

fn open(path: &Path, keys: &KeySet) -> Result<(BufReader<File>, Nca)> {
    let file = File::open(path).with_context(|| format!("open `{}`", path.display()))?;
    let mut reader = BufReader::new(file);
    let nca =
        Nca::open(&mut reader, keys).with_context(|| format!("parse `{}`", path.display()))?;
    Ok((reader, nca))
}

fn info_json(input: &Path, nca: &Nca, reader: &mut BufReader<File>, list: bool) -> Result<()> {
    let h = &nca.header;
    let mut partitions = Vec::with_capacity(nca.partitions.len());
    for p in &nca.partitions {
        let mut obj = serde_json::json!({
            "index": p.index,
            "format": p.format.name(),
            "hash_type": p.hash_type.name(),
            "enc_type": p.enc_type.name(),
            "offset": p.offset,
            "size": p.size,
        });
        if list {
            match nca.list_partition(reader, p) {
                Ok(entries) => {
                    obj["files"] = entries
                        .iter()
                        .map(|e| serde_json::json!({ "path": e.path, "size": e.size }))
                        .collect();
                }
                Err(err) => obj["error"] = serde_json::json!(err.to_string()),
            }
        }
        partitions.push(obj);
    }
    let mut obj = serde_json::json!({
        "file": input.display().to_string(),
        "format": h.format.to_string(),
        "distribution": h.distribution.name(),
        "content_type": h.content_type.name(),
        "program_id": format!("{:016x}", h.program_id),
        "content_index": h.content_index,
        "sdk_version": h.sdk_addon_version_string(),
        "key_generation": h.key_generation,
        "kaek_index": h.kaek_index,
        "content_size": h.content_size,
        "content_key": if nca.has_content_key() { "resolved" } else { "missing" },
        "partitions": partitions,
    });
    if h.has_rights_id() {
        obj["rights_id"] = serde_json::json!(hex(&h.rights_id));
    }
    crate::fmt::print_json(&obj)
}

fn info(input: &Path, list: bool, keys: &KeyOpts, json: bool) -> Result<()> {
    let keys = load_keys(keys)?;
    let (mut reader, nca) = open(input, &keys)?;
    let h = &nca.header;

    if json {
        return info_json(input, &nca, &mut reader, list);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String, extra: String| t.push_record([label(k), value(v), extra]);
    row("Format", h.format.to_string(), String::new());
    row(
        "Distribution",
        h.distribution.name().to_string(),
        String::new(),
    );
    row(
        "Content type",
        h.content_type.name().to_string(),
        String::new(),
    );
    row(
        "Program ID",
        format!("{:016x}", h.program_id),
        String::new(),
    );
    row("Content index", h.content_index.to_string(), String::new());
    row("SDK version", h.sdk_addon_version_string(), String::new());
    row(
        "Key generation",
        h.key_generation.to_string(),
        String::new(),
    );
    row("KAEK index", h.kaek_index.to_string(), String::new());
    row(
        "Content size",
        fmt_bytes(h.content_size),
        extra_bytes(h.content_size),
    );
    if h.has_rights_id() {
        row("Rights ID", hex(&h.rights_id), String::new());
    }
    row(
        "Content key",
        if nca.has_content_key() {
            "resolved".to_string()
        } else {
            "missing".to_string()
        },
        String::new(),
    );

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    println!();
    let mut pt = Builder::default();
    pt.push_record(["#", "format", "hash", "enc", "offset", "size"]);
    for p in &nca.partitions {
        pt.push_record([
            p.index.to_string(),
            p.format.name().to_string(),
            p.hash_type.name().to_string(),
            p.enc_type.name().to_string(),
            format!("{:#x}", p.offset),
            fmt_bytes(p.size),
        ]);
    }
    let mut pt = pt.build();
    pt.with(Style::blank()).with(Padding::new(2, 2, 0, 0));
    println!("{pt}");

    if list {
        for p in &nca.partitions {
            match nca.list_partition(&mut reader, p) {
                Ok(entries) => {
                    println!();
                    println!(
                        "  {} {}",
                        format!("partition {}", p.index).bold(),
                        format!("({} files)", entries.len()).dimmed()
                    );
                    for e in &entries {
                        println!("    {}  {}", e.path, fmt_bytes(e.size).dimmed());
                    }
                }
                Err(err) => {
                    println!();
                    println!("  partition {} not readable: {}", p.index, err);
                }
            }
        }
    }

    Ok(())
}

fn verify(input: &Path, keys: &KeyOpts) -> Result<()> {
    let keys = load_keys(keys)?;
    let (mut reader, nca) = open(input, &keys)?;

    if nca.partitions.is_empty() {
        bail!("NCA has no enabled partitions");
    }

    println!();
    println!("  {}", input.display().bold());
    println!();

    let mut failures = 0usize;
    for p in &nca.partitions {
        match nca.verify_partition(&mut reader, p) {
            Ok(()) => println!("  partition {} {}", p.index, "ok".green()),
            Err(err) => {
                failures += 1;
                println!("  partition {} {}: {}", p.index, "FAILED".red(), err);
            }
        }
    }
    println!();

    if failures > 0 {
        bail!("{failures} partition(s) failed verification");
    }
    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>, verify_first: bool, keys: &KeyOpts) -> Result<()> {
    let keys = load_keys(keys)?;
    let (mut reader, nca) = open(input, &keys)?;

    let out_dir = out.unwrap_or_else(|| input.with_extension(""));
    fs::create_dir_all(&out_dir).with_context(|| format!("create `{}`", out_dir.display()))?;

    if nca.partitions.is_empty() {
        bail!("NCA has no enabled partitions");
    }

    if verify_first {
        for p in &nca.partitions {
            nca.verify_partition(&mut reader, p)
                .with_context(|| format!("verify partition {}", p.index))?;
        }
    }

    let mut planned = Vec::new();
    for p in &nca.partitions {
        match nca.list_partition(&mut reader, p) {
            Ok(entries) => planned.push((p, entries)),
            Err(err) => println!("  skipping partition {}: {}", p.index, err),
        }
    }

    let total: u64 = planned
        .iter()
        .flat_map(|(_, entries)| entries.iter())
        .map(|e| e.size)
        .sum();
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
        )
        .expect("valid template")
        .progress_chars("=> "),
    );

    let mut file_count = 0usize;
    for (p, entries) in &planned {
        let part_dir = out_dir.join(p.index.to_string());
        for entry in entries {
            let safe = sanitize_relative(&entry.path)
                .with_context(|| format!("unsafe entry path `{}`", entry.path))?;
            let dest = part_dir.join(&safe);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create `{}`", parent.display()))?;
            }
            pb.set_message(entry.path.clone());
            let mut writer = crate::paths::create(&dest)?;
            nca.copy_file(&mut reader, p, entry, &mut writer)
                .with_context(|| format!("extract `{}`", entry.path))?;
            file_count += 1;
            pb.inc(entry.size);
        }
    }
    pb.finish_and_clear();

    crate::fmt::report(
        "extracted",
        input,
        &out_dir,
        &format!("{file_count} files, {}", fmt_bytes(total)),
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
