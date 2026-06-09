use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::nca::{ContentType, FsEntry, KeySet, Nca, parse_ticket, romfs_entries};
use tomolib::formats::nsp::{Entry, PartitionFs};

use crate::commands::nca::{KeyOpts, load_keys};
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
    /// Extract the program `RomFs` from an NSP, optionally applying an update NSP.
    ///
    /// Decrypts the base program `NCA`'s `RomFs` section. When `--update` points
    /// at an update package, its `BKTR` patch section is merged on top of the
    /// base, so the output reflects the game at the update's version. Title keys
    /// are read from the tickets embedded in each package.
    Romfs {
        /// Path to the base NSP file.
        input: PathBuf,
        /// Path to an update NSP whose patch is applied on top of the base.
        #[arg(short, long)]
        update: Option<PathBuf>,
        /// Destination directory for the extracted `RomFs` tree.
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        keys: KeyOpts,
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
        Verb::Romfs {
            input,
            update,
            out,
            keys,
        } => romfs(&input, update.as_deref(), &out, &keys),
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
        let mut writer = crate::paths::create(&dest)?;

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

    crate::fmt::report(
        "extracted",
        input,
        &out_dir,
        &format!("{} files, {}", fs.entries().len(), fmt_bytes(payload)),
    );
    Ok(())
}

struct SubReader<R> {
    inner: R,
    start: u64,
    len: u64,
    pos: u64,
}

impl<R: Read + Seek> SubReader<R> {
    fn new(inner: R, start: u64, len: u64) -> Self {
        Self {
            inner,
            start,
            len,
            pos: 0,
        }
    }
}

impl<R: Read + Seek> Read for SubReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        let n = buf
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if n == 0 {
            return Ok(0);
        }
        self.inner.seek(SeekFrom::Start(self.start + self.pos))?;
        let got = self.inner.read(&mut buf[..n])?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl<R: Read + Seek> Seek for SubReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(n) => self.len.checked_add_signed(n),
            SeekFrom::Current(n) => self.pos.checked_add_signed(n),
        };
        self.pos =
            new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        Ok(self.pos)
    }
}

