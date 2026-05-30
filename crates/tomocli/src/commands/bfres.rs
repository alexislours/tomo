use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;
use saphyr::{LoadableYamlNode, Yaml};
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::bfres::{Bfres, model};

use crate::commands::yaml::{get, quote as yaml_quote};
use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, order_str, plural, value};
use crate::paths::{append_ext, read_file, write_file};

mod gltf;

const MANIFEST: &str = "bfres.yml";
const CONTAINER: &str = "container.bin";

#[derive(Debug, clap::Args)]
pub(crate) struct BfresArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, clap::Subcommand)]
enum Verb {
    /// Print a summary of a BFRES resource container.
    Info {
        /// Path to the BFRES file.
        input: PathBuf,
        /// List models, sub-files, and embedded files in detail.
        #[arg(short, long)]
        list: bool,
        #[command(flatten)]
        common: crate::fmt::InfoArgs,
    },
    /// Extract a BFRES to a `<name>.bfres.d/` bundle: a `bfres.yml` manifest, a
    /// lossless `container.bin`, each embedded file (e.g. a `.bntx`), and a
    /// `.glb` per model where geometry can be decoded.
    Extract {
        /// Path to the BFRES file.
        input: PathBuf,
        /// Output directory. Defaults to `<input>.d`.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Skip glTF model export (faster; the container and embedded files are still written).
        #[arg(long)]
        no_models: bool,
    },
    /// Rebuild a BFRES from a bundle directory. Byte-for-byte for unmodified
    /// input; edited embedded files are spliced back in, relocating the file
    /// (and its `_RLT`) when their size changes.
    Pack {
        /// A `<name>.bfres.d/` bundle produced by `extract`.
        input: PathBuf,
        /// Output BFRES. Defaults to the bundle name with the `.d` stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
    /// Export the models in a BFRES to glTF binary (`.glb`).
    Gltf {
        /// Path to the BFRES file.
        input: PathBuf,
        /// Output directory for the `.glb` files. Defaults to `<input>.gltf.d`.
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

pub(crate) fn run(args: BfresArgs) -> Result<()> {
    match args.verb {
        Verb::Info {
            input,
            list,
            common,
        } => info(&input, list, common.json),
        Verb::Extract {
            input,
            out,
            no_models,
        } => {
            let dir = out.unwrap_or_else(|| append_ext(&input, "d"));
            let bytes = read_file(&input)?;
            let bfres =
                Bfres::parse(&bytes).with_context(|| format!("parse `{}`", input.display()))?;
            let n = write_bundle(&bfres, &dir, !no_models)?;
            crate::fmt::report(
                "extracted",
                &input,
                &dir,
                &format!("{n} model{}", plural(n)),
            );
            Ok(())
        }
        Verb::Pack { input, out } => {
            let bytes = pack_bundle(&input)?;
            let dest = out.unwrap_or_else(|| strip_d(&input));
            write_file(&dest, &bytes)?;
            crate::fmt::report("packed", &input, &dest, &fmt_bytes(bytes.len() as u64));
            Ok(())
        }
        Verb::Gltf { input, out } => {
            let dir = out.unwrap_or_else(|| append_ext(&input, "gltf.d"));
            let bytes = read_file(&input)?;
            let bfres =
                Bfres::parse(&bytes).with_context(|| format!("parse `{}`", input.display()))?;
            let n = export_models(&bfres, &dir)?;
            crate::fmt::report("exported", &input, &dir, &format!("{n} model{}", plural(n)));
            Ok(())
        }
    }
}

fn info_json(
    input: &Path,
    bfres: &Bfres,
    bytes: &[u8],
    models: &[model::ModelInfo],
    list: bool,
) -> Result<()> {
    let (maj, min, mic, _) = bfres.version_tuple();
    let order = order_str(bfres.byte_order);
    let mut obj = serde_json::json!({
        "file": input.display().to_string(),
        "name": bfres.name,
        "version": format!("{maj}.{min}.{mic}"),
        "byte_order": order,
        "alignment": bfres.alignment(),
        "total_size": bytes.len(),
        "models": bfres.models.names.len(),
        "skeletal_anims": bfres.skeletal_anims.names.len(),
        "material_anims": bfres.material_anims.names.len(),
        "bone_visibility_anims": bfres.bone_visibility_anims.names.len(),
        "shape_anims": bfres.shape_anims.names.len(),
        "scene_anims": bfres.scene_anims.names.len(),
        "embedded_files": bfres.embedded_files.len(),
    });
    if list {
        obj["model_list"] = models
            .iter()
            .map(|m| {
                serde_json::json!({
                    "name": m.name,
                    "vertex_buffers": m.vertex_buffers,
                    "shapes": m.shapes,
                    "materials": m.materials,
                })
            })
            .collect();
        obj["embedded_list"] = bfres
            .embedded_files
            .iter()
            .map(|f| serde_json::json!({ "name": f.name, "size": f.size }))
            .collect();
    }
    crate::fmt::print_json(&obj)
}

fn info(input: &Path, list: bool, json: bool) -> Result<()> {
    let bytes = read_file(input)?;
    let bfres = Bfres::parse(&bytes).with_context(|| format!("parse `{}`", input.display()))?;
    let (maj, min, mic, _) = bfres.version_tuple();
    let order = order_str(bfres.byte_order);

    if json {
        let models = model::model_infos(&bfres);
        return info_json(input, &bfres, &bytes, &models, list);
    }

    let mut t = Builder::default();
    let mut row = |k: &str, v: String, extra: String| t.push_record([label(k), value(v), extra]);
    row("Name", bfres.name.clone(), String::new());
    row("Version", format!("{maj}.{min}.{mic}"), String::new());
    row("Byte order", order.to_string(), String::new());
    row(
        "Alignment",
        format!("{:#x}", bfres.alignment()),
        format!("{} bytes", bfres.alignment()).dimmed().to_string(),
    );
    row(
        "Models",
        bfres.models.names.len().to_string(),
        String::new(),
    );
    row(
        "Embedded files",
        bfres.embedded_files.len().to_string(),
        String::new(),
    );
    let total = bytes.len() as u64;
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    let subfiles = [
        ("skeletal anims", &bfres.skeletal_anims.names),
        ("material anims", &bfres.material_anims.names),
        ("bone visibility anims", &bfres.bone_visibility_anims.names),
        ("shape anims", &bfres.shape_anims.names),
        ("scene anims", &bfres.scene_anims.names),
    ];
    let extra: Vec<_> = subfiles.iter().filter(|(_, v)| !v.is_empty()).collect();
    if !extra.is_empty() {
        println!();
        println!("  {}", "sub-files:".dimmed());
        for (label, v) in extra {
            println!("    {label}: {}", v.len());
        }
    }

    if list {
        for m in &model::model_infos(&bfres) {
            println!();
            println!("  {} {}", "model".dimmed(), m.name.bold());
            let mut b = Builder::default();
            let mut row = |k: &str, v: String| b.push_record([label(k), value(v)]);
            row("Vertex buffers", m.vertex_buffers.to_string());
            row("Shapes", m.shapes.to_string());
            row("Materials", m.materials.to_string());
            let mut tt = b.build();
            tt.with(Style::blank()).with(Padding::new(4, 2, 0, 0));
            println!("{tt}");
        }
        if !bfres.embedded_files.is_empty() {
            println!();
            println!("  {}", "embedded files:".dimmed());
            for f in &bfres.embedded_files {
                println!("    {}  ({})", f.name, fmt_bytes(u64::from(f.size)));
            }
        }
    } else if !bfres.models.names.is_empty() {
        println!();
        println!("  {}", "models:".dimmed());
        for name in bfres.models.names.iter().take(8) {
            println!("    {name}");
        }
        if bfres.models.names.len() > 8 {
            println!(
                "    {}",
                format!("... and {} more", bfres.models.names.len() - 8).dimmed()
            );
        }
    }
    Ok(())
}

fn write_bundle(bfres: &Bfres, dir: &Path, models: bool) -> Result<usize> {
    fs::create_dir_all(dir).with_context(|| format!("create `{}`", dir.display()))?;

    write_file(&dir.join(CONTAINER), bfres.raw())?;

    let stems = unique_stems(bfres.embedded_files.iter().map(|f| f.name.as_str()));
    if !bfres.embedded_files.is_empty() {
        fs::create_dir_all(dir.join("embedded"))
            .with_context(|| format!("create `{}/embedded`", dir.display()))?;
    }
    for (f, stem) in bfres.embedded_files.iter().zip(&stems) {
        let data = bfres.embedded_data(f).unwrap_or(&[]);
        write_file(&dir.join("embedded").join(stem), data)?;
    }

    let mut model_count = 0usize;
    let parsed = if models {
        model::parse_models(bfres).ok()
    } else {
        None
    };
    if let Some(parsed) = &parsed
        && !parsed.is_empty()
    {
        fs::create_dir_all(dir.join("models"))
            .with_context(|| format!("create `{}/models`", dir.display()))?;
        let model_stems = unique_stems(parsed.iter().map(|m| m.name.as_str()));
        for (m, stem) in parsed.iter().zip(&model_stems) {
            if let Some(glb) = gltf::build_glb(m) {
                write_file(&dir.join("models").join(format!("{stem}.glb")), &glb)?;
                model_count += 1;
            }
        }
    }

    let manifest = build_manifest(bfres, &stems);
    write_file(&dir.join(MANIFEST), manifest.as_bytes())?;
    Ok(model_count)
}

fn build_manifest(bfres: &Bfres, embedded_stems: &[String]) -> String {
    let order = order_str(bfres.byte_order);
    let (maj, min, mic, _) = bfres.version_tuple();
    let mut m = String::new();
    let _ = writeln!(m, "name: {}", yaml_quote(&bfres.name));
    let _ = writeln!(m, "version: {maj}.{min}.{mic}");
    let _ = writeln!(m, "byte_order: {order}");
    let _ = writeln!(m, "alignment: {:#x}", bfres.alignment());
    let _ = writeln!(m, "container: {CONTAINER}");
    let _ = writeln!(m, "models:");
    for name in &bfres.models.names {
        let _ = writeln!(m, "  - {}", yaml_quote(name));
    }
    let _ = writeln!(m, "embedded:");
    for (f, stem) in bfres.embedded_files.iter().zip(embedded_stems) {
        let _ = writeln!(m, "  - name: {}", yaml_quote(&f.name));
        let _ = writeln!(m, "    file: {}", yaml_quote(&format!("embedded/{stem}")));
        let _ = writeln!(m, "    offset: {:#x}", f.offset);
        let _ = writeln!(m, "    size: {:#x}", f.size);
    }
    m
}

fn pack_bundle(dir: &Path) -> Result<Vec<u8>> {
    if !dir.is_dir() {
        bail!("`{}` is not a bundle directory", dir.display());
    }
    let text = fs::read_to_string(dir.join(MANIFEST))
        .with_context(|| format!("read `{}/{MANIFEST}`", dir.display()))?;
    let docs = Yaml::load_from_str(&text).context("parse bfres.yml")?;
    let doc = docs.first().context("empty bfres.yml")?;

    let container_name = get(doc, "container")
        .and_then(Yaml::as_str)
        .unwrap_or(CONTAINER);
    let mut data = read_file(&dir.join(container_name))?;
    let bfres = Bfres::parse(&data).context("parse container.bin")?;
    let n = bfres.embedded_files.len();

    let mut replacements: Vec<Option<Vec<u8>>> = vec![None; n];
    if let Some(seq) = get(doc, "embedded").and_then(Yaml::as_sequence) {
        if seq.len() != n {
            bail!(
                "manifest lists {} embedded entries but the container has {n}. Do not add or remove `embedded:` entries",
                seq.len()
            );
        }
        for (i, e) in seq.iter().enumerate().take(n) {
            if let Some(name) = get(e, "name").and_then(Yaml::as_str)
                && name != bfres.embedded_files[i].name
            {
                bail!(
                    "embedded entry {i}: manifest name `{name}` does not match container `{}`; do not reorder the `embedded:` list",
                    bfres.embedded_files[i].name
                );
            }
            let file = get(e, "file")
                .and_then(Yaml::as_str)
                .with_context(|| format!("embedded entry {i}: missing `file`"))?;
            let path = dir.join(file);
            if !path.exists() {
                bail!(
                    "embedded entry {i}: `{}` does not exist; pack needs every embedded file the manifest lists",
                    path.display()
                );
            }
            let bytes = read_file(&path)?;
            let current = bfres.embedded_data(&bfres.embedded_files[i]).unwrap_or(&[]);
            if bytes.as_slice() != current {
                replacements[i] = Some(bytes);
            }
        }
    }

    if replacements.iter().all(Option::is_none) {
        return Ok(data);
    }

    let same_size = replacements.iter().enumerate().all(|(i, r)| {
        r.as_ref()
            .is_none_or(|b| b.len() == bfres.embedded_files[i].size as usize)
    });

    if same_size {
        for (i, r) in replacements.iter().enumerate() {
            if let Some(b) = r {
                let off = bfres.embedded_files[i].offset as usize;
                let end = off + b.len();
                if end > data.len() {
                    bail!(
                        "embedded entry {i}: container region {off:#x}..{end:#x} is outside the {} byte container",
                        data.len()
                    );
                }
                data[off..end].copy_from_slice(b);
            }
        }
        Bfres::parse(&data).context("packed BFRES failed to re-parse")?;
        Ok(data)
    } else {
        bfres
            .rebuild_with_embedded(&replacements)
            .map_err(|e| anyhow::anyhow!("relocate embedded files: {e}"))
    }
}

fn export_models(bfres: &Bfres, dir: &Path) -> Result<usize> {
    let models = model::parse_models(bfres).map_err(|e| anyhow::anyhow!("decode models: {e}"))?;
    if models.is_empty() {
        bail!("no models to export");
    }
    fs::create_dir_all(dir).with_context(|| format!("create `{}`", dir.display()))?;
    let stems = unique_stems(models.iter().map(|m| m.name.as_str()));
    let mut n = 0usize;
    let mut skipped = Vec::new();
    for (m, stem) in models.iter().zip(&stems) {
        if let Some(glb) = gltf::build_glb(m) {
            write_file(&dir.join(format!("{stem}.glb")), &glb)?;
            n += 1;
        } else {
            skipped.push(m.name.clone());
        }
    }
    if !skipped.is_empty() {
        eprintln!(
            "warning: {} model(s) had no exportable geometry and were skipped:",
            skipped.len()
        );
        for s in &skipped {
            eprintln!("  - {s}");
        }
    }
    Ok(n)
}

pub(crate) fn convert_to_bundle(bytes: &[u8], dir: &Path, models: bool) -> Result<u64> {
    let bfres = Bfres::parse(bytes)?;
    write_bundle(&bfres, dir, models)?;
    let mut total = 0u64;
    for entry in walk(dir) {
        total += fs::metadata(&entry).map_or(0, |m| m.len());
    }
    Ok(total)
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

fn unique_stems<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    names
        .map(|name| {
            let base = sanitize(name);
            let mut stem = base.clone();
            let mut i = 1u32;
            while !seen.insert(stem.clone()) {
                stem = format!("{base}_{i}");
                i += 1;
            }
            stem
        })
        .collect()
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "unnamed".to_string()
    } else {
        s
    }
}

fn strip_d(dir: &Path) -> PathBuf {
    if dir.extension().is_some_and(|e| e == "d") {
        dir.with_extension("")
    } else {
        append_ext(dir, "bfres")
    }
}
