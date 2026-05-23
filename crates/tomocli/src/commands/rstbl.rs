use std::fs;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::rstbl::{CrcEntry, PathEntry, Rstbl};

use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, read_file, strip_ext, write_file};

#[derive(Debug, Args)]
pub(crate) struct RstblArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    Info {
        input: PathBuf,
        #[arg(short, long)]
        list: bool,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    Extract {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    Pack {
        input: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(args: RstblArgs) -> Result<()> {
    match args.verb {
        Verb::Info {
            input,
            list,
            common,
        } => info(&input, list, common.json),
        Verb::Extract { input, out } => extract(&input, out),
        Verb::Pack { input, out } => pack(&input, out),
    }
}

fn load(path: &Path) -> Result<Rstbl> {
    let bytes = read_file(path)?;
    Rstbl::parse(&bytes).with_context(|| format!("parse `{}`", path.display()))
}

fn info(input: &Path, list: bool, json: bool) -> Result<()> {
    let table = load(input)?;
    let meta = fs::metadata(input).with_context(|| format!("stat `{}`", input.display()))?;

    if json {
        let mut obj = serde_json::json!({
            "file": input.display().to_string(),
            "version": table.version(),
            "path_field_size": table.path_size(),
            "crc_entries": table.crc_entries().len(),
            "path_entries": table.path_entries().len(),
            "total_size": meta.len(),
        });
        if list {
            obj["crc_entry_list"] = table
                .crc_entries()
                .iter()
                .map(|e| serde_json::json!({ "hash": e.hash, "size": e.size }))
                .collect();
            obj["path_entry_list"] = table
                .path_entries()
                .iter()
                .map(|e| serde_json::json!({ "name": e.name, "size": e.size }))
                .collect();
        }
        return crate::fmt::print_json(&obj);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String, extra: String| {
        t.push_record([label(k), value(v), extra]);
    };
    row("Version", table.version().to_string(), String::new());
    row(
        "Path field size",
        format!("{:#x}", table.path_size()),
        format!("{} bytes", table.path_size()).dimmed().to_string(),
    );
    row(
        "CRC entries",
        table.crc_entries().len().to_string(),
        String::new(),
    );
    row(
        "Path entries",
        table.path_entries().len().to_string(),
        String::new(),
    );
    let total = meta.len();
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    if list {
        if !table.crc_entries().is_empty() {
            println!();
            println!("  {}", "crc32 entries:".dimmed());
            let mut b = Builder::default();
            b.push_record(["#", "hash", "size"]);
            for (i, e) in table.crc_entries().iter().enumerate() {
                b.push_record([
                    i.to_string(),
                    format!("{:#010x}", e.hash),
                    fmt_bytes(u64::from(e.size)),
                ]);
            }
            let mut tt = b.build();
            tt.with(Style::blank()).with(Padding::new(2, 2, 0, 0));
            println!("{tt}");
        }
        if !table.path_entries().is_empty() {
            println!();
            println!("  {}", "named (collision) entries:".dimmed());
            let mut b = Builder::default();
            b.push_record(["#", "size", "name"]);
            for (i, e) in table.path_entries().iter().enumerate() {
                b.push_record([i.to_string(), fmt_bytes(u64::from(e.size)), e.name.clone()]);
            }
            let mut tt = b.build();
            tt.with(Style::blank()).with(Padding::new(2, 2, 0, 0));
            println!("{tt}");
        }
    } else if !table.path_entries().is_empty() {
        println!();
        println!("  {}", "named entries:".dimmed());
        for e in table.path_entries().iter().take(5) {
            println!("    {}  ({})", e.name, fmt_bytes(u64::from(e.size)));
        }
        if table.path_entries().len() > 5 {
            println!(
                "    {}",
                format!("... and {} more", table.path_entries().len() - 5).dimmed()
            );
        }
    }

    Ok(())
}

fn extract(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let table = load(input)?;
    let out = out.unwrap_or_else(|| append_ext(input, "json"));
    let mut writer = BufWriter::new(crate::paths::create(&out)?);
    write_json(&table, &mut writer).with_context(|| format!("write `{}`", out.display()))?;
    println!(
        "extracted {} -> {} ({} crc, {} paths)",
        input.display(),
        out.display(),
        table.crc_entries().len(),
        table.path_entries().len(),
    );
    Ok(())
}

pub(crate) fn write_json<W: std::io::Write>(table: &Rstbl, writer: &mut W) -> Result<()> {
    serde_json::to_writer_pretty(writer, &RstblDoc(table)).map_err(Into::into)
}

pub(crate) fn convert_bytes_to_json(bytes: &[u8]) -> Result<Vec<u8>> {
    let table = Rstbl::parse(bytes)?;
    let mut out = Vec::new();
    write_json(&table, &mut out)?;
    Ok(out)
}

fn pack(input: &Path, out: Option<PathBuf>) -> Result<()> {
    let text = fs::read_to_string(input).with_context(|| format!("read `{}`", input.display()))?;
    let doc: Document =
        serde_json::from_str(&text).with_context(|| format!("parse `{}`", input.display()))?;
    let table = build_table(&doc)?;
    let out = match out {
        Some(p) => p,
        None => strip_ext(input, &["json"])?,
    };
    let mut writer = BufWriter::new(crate::paths::create(&out)?);
    let n = table
        .write(&mut writer)
        .with_context(|| format!("write `{}`", out.display()))?;
    println!(
        "packed {} -> {} ({})",
        input.display(),
        out.display(),
        fmt_bytes(n),
    );
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Document {
    version: u32,
    path_size: u32,
    #[serde(default)]
    crc: serde_json::Map<String, Value>,
    #[serde(default)]
    paths: serde_json::Map<String, Value>,
}

struct RstblDoc<'a>(&'a Rstbl);

impl Serialize for RstblDoc<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let t = self.0;
        let mut st = serializer.serialize_struct("Document", 4)?;
        st.serialize_field("version", &t.version())?;
        st.serialize_field("path_size", &t.path_size())?;
        st.serialize_field("crc", &CrcMap(t.crc_entries()))?;
        st.serialize_field("paths", &PathMap(t.path_entries()))?;
        st.end()
    }
}

struct CrcMap<'a>(&'a [CrcEntry]);

impl Serialize for CrcMap<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        let mut key = *b"0x00000000";
        for e in self.0 {
            for (i, slot) in key[2..].iter_mut().enumerate() {
                *slot = HEX[((e.hash >> ((7 - i) * 4)) & 0xf) as usize];
            }
            let key = std::str::from_utf8(&key).expect("hex digits are ASCII");
            map.serialize_entry(key, &e.size)?;
        }
        map.end()
    }
}

