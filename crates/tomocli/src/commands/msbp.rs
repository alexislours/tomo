use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use tomolib::formats::msbp::{MSBP_MAGIC, Msbp};

use crate::commands::{lms, rstbl};
use crate::fmt::{finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, strip_ext, write_file};
use tabled::builder::Builder;

#[derive(Debug, Args)]
pub(crate) struct MsbpArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Summarize an MSBP file.
    Info {
        input: PathBuf,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Decompose an MSBP into a YAML document.
    Extract {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Build an MSBP from a YAML document.
    Pack {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Update an existing RESTBL table with the packed file's size.
        #[arg(long, value_name = "PATH")]
        update_rstbl: Option<PathBuf>,
        /// Resource name used when updating the RESTBL. Defaults to the
        /// output file's basename.
        #[arg(long, value_name = "NAME", requires = "update_rstbl")]
        resource_name: Option<String>,
    },
}

pub(crate) fn run(args: MsbpArgs) -> Result<()> {
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

fn read_msbp(input: &Path) -> Result<Msbp> {
    let bytes = fs::read(input).with_context(|| format!("read `{}`", input.display()))?;
    if !bytes.starts_with(&MSBP_MAGIC) {
        anyhow::bail!("`{}` is not an MSBP file", input.display());
    }
    Ok(Msbp::parse(&bytes)?)
}

fn info(input: &Path, json: bool) -> Result<()> {
    let m = read_msbp(input)?;

    if json {
        let obj = serde_json::json!({
            "file": input.display().to_string(),
            "format": "MSBP",
            "version": m.header.version,
            "colors": m.colors.len(),
            "attributes": m.attributes.len(),
            "tag_groups": m.tag_groups.len(),
            "sources": m.sources.len(),
        });
        return crate::fmt::print_json(&obj);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String| t.push_record([label(k), value(v), String::new()]);
    row("Format", "MSBP".into());
    row("Version", m.header.version.to_string());
    row("Colors", m.colors.len().to_string());
    row("Attributes", m.attributes.len().to_string());
    row("Tag groups", m.tag_groups.len().to_string());
    row("Sources", m.sources.len().to_string());
    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));
    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let m = read_msbp(input)?;
    let out = out.unwrap_or_else(|| append_ext(input, "yml"));
    let mut w = BufWriter::new(crate::paths::create(&out)?);
    lms::emit_msbp(&m, &mut w)?;
    crate::fmt::report("extracted", input, &out, "");
    Ok(())
}

fn pack(
    input: &Path,
    out: Option<PathBuf>,
    update_rstbl: Option<PathBuf>,
    resource_name: Option<String>,
) -> Result<()> {
    let text = fs::read_to_string(input).with_context(|| format!("read `{}`", input.display()))?;
    let out = match out {
        Some(p) => p,
        None => strip_ext(input, &["yml", "yaml"])?,
    };
    let bytes = lms::parse_msbp(&text)?.to_bytes()?;
    write_file(&out, &bytes)?;
    crate::fmt::report("packed", input, &out, &fmt_bytes(bytes.len() as u64));
    if let Some(rstbl_path) = update_rstbl {
        rstbl::maybe_update_rstbl(&rstbl_path, resource_name, &out, bytes.len())?;
    }
    Ok(())
}
