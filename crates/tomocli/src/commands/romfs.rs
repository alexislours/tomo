use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use rayon::prelude::*;
use tomolib::formats::{
    ainb::AINB_MAGIC,
    amta::AMTA_MAGIC,
    bars::BARS_MAGIC,
    bntx::BNTX_MAGIC,
    bwav::BWAV_MAGIC,
    byml::{BYML_MAGIC_BE, BYML_MAGIC_LE},
    msbp::MSBP_MAGIC,
    msbt::MSBT_MAGIC,
    rstbl::RSTBL_MAGIC,
    sarc::{SARC_MAGIC, Sarc},
    zs,
};

use crate::commands::lms::Registry;

use crate::fmt::fmt_bytes;
use crate::paths::{append_ext, sanitize_relative, unnamed_entry};

const ZSTD_MAGIC: [u8; 4] = zs::ZSTD_MAGIC.to_le_bytes();

const ZS_STRIP_EXTS: &[&str] = &["zs", "szs"];
const SARC_STRIP_EXTS: &[&str] = &["sarc", "pack", "blarc", "bfarc", "bars", "baatarc"];

const MAX_NEST_DEPTH: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, ValueEnum)]
enum Kind {
    Sarc,
    Zs,
    Rstbl,
    Byml,
    Msbt,
    Msbp,
    Bntx,
    Bars,
    Bamta,
    Bwav,
    Bnvib,
    Ainb,
}

impl Kind {
    fn magics(self) -> &'static [&'static [u8]] {
        match self {
            Self::Sarc => &[&SARC_MAGIC],
            Self::Zs => &[&ZSTD_MAGIC],
            Self::Rstbl => &[&RSTBL_MAGIC],
            Self::Byml => &[&BYML_MAGIC_LE, &BYML_MAGIC_BE],
            Self::Msbt => &[&MSBT_MAGIC],
            Self::Msbp => &[&MSBP_MAGIC],
            Self::Bntx => &[&BNTX_MAGIC],
            Self::Bars => &[&BARS_MAGIC],
            Self::Bamta => &[&AMTA_MAGIC],
            Self::Bwav => &[&BWAV_MAGIC],
            Self::Bnvib => &[
                &[0x04, 0, 0, 0, 0x03, 0],
                &[0x0C, 0, 0, 0, 0x03, 0],
                &[0x10, 0, 0, 0, 0x03, 0],
            ],
            Self::Ainb => &[&AINB_MAGIC],
        }
    }

    fn detect(bytes: &[u8]) -> Option<Self> {
        Self::value_variants()
            .iter()
            .copied()
            .find(|k| k.magics().iter().any(|m| has_prefix(bytes, m)))
    }

    fn name(self) -> &'static str {
        match self {
            Self::Sarc => "sarc",
            Self::Zs => "zs",
            Self::Rstbl => "rstbl",
            Self::Byml => "byml",
            Self::Msbt => "msbt",
            Self::Msbp => "msbp",
            Self::Bntx => "bntx",
            Self::Bars => "bars",
            Self::Bamta => "bamta",
            Self::Bwav => "bwav",
            Self::Bnvib => "bnvib",
            Self::Ainb => "ainb",
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
struct ConvertStat {
    files: u64,
    input_bytes: u64,
    output_bytes: u64,
    nanos: u128,
}

#[derive(Debug, Args)]
pub(crate) struct RomfsArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Recursively unpack every supported container under <input>.
    ///
    /// Walks the input tree, mirrors it under <out>, and for each file
    /// transparently decompresses zstd frames and extracts SARC archives,
    /// repeating until no recognized magic remains. Detection is magic-based,
    /// never extension-based. Collisions error.
    Extract {
        /// Directory tree to walk.
        input: PathBuf,
        /// Destination directory. Created if missing; must not contain colliding paths.
        #[arg(short, long)]
        out: PathBuf,
        /// Only emit files whose content (by magic) is one of these kinds.
        /// When a matched kind is also a container, recursion halts and the
        /// raw bytes are written. Other containers are still descended into
        /// to find inner matches. Default: every leaf is emitted.
        #[arg(long, value_delimiter = ',', value_enum)]
        only: Vec<Kind>,
        /// Convert known leaf formats to a text form (RESTBL to JSON; byml,
        /// msbt, msbp, ainb and bnvib to YAML). Containers are decomposed into
        /// directories: bntx/bwav into blob bundles, bars into per-asset bwav
        /// bundles and bamta YAML sidecars. Converted files are written next to
        /// the original output path with the matching extension appended.
        /// Other formats are emitted as raw bytes.
        #[arg(long)]
        convert: bool,
    },
}

