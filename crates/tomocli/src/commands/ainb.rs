use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use tabled::builder::Builder;
use tomolib::formats::ainb::Ainb;

use crate::fmt::{finish_info_table, label, value};
use crate::paths::{append_ext, read_file, write_file};

#[derive(Debug, Args)]
pub(crate) struct AinbArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of an AINB node graph.
    Info {
        /// Path to the AINB file.
        input: PathBuf,
    },
    /// Decompose an AINB into a YAML tree (EXB kept as a base64 blob).
    Extract {
        /// Path to the AINB file.
        input: PathBuf,
        /// Destination YAML file. Defaults to <input>.yml.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Rebuild an AINB from a YAML file produced by `extract`.
    Pack {
        /// Path to the YAML file.
        input: PathBuf,
        /// Destination AINB file. Defaults to <input> with the `.yaml` extension stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn convert_bytes_to_yaml(bytes: &[u8]) -> Result<Vec<u8>> {
    let ainb = Ainb::parse(bytes).context("parse AINB")?;
    Ok(ainb
        .to_yaml()
        .context("serialize AINB to YAML")?
        .into_bytes())
}

pub(crate) fn run(args: AinbArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input } => info(&input),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack { input, out } => pack(&input, out),
    }
}

fn load(path: &Path) -> Result<Ainb> {
    let bytes = read_file(path)?;
    Ainb::parse(&bytes).with_context(|| format!("parse `{}`", path.display()))
}

fn info(input: &Path) -> Result<()> {
    let ainb = load(input)?;

    let mut t = Builder::default();
    let mut row = |k: &str, v: String| t.push_record([label(k), value(v)]);
    row("Version", format!("{:#06x}", ainb.version));
    row("Filename", ainb.filename.clone());
    row("Category", ainb.category.clone());
    row("Commands", ainb.commands.len().to_string());
    row("Nodes", ainb.nodes.len().to_string());
    let queries = ainb.nodes.iter().filter(|n| n.is_query()).count();
    row("Query nodes", queries.to_string());
    let bb_params: usize = ainb
        .blackboard
        .as_ref()
        .map_or(0, |bb| bb.params.iter().map(Vec::len).sum());
    row("Blackboard params", bb_params.to_string());
    row("Modules", ainb.modules.len().to_string());
    let attachments: usize = ainb.nodes.iter().map(|n| n.attachments.len()).sum();
    row("Node attachments", attachments.to_string());
    if ainb.version >= 0x407 {
        row("Replacements", ainb.replacement_table.len().to_string());
    }
    if let Some(exb) = &ainb.expressions {
        row(
            "Expressions (EXB)",
            format!(
                "v{}, {} expr, {} instr, {} bytes",
                exb.version(),
                exb.expression_count(),
                exb.instruction_count(),
                exb.len()
            ),
        );
    } else {
        row("Expressions (EXB)", "none".to_string());
    }
    row("Section 0x6C", ainb.exists_section_0x6c.to_string());

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));
    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let ainb = load(input)?;
    let yaml = ainb
        .to_yaml()
        .with_context(|| format!("serialize `{}` to YAML", input.display()))?;
    let dest = out.unwrap_or_else(|| append_ext(input, "yml"));
    write_file(&dest, yaml.as_bytes())?;
    println!("Wrote {}", dest.display());
    Ok(())
}

fn pack(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let text = read_file(input)?;
    let text = std::str::from_utf8(&text).context("YAML input is not valid UTF-8")?;
    let ainb =
        Ainb::from_yaml(text).with_context(|| format!("parse YAML `{}`", input.display()))?;
    let dest = out.unwrap_or_else(|| match input.extension().and_then(|e| e.to_str()) {
        Some(e) if e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml") => {
            input.with_extension("")
        }
        _ => append_ext(input, "ainb"),
    });
    write_file(&dest, &ainb.to_binary())?;
    println!("Wrote {}", dest.display());
    Ok(())
}