fn has_ext(name: &str, ext: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn register_tickets(file: &mut File, fs: &PartitionFs, keys: &mut KeySet) -> Result<()> {
    for e in fs.entries() {
        if !has_ext(&e.name, "tik") {
            continue;
        }
        let len = usize::try_from(e.size).unwrap_or(usize::MAX);
        let mut buf = vec![0u8; len];
        file.seek(SeekFrom::Start(e.offset))?;
        file.read_exact(&mut buf)
            .with_context(|| format!("read ticket `{}`", e.name))?;
        if let Some((rights_id, wrapped)) = parse_ticket(&buf) {
            keys.add_title_key(rights_id, wrapped);
        }
    }
    Ok(())
}

fn find_program_entry(file: &mut File, fs: &PartitionFs, keys: &KeySet) -> Result<Entry> {
    let mut last_err = None;
    for e in fs.entries() {
        let stem = Path::new(&e.name).file_stem().and_then(|s| s.to_str());
        let is_cnmt = stem.is_some_and(|s| has_ext(s, "cnmt"));
        if !has_ext(&e.name, "nca") || is_cnmt {
            continue;
        }
        let mut sub = SubReader::new(&mut *file, e.offset, e.size);
        match Nca::open(&mut sub, keys) {
            Ok(nca) if nca.header.content_type == ContentType::Program => return Ok(e.clone()),
            Ok(_) => {}
            Err(err) => {
                last_err = Some(anyhow::Error::new(err).context(format!("open NCA `{}`", e.name)));
            }
        }
    }
    match last_err {
        Some(err) => Err(err.context("no Program NCA could be opened in package")),
        None => bail!("no Program NCA found in package"),
    }
}

fn romfs(input: &Path, update: Option<&Path>, out: &Path, keyopts: &KeyOpts) -> Result<()> {
    let mut keys = load_keys(keyopts)?;

    let mut base_file = File::open(input).with_context(|| format!("open `{}`", input.display()))?;
    let base_fs = PartitionFs::read_header(&mut base_file)
        .with_context(|| format!("parse `{}`", input.display()))?;
    register_tickets(&mut base_file, &base_fs, &mut keys)?;
    let base_entry = find_program_entry(&mut base_file, &base_fs, &keys)
        .with_context(|| format!("locate program NCA in `{}`", input.display()))?;

    let Some(update) = update else {
        let mut base_reader = SubReader::new(base_file, base_entry.offset, base_entry.size);
        let base_nca = Nca::open(&mut base_reader, &keys).context("open base program NCA")?;
        let base_part = base_nca
            .romfs_partition()
            .context("base program NCA has no RomFs partition")?
            .clone();
        let entries = base_nca
            .list_partition(&mut base_reader, &base_part)
            .context("read base RomFs")?;
        return dump_entries(&entries, input, out, |e, mut writer, pb| {
            base_nca.copy_file(&mut base_reader, &base_part, e, &mut writer)?;
            pb.inc(e.size);
            Ok(())
        });
    };

    let mut up_file = File::open(update).with_context(|| format!("open `{}`", update.display()))?;
    let up_fs = PartitionFs::read_header(&mut up_file)
        .with_context(|| format!("parse `{}`", update.display()))?;
    register_tickets(&mut up_file, &up_fs, &mut keys)?;
    let up_entry = find_program_entry(&mut up_file, &up_fs, &keys)
        .with_context(|| format!("locate program NCA in `{}`", update.display()))?;

    let mut base_reader = SubReader::new(base_file, base_entry.offset, base_entry.size);
    let mut patch_reader = SubReader::new(up_file, up_entry.offset, up_entry.size);
    let base_nca = Nca::open(&mut base_reader, &keys).context("open base program NCA")?;
    let patch_nca = Nca::open(&mut patch_reader, &keys).context("open update program NCA")?;
    if patch_nca.program_id() != base_nca.program_id() {
        bail!(
            "base/update program id mismatch ({:016x} vs {:016x})",
            base_nca.program_id(),
            patch_nca.program_id()
        );
    }

    let base_part = base_nca
        .romfs_partition()
        .context("base program NCA has no RomFs partition")?;
    let patch_part = patch_nca
        .romfs_partition()
        .context("update program NCA has no RomFs partition")?;
    if !patch_part.is_patch() {
        bail!("update RomFs section is not a BKTR patch; is this really an update?");
    }

    let tables = patch_nca
        .patch_tables(&mut patch_reader, patch_part)
        .context("read BKTR tables")?;
    let mut stream = patch_nca.patch_stream(
        &tables,
        &mut patch_reader,
        patch_part,
        &base_nca,
        &mut base_reader,
        base_part,
    )?;
    let entries = romfs_entries(&mut stream).context("read merged RomFs")?;
    let mut buf = vec![0u8; COPY_BUF_SIZE];
    dump_entries(&entries, input, out, |e, writer, pb| {
        stream.seek(SeekFrom::Start(e.offset))?;
        let mut remaining = e.size;
        while remaining > 0 {
            let want = buf
                .len()
                .min(usize::try_from(remaining).unwrap_or(usize::MAX));
            stream.read_exact(&mut buf[..want])?;
            writer.write_all(&buf[..want])?;
            remaining -= want as u64;
            pb.inc(want as u64);
        }
        Ok(())
    })
}

fn dump_entries(
    entries: &[FsEntry],
    input: &Path,
    out: &Path,
    mut copy: impl FnMut(&FsEntry, &mut dyn Write, &ProgressBar) -> Result<()>,
) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("create `{}`", out.display()))?;
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let pb = romfs_progress(total);

    for e in entries {
        let dest = romfs_dest(out, &e.path)?;
        pb.set_message(e.path.clone());
        let mut writer = crate::paths::create(&dest)?;
        copy(e, &mut writer, &pb).with_context(|| format!("extract `{}`", e.path))?;
    }
    pb.finish_and_clear();

    crate::fmt::report(
        "extracted",
        input,
        out,
        &format!("{} files, {}", entries.len(), fmt_bytes(total)),
    );
    Ok(())
}

fn romfs_dest(out: &Path, path: &str) -> Result<PathBuf> {
    let safe = sanitize_relative(path).with_context(|| format!("unsafe entry path `{path}`"))?;
    let dest = out.join(&safe);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create `{}`", parent.display()))?;
    }
    Ok(dest)
}

fn romfs_progress(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.cyan} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})",
        )
        .expect("valid template")
        .progress_chars("=> "),
    );
    pb
}