pub(crate) fn run(args: RomfsArgs) -> Result<()> {
    match args.verb {
        Verb::Extract {
            input,
            out,
            only,
            convert,
        } => {
            let filter = if only.is_empty() {
                None
            } else {
                Some(only.into_iter().collect::<HashSet<_>>())
            };
            extract(&input, &out, filter.as_ref(), convert)
        }
    }
}

fn extract(input: &Path, out: &Path, filter: Option<&HashSet<Kind>>, convert: bool) -> Result<()> {
    if !input.is_dir() {
        bail!("input `{}` must be a directory", input.display());
    }
    fs::create_dir_all(out).with_context(|| format!("create `{}`", out.display()))?;

    let started = Instant::now();
    let mut inputs = Vec::new();
    collect_files(input, input, &mut inputs)?;
    if inputs.is_empty() {
        bail!("no files found under `{}`", input.display());
    }

    let n_threads = rayon::current_num_threads().max(1);
    let mp = MultiProgress::new();
    let spinner_style = ProgressStyle::with_template("{spinner:.cyan} [{prefix:>4}] {wide_msg}")
        .expect("valid template")
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
    let spinners: Vec<ProgressBar> = (0..n_threads)
        .map(|i| {
            let pb = mp.add(ProgressBar::new_spinner());
            pb.set_style(spinner_style.clone());
            pb.set_prefix(format!("#{i}"));
            pb.enable_steady_tick(Duration::from_millis(100));
            pb.set_message("idle".to_string());
            pb
        })
        .collect();
    let counter = mp.add(ProgressBar::new_spinner());
    counter.set_style(
        ProgressStyle::with_template("{spinner:.green} {wide_msg}").expect("valid template"),
    );
    counter.enable_steady_tick(Duration::from_millis(100));
    counter.set_message("0 files written".to_string());

    let ctx = ProcessCtx {
        spinners,
        counter: counter.clone(),
        files_written: AtomicU64::new(0),
        files_skipped: AtomicU64::new(0),
        bytes_written: AtomicU64::new(0),
        containers_unpacked: AtomicU64::new(0),
        convert_stats: Mutex::new(BTreeMap::new()),
        filter: filter.cloned(),
        convert,
    };
    let ctx = Arc::new(ctx);

    let result: Result<()> = inputs.par_iter().try_for_each(|rel| {
        let in_path = input.join(rel);
        let out_path = out.join(rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create `{}`", parent.display()))?;
        }
        ctx.set_current(rel.display().to_string());
        let bytes = fs::read(&in_path).with_context(|| format!("read `{}`", in_path.display()))?;
        process(&ctx, bytes, &out_path, 0, None)
    });

    for pb in &ctx.spinners {
        pb.finish_and_clear();
    }
    counter.finish_and_clear();
    result?;

    let elapsed = started.elapsed();
    let files = ctx.files_written.load(Ordering::Relaxed);
    let skipped = ctx.files_skipped.load(Ordering::Relaxed);
    let bytes = ctx.bytes_written.load(Ordering::Relaxed);
    let containers = ctx.containers_unpacked.load(Ordering::Relaxed);
    println!();
    println!("{}", "romfs extract complete".green().bold());
    println!(
        "  {} files written, {} ({} containers unpacked)",
        files,
        fmt_bytes(bytes),
        containers,
    );
    if skipped > 0 {
        println!("  {skipped} files skipped by --only filter");
    }
    let elapsed_ms = elapsed.as_millis().max(1);
    let rate_u128 = u128::from(bytes).saturating_mul(1000) / elapsed_ms;
    let rate = u64::try_from(rate_u128).unwrap_or(u64::MAX);
    println!("  elapsed {elapsed:.2?} ({}/s)", fmt_bytes(rate));

    if convert {
        print_convert_report(&ctx);
    }
    Ok(())
}

