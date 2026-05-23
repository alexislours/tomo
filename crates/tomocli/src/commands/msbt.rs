use std::fs::{self, File};
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use tomolib::formats::msbt::{MSBT_MAGIC, Msbt};

use crate::commands::{lms, rstbl};
use crate::fmt::{finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, strip_ext, write_file};
use tabled::builder::Builder;

#[derive(Debug, Args)]
pub(crate) struct MsbtArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Summarize an MSBT file.
    Info {
        input: PathBuf,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Decompose an MSBT into a YAML document.
    Extract {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Path to the MSBP project used to name tags and attributes. If
        /// omitted, a sibling `project.msbp` is auto-discovered.
        #[arg(long, value_name = "PATH")]
        msbp: Option<PathBuf>,
    },
    /// Build an MSBT from a YAML document.
    Pack {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// MSBP project for resolving tag/attribute names back to indices.
        #[arg(long, value_name = "PATH")]
        msbp: Option<PathBuf>,
        /// Update an existing RESTBL table with the packed file's size.
        #[arg(long, value_name = "PATH")]
        update_rstbl: Option<PathBuf>,
        /// Resource name used when updating the RESTBL. Defaults to the
        /// output file's basename.
        #[arg(long, value_name = "NAME", requires = "update_rstbl")]
        resource_name: Option<String>,
    },
}

pub(crate) fn run(args: MsbtArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input, common } => info(&input, common.json),
        Verb::Extract { input, out, msbp } => extract(&input, out, msbp),
        Verb::Pack {
            input,
            out,
            msbp,
            update_rstbl,
            resource_name,
        } => pack(&input, out, msbp, update_rstbl, resource_name),
    }
}

fn read_msbt(input: &Path) -> Result<Msbt> {
    let bytes = fs::read(input).with_context(|| format!("read `{}`", input.display()))?;
    if !bytes.starts_with(&MSBT_MAGIC) {
        anyhow::bail!("`{}` is not an MSBT file", input.display());
    }
    Ok(Msbt::parse(&bytes)?)
}

fn info(input: &Path, json: bool) -> Result<()> {
    let m = read_msbt(input)?;

    if json {
        let obj = serde_json::json!({
            "file": input.display().to_string(),
            "format": "MSBT",
            "version": m.header.version,
            "messages": m.messages.len(),
            "attr_size": m.attr_size,
            "sections": m.order.len(),
        });
        return crate::fmt::print_json(&obj);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String| t.push_record([label(k), value(v), String::new()]);
    row("Format", "MSBT".into());
    row("Version", m.header.version.to_string());
    row("Messages", m.messages.len().to_string());
    row("Attr size", m.attr_size.to_string());
    row("Sections", m.order.len().to_string());
    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));
    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>, msbp: Option<PathBuf>) -> Result<()> {
    let m = read_msbt(input)?;
    let out = out.unwrap_or_else(|| append_ext(input, "yml"));
    let mut w =
        BufWriter::new(File::create(&out).with_context(|| format!("create `{}`", out.display()))?);
    let reg = lms::load_registry(input, msbp)?;
    lms::emit_msbt(&m, reg.as_ref(), &mut w)?;
    println!("extracted {} -> {}", input.display(), out.display());
    Ok(())
}

fn pack(
    input: &Path,
    out: Option<PathBuf>,
    msbp: Option<PathBuf>,
    update_rstbl: Option<PathBuf>,
    resource_name: Option<String>,
) -> Result<()> {
    let text = fs::read_to_string(input).with_context(|| format!("read `{}`", input.display()))?;
    let out = match out {
        Some(p) => p,
        None => strip_ext(input, &["yml", "yaml"])?,
    };
    let reg = lms::load_registry(&out, msbp)?;
    let bytes = lms::parse_msbt(&text, reg.as_ref())?.to_bytes()?;
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