struct PathMap<'a>(&'a [PathEntry]);

impl Serialize for PathMap<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for e in self.0 {
            map.serialize_entry(&e.name, &e.size)?;
        }
        map.end()
    }
}

fn build_table(doc: &Document) -> Result<Rstbl> {
    let mut crc = Vec::with_capacity(doc.crc.len());
    for (k, v) in &doc.crc {
        let hash = parse_u32(k).with_context(|| format!("invalid crc key `{k}`"))?;
        let size = v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .with_context(|| format!("crc[{k}] is not a u32"))?;
        crc.push(CrcEntry { hash, size });
    }
    crc.sort_by_key(|e| e.hash);

    let path_size = doc.path_size;
    let path_size_usize = path_size as usize;
    let mut paths = Vec::with_capacity(doc.paths.len());
    for (name, v) in &doc.paths {
        let size = v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .with_context(|| format!("paths[{name}] is not a u32"))?;
        paths.push(PathEntry {
            name: name.clone(),
            size,
        });
    }
    paths.sort_by_key(|a| pad(&a.name, path_size_usize));

    let mut t = Rstbl::new(doc.version, path_size);
    t.set_crc_entries(crc);
    t.set_path_entries(paths);
    Ok(t)
}

fn pad(name: &str, width: usize) -> Vec<u8> {
    let mut v = vec![0u8; width];
    let bytes = name.as_bytes();
    let n = bytes.len().min(width);
    v[..n].copy_from_slice(&bytes[..n]);
    v
}

fn parse_u32(s: &str) -> Result<u32> {
    let s = s.trim();
    let (radix, body) = if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        (16, rest)
    } else {
        (10, s)
    };
    u32::from_str_radix(body, radix).with_context(|| format!("`{s}` is not a u32"))
}

pub(crate) fn maybe_update_rstbl(
    table_path: &Path,
    resource_name: Option<String>,
    out: &Path,
    size: usize,
) -> Result<()> {
    let name = resource_name.unwrap_or_else(|| {
        out.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned()
    });
    update_entry(table_path, &name, size)
}

pub(crate) fn update_entry(table_path: &Path, name: &str, size: usize) -> Result<()> {
    let raw = read_file(table_path)?;
    let mut table =
        Rstbl::parse(&raw).with_context(|| format!("parse `{}`", table_path.display()))?;
    let size_u32 = u32::try_from(size)
        .map_err(|_| anyhow::anyhow!("packed size {size} exceeds u32 (rstbl entry width)"))?;
    if table.get(name).is_none() {
        bail!(
            "resource `{name}` not found in rstbl `{}` (pass --resource-name to match an existing key)",
            table_path.display()
        );
    }
    table.set(name, size_u32);
    let mut buf = Vec::new();
    table
        .write(&mut buf)
        .with_context(|| format!("rewrite `{}`", table_path.display()))?;
    write_file(table_path, &buf)?;
    println!(
        "  updated rstbl `{}`: `{name}` -> {size} bytes",
        table_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomolib::formats::rstbl::DEFAULT_PATH_SIZE;

    #[test]
    fn json_round_trip() {
        let mut t = Rstbl::new(1, DEFAULT_PATH_SIZE);
        t.set_crc_entries(vec![
            CrcEntry {
                hash: 0x0001,
                size: 16,
            },
            CrcEntry {
                hash: 0xDEAD_BEEF,
                size: 64,
            },
        ]);
        t.set_path_entries(vec![PathEntry {
            name: "Foo/Bar.bgyml".into(),
            size: 100,
        }]);
        let text = serde_json::to_string(&RstblDoc(&t)).unwrap();
        let parsed: Document = serde_json::from_str(&text).unwrap();
        let back = build_table(&parsed).unwrap();
        assert_eq!(back.version(), 1);
        assert_eq!(back.path_size(), DEFAULT_PATH_SIZE);
        assert_eq!(back.crc_entries(), t.crc_entries());
        assert_eq!(back.path_entries(), t.path_entries());
    }
}