fn print_convert_report(ctx: &ProcessCtx) {
    let stats = ctx.convert_stats.lock().expect("convert_stats poisoned");
    if stats.is_empty() {
        return;
    }
    let workers = u128::try_from(rayon::current_num_threads().max(1)).unwrap_or(1);
    println!();
    println!("{}", "convert breakdown (per type)".green().bold());
    for (kind, stat) in stats.iter() {
        let nanos = (stat.nanos / workers).max(1);
        let out_per_sec = u128::from(stat.output_bytes).saturating_mul(1_000_000_000) / nanos;
        let out_per_sec = u64::try_from(out_per_sec).unwrap_or(u64::MAX);
        let fps_deci = (u128::from(stat.files) * 10_000_000_000 + nanos / 2) / nanos;
        let files_per_sec = format!("{}.{}", fps_deci / 10, fps_deci % 10);
        let dur = Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX));
        println!(
            "  {}: {} files, {} in -> {} out in {:.2?} ({}/s out, {} files/s)",
            kind.name(),
            stat.files,
            fmt_bytes(stat.input_bytes),
            fmt_bytes(stat.output_bytes),
            dur,
            fmt_bytes(out_per_sec),
            files_per_sec,
        );
    }
}

#[derive(Debug)]
struct ProcessCtx {
    spinners: Vec<ProgressBar>,
    counter: ProgressBar,
    files_written: AtomicU64,
    files_skipped: AtomicU64,
    bytes_written: AtomicU64,
    containers_unpacked: AtomicU64,
    convert_stats: Mutex<BTreeMap<Kind, ConvertStat>>,
    filter: Option<HashSet<Kind>>,
    convert: bool,
}

impl ProcessCtx {
    fn set_current(&self, msg: String) {
        let idx = rayon::current_thread_index().unwrap_or(0);
        if let Some(pb) = self.spinners.get(idx) {
            pb.set_message(msg);
        }
    }

    fn note_written(&self, n: u64) {
        let count = self.files_written.fetch_add(1, Ordering::Relaxed) + 1;
        let total = self.bytes_written.fetch_add(n, Ordering::Relaxed) + n;
        self.counter
            .set_message(format!("{count} files written, {}", fmt_bytes(total)));
    }

    fn note_skipped(&self) {
        self.files_skipped.fetch_add(1, Ordering::Relaxed);
    }

    fn note_container(&self) {
        self.containers_unpacked.fetch_add(1, Ordering::Relaxed);
    }

    fn note_convert(&self, kind: Kind, input_bytes: u64, output_bytes: u64, dur: Duration) {
        self.note_convert_n(kind, 1, input_bytes, output_bytes, dur);
    }

    fn note_convert_n(
        &self,
        kind: Kind,
        files: u64,
        input_bytes: u64,
        output_bytes: u64,
        dur: Duration,
    ) {
        let mut stats = self.convert_stats.lock().expect("convert_stats poisoned");
        let entry = stats.entry(kind).or_default();
        entry.files += files;
        entry.input_bytes += input_bytes;
        entry.output_bytes += output_bytes;
        entry.nanos += dur.as_nanos();
    }

    fn allows(&self, kind: Option<Kind>) -> bool {
        filter_allows(self.filter.as_ref(), kind)
    }
}

fn filter_allows(filter: Option<&HashSet<Kind>>, kind: Option<Kind>) -> bool {
    match (filter, kind) {
        (None, _) => true,
        (Some(set), Some(k)) => set.contains(&k),
        (Some(_), None) => false,
    }
}

