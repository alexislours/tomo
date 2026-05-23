use std::io::Write;

use anyhow::Result;
use owo_colors::OwoColorize;
use tabled::Table;
use tabled::builder::Builder;
use tabled::settings::object::Columns;
use tabled::settings::{Alignment, Modify, Padding, Style};

const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];

#[derive(Debug, clap::Args)]
pub(crate) struct InfoArgs {
    /// Emit machine-readable JSON instead of a formatted table.
    #[arg(long)]
    pub(crate) json: bool,
}

pub(crate) fn print_json(value: &serde_json::Value) -> Result<()> {
    let mut buf = serde_json::to_vec_pretty(value)?;
    buf.push(b'\n');
    match std::io::stdout().lock().write_all(&buf) {
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

pub(crate) fn fmt_bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} bytes");
    }
    #[allow(clippy::cast_precision_loss)]
    let mut v = n as f64;
    let mut idx = 0;
    while v >= 1024.0 && idx < UNITS.len() - 1 {
        v /= 1024.0;
        idx += 1;
    }
    format!("{v:.2} {}", UNITS[idx])
}

pub(crate) fn with_commas(n: u64) -> String {
    let raw = n.to_string();
    let len = raw.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in raw.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub(crate) fn label(s: &str) -> String {
    s.dimmed().to_string()
}

pub(crate) fn value(s: impl Into<String>) -> String {
    s.into().cyan().bold().to_string()
}

pub(crate) fn extra_bytes(n: u64) -> String {
    format!("{} bytes", with_commas(n)).dimmed().to_string()
}

pub(crate) fn finish_info_table(builder: Builder) -> Table {
    let mut table = builder.build();
    table
        .with(Style::blank())
        .with(Modify::new(Columns::one(0)).with(Alignment::right()))
        .with(Modify::new(Columns::one(1)).with(Alignment::right()))
        .with(Padding::new(2, 2, 0, 0));
    table
}