fn process(
    ctx: &ProcessCtx,
    bytes: Vec<u8>,
    out_path: &Path,
    depth: u32,
    reg: Option<Arc<Registry>>,
) -> Result<()> {
    let kind = Kind::detect(&bytes);

    if ctx.filter.is_some() && ctx.allows(kind) {
        return emit_leaf(ctx, &bytes, out_path, kind, reg.as_deref());
    }

    if kind.is_some() && depth >= MAX_NEST_DEPTH {
        bail!(
            "nested container depth exceeded {MAX_NEST_DEPTH} at `{}`",
            out_path.display()
        );
    }

    match kind {
        Some(Kind::Zs) => {
            ctx.note_container();
            ctx.set_current(format!("zs  {}", out_path.display()));
            let mut decompressed = Vec::with_capacity(bytes.len().saturating_mul(2));
            zs::decompress(Cursor::new(&bytes), &mut decompressed)
                .with_context(|| format!("decompress `{}`", out_path.display()))?;
            drop(bytes);
            let next = strip_known_ext(out_path, ZS_STRIP_EXTS);
            process(ctx, decompressed, &next, depth + 1, reg)
        }
        Some(Kind::Sarc) => {
            ctx.note_container();
            let dir = strip_known_ext(out_path, SARC_STRIP_EXTS);
            ctx.set_current(format!("sarc {}", dir.display()));
            fs::create_dir(&dir).with_context(|| {
                format!(
                    "create directory `{}` (path already taken; collision?)",
                    dir.display()
                )
            })?;
            let sarc = Sarc::parse(bytes)
                .with_context(|| format!("parse SARC for `{}`", out_path.display()))?;
            let entries = sarc.entries();
            let sarc_reg = if ctx.convert {
                entries
                    .iter()
                    .find(|e| {
                        e.name.as_deref().is_some_and(|n| {
                            std::path::Path::new(n)
                                .extension()
                                .is_some_and(|e| e.eq_ignore_ascii_case("msbp"))
                        })
                    })
                    .and_then(|e| super::lms::registry_from_msbp_bytes(sarc.data(e)).map(Arc::new))
                    .or(reg)
            } else {
                reg
            };
            entries
                .par_iter()
                .enumerate()
                .try_for_each(|(i, entry)| -> Result<()> {
                    let name = entry.name.clone().unwrap_or_else(|| unnamed_entry(i));
                    let safe = sanitize_relative(&name)
                        .with_context(|| format!("unsafe entry path `{name}`"))?;
                    let dest = dir.join(safe);
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent)
                            .with_context(|| format!("create `{}`", parent.display()))?;
                    }
                    let data = sarc.data(entry).to_vec();
                    process(ctx, data, &dest, depth + 1, sarc_reg.clone())
                })?;
            Ok(())
        }
        Some(
            Kind::Rstbl
            | Kind::Byml
            | Kind::Msbt
            | Kind::Msbp
            | Kind::Bntx
            | Kind::Bars
            | Kind::Bamta
            | Kind::Bwav
            | Kind::Bnvib
            | Kind::Ainb,
        )
        | None => {
            if !ctx.allows(kind) {
                ctx.note_skipped();
                return Ok(());
            }
            emit_leaf(ctx, &bytes, out_path, kind, reg.as_deref())
        }
    }
}

fn emit_leaf(
    ctx: &ProcessCtx,
    bytes: &[u8],
    out_path: &Path,
    kind: Option<Kind>,
    reg: Option<&Registry>,
) -> Result<()> {
    if ctx.convert
        && let Some(k) = kind
    {
        if matches!(k, Kind::Bntx | Kind::Bars | Kind::Bwav) {
            let started = Instant::now();
            let dir = append_ext(out_path, "d");
            let mut inner = super::bars::InnerConverts::default();
            let written = match k {
                Kind::Bntx => super::bntx::convert_to_bundle(bytes, &dir, true)
                    .with_context(|| format!("convert `{}`", out_path.display()))?,
                Kind::Bars => {
                    let (w, c) = super::bars::convert_to_bundle(bytes, &dir, true)
                        .with_context(|| format!("convert `{}`", out_path.display()))?;
                    inner = c;
                    w
                }
                Kind::Bwav => super::bwav::convert_to_bundle(bytes, &dir, true)
                    .with_context(|| format!("convert `{}`", out_path.display()))?,
                _ => unreachable!(),
            };
            let elapsed = started.elapsed();
            if inner.bamta_files > 0 {
                ctx.note_convert(
                    k,
                    (bytes.len() as u64).saturating_sub(inner.bamta_in),
                    written.saturating_sub(inner.bamta_out),
                    elapsed.saturating_sub(inner.bamta_dur),
                );
                ctx.note_convert_n(
                    Kind::Bamta,
                    inner.bamta_files,
                    inner.bamta_in,
                    inner.bamta_out,
                    inner.bamta_dur,
                );
            } else {
                ctx.note_convert(k, bytes.len() as u64, written, elapsed);
            }
            ctx.note_written(written);
            return Ok(());
        }
        let started = Instant::now();
        if let Some((converted, ext)) = convert_leaf(bytes, k, reg)
            .with_context(|| format!("convert `{}`", out_path.display()))?
        {
            let elapsed = started.elapsed();
            ctx.note_convert(k, bytes.len() as u64, converted.len() as u64, elapsed);
            let dest = append_ext(out_path, ext);
            return emit_terminal(ctx, &converted, &dest);
        }
    }
    emit_terminal(ctx, bytes, out_path)
}

fn convert_leaf(
    bytes: &[u8],
    kind: Kind,
    reg: Option<&Registry>,
) -> Result<Option<(Vec<u8>, &'static str)>> {
    match kind {
        Kind::Rstbl => super::rstbl::convert_bytes_to_json(bytes).map(|b| Some((b, "json"))),
        Kind::Byml => super::byml::convert_bytes_to_yaml(bytes).map(|b| Some((b, "yml"))),
        Kind::Msbt => Ok(super::lms::msbt_to_yaml(bytes, reg).map(|b| Some((b, "yml")))?),
        Kind::Msbp => Ok(super::lms::msbp_to_yaml(bytes).map(|b| Some((b, "yml")))?),
        Kind::Ainb => super::ainb::convert_bytes_to_yaml(bytes).map(|b| Some((b, "yml"))),
        Kind::Bnvib => super::bnvib::convert_to_yaml(bytes).map(|b| Some((b, "yml"))),
        Kind::Bamta => super::bamta::convert_to_yaml(bytes).map(|b| Some((b, "yml"))),
        Kind::Bntx | Kind::Sarc | Kind::Zs | Kind::Bars | Kind::Bwav => Ok(None),
    }
}

fn emit_terminal(ctx: &ProcessCtx, bytes: &[u8], out_path: &Path) -> Result<()> {
    let len = bytes.len() as u64;
    write_new(out_path, bytes)
        .with_context(|| format!("write `{}` (collision?)", out_path.display()))?;
    ctx.note_written(len);
    Ok(())
}

fn has_prefix(bytes: &[u8], magic: &[u8]) -> bool {
    bytes.len() >= magic.len() && &bytes[..magic.len()] == magic
}

fn strip_known_ext(p: &Path, known: &[&str]) -> PathBuf {
    match p.extension().and_then(|e| e.to_str()) {
        Some(ext) if known.iter().any(|k| k.eq_ignore_ascii_case(ext)) => p.with_extension(""),
        _ => p.to_path_buf(),
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir `{}`", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else if ft.is_file() {
            let rel = path
                .strip_prefix(root)
                .expect("walked path is under root")
                .to_path_buf();
            out.push(rel);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_magic(magic: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(magic);
        v.extend_from_slice(&[0u8; 32]);
        v
    }

    #[test]
    fn detect_matches_each_magic() {
        assert_eq!(Kind::detect(&with_magic(&ZSTD_MAGIC)), Some(Kind::Zs));
        assert_eq!(Kind::detect(&with_magic(&SARC_MAGIC)), Some(Kind::Sarc));
        assert_eq!(Kind::detect(&with_magic(&RSTBL_MAGIC)), Some(Kind::Rstbl));
        assert_eq!(Kind::detect(&with_magic(&BYML_MAGIC_LE)), Some(Kind::Byml));
        assert_eq!(Kind::detect(&with_magic(&BYML_MAGIC_BE)), Some(Kind::Byml));
        assert_eq!(Kind::detect(&with_magic(&MSBT_MAGIC)), Some(Kind::Msbt));
        assert_eq!(Kind::detect(&with_magic(&MSBP_MAGIC)), Some(Kind::Msbp));
        assert_eq!(Kind::detect(&with_magic(&BNTX_MAGIC)), Some(Kind::Bntx));
        assert_eq!(Kind::detect(&with_magic(&BARS_MAGIC)), Some(Kind::Bars));
        assert_eq!(Kind::detect(&with_magic(&AMTA_MAGIC)), Some(Kind::Bamta));
        assert_eq!(Kind::detect(&with_magic(&BWAV_MAGIC)), Some(Kind::Bwav));
        assert_eq!(
            Kind::detect(&with_magic(&[0x0C, 0, 0, 0, 0x03, 0])),
            Some(Kind::Bnvib)
        );
        assert_eq!(Kind::detect(&with_magic(&AINB_MAGIC)), Some(Kind::Ainb));
    }

    #[test]
    fn detect_rejects_unknown_and_short() {
        assert_eq!(Kind::detect(b"not a real magic header"), None);
        assert_eq!(Kind::detect(b""), None);
        assert_eq!(Kind::detect(b"S"), None);
        assert_eq!(Kind::detect(&BNTX_MAGIC[..3]), None);
    }

    #[test]
    fn has_prefix_handles_lengths() {
        assert!(has_prefix(b"SARCxxxx", &SARC_MAGIC));
        assert!(!has_prefix(b"SARZxxxx", &SARC_MAGIC));
        assert!(!has_prefix(b"SAR", &SARC_MAGIC));
        assert!(has_prefix(b"abc", b""));
    }

    #[test]
    fn strip_known_ext_is_case_insensitive() {
        assert_eq!(
            strip_known_ext(Path::new("a/b.pack.zs"), ZS_STRIP_EXTS),
            PathBuf::from("a/b.pack")
        );
        assert_eq!(
            strip_known_ext(Path::new("a/b.ZS"), ZS_STRIP_EXTS),
            PathBuf::from("a/b")
        );
        assert_eq!(
            strip_known_ext(Path::new("a/b.pack"), SARC_STRIP_EXTS),
            PathBuf::from("a/b")
        );
    }

    #[test]
    fn strip_known_ext_leaves_unknown_or_missing() {
        assert_eq!(
            strip_known_ext(Path::new("a/b.byml"), ZS_STRIP_EXTS),
            PathBuf::from("a/b.byml")
        );
        assert_eq!(
            strip_known_ext(Path::new("a/b"), ZS_STRIP_EXTS),
            PathBuf::from("a/b")
        );
    }

    #[test]
    fn kind_name_round_trips_through_value_enum() {
        for kind in [
            Kind::Sarc,
            Kind::Zs,
            Kind::Rstbl,
            Kind::Byml,
            Kind::Msbt,
            Kind::Msbp,
            Kind::Bntx,
            Kind::Bars,
            Kind::Bamta,
            Kind::Bwav,
            Kind::Bnvib,
            Kind::Ainb,
        ] {
            let parsed = <Kind as ValueEnum>::from_str(kind.name(), false);
            assert_eq!(parsed, Ok(kind), "name `{}` should parse back", kind.name());
        }
    }

    #[test]
    fn romfs_handles_every_format_command() {
        use clap::CommandFactory;

        let outer_containers: HashSet<&str> = ["romfs", "nsp", "nca"].into_iter().collect();
        let non_format: HashSet<&str> = ["completions"].into_iter().collect();
        let kinds: HashSet<&str> = Kind::value_variants().iter().map(|k| k.name()).collect();
        for sub in crate::Cli::command().get_subcommands() {
            let name = sub.get_name();
            if outer_containers.contains(name) || non_format.contains(name) {
                continue;
            }
            assert!(
                kinds.contains(name),
                "`tomo {name}` is a format command but romfs Kind has no matching variant; \
                 add it to Kind so `romfs extract` can detect it, or list it in \
                 `outer_containers` if it wraps a romfs tree rather than living inside one"
            );
        }
    }

    #[test]
    fn filter_allows_logic() {
        assert!(filter_allows(None, Some(Kind::Byml)));
        assert!(filter_allows(None, None));

        let set: HashSet<Kind> = [Kind::Byml, Kind::Rstbl].into_iter().collect();
        assert!(filter_allows(Some(&set), Some(Kind::Byml)));
        assert!(!filter_allows(Some(&set), Some(Kind::Sarc)));
        assert!(!filter_allows(Some(&set), None));
    }
}
