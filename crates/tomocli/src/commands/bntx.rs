use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};
use tabled::builder::Builder;
use tabled::settings::{Padding, Style};
use tomolib::formats::binio::ByteOrder;
use tomolib::formats::bntx::{Bntx, ImageFormat, Platform, Texture, TextureInfo, image};

use crate::fmt::{extra_bytes, finish_info_table, fmt_bytes, label, value};
use crate::paths::{append_ext, read_file, write_file};

#[derive(Debug, Args)]
pub(crate) struct BntxArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Debug, Subcommand)]
enum Verb {
    /// Print a summary of a BNTX texture container.
    Info {
        /// Path to the BNTX file.
        input: PathBuf,
        /// List per-texture details for every texture.
        #[arg(short, long)]
        list: bool,
    },
    /// Extract a BNTX to a `<name>.bntx.d/` bundle: `meta.json`, a raw swizzled
    /// `.bin` per texture (lossless), and a decoded `.png` preview per texture.
    Extract {
        /// Path to the BNTX file.
        input: PathBuf,
        /// Output directory. Defaults to `<input>.d`.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Skip PNG previews (faster; the lossless `.bin` files are still written).
        #[arg(long)]
        no_preview: bool,
    },
    /// Rebuild a BNTX from a bundle directory (lossless, no original needed).
    Pack {
        /// A `<name>.bntx.d/` bundle produced by `extract`.
        input: PathBuf,
        /// Output BNTX. Defaults to the bundle name with the `.d` stripped.
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Rebuild each texture from its `.png` instead of using the raw
        /// `.bin`. Only formats with an encoder are rebuilt (regenerating the
        /// full mip chain); formats without one keep their lossless `.bin`.
        #[arg(long)]
        from_png: bool,
    },
}

pub(crate) fn run(args: BntxArgs) -> Result<()> {
    match args.verb {
        Verb::Info { input, list } => info(&input, list),
        Verb::Extract {
            input,
            out,
            no_preview,
        } => {
            let dir = out.unwrap_or_else(|| append_ext(&input, "d"));
            let bytes = read_file(&input)?;
            let bntx =
                Bntx::parse(&bytes).with_context(|| format!("parse `{}`", input.display()))?;
            let n = write_bundle(&bntx, &dir, !no_preview)?;
            println!(
                "extracted {} -> {}/ ({n} texture{})",
                input.display(),
                dir.display(),
                plural(n)
            );
            Ok(())
        }
        Verb::Pack {
            input,
            out,
            from_png,
        } => {
            let bntx = read_bundle(&input, from_png)?;
            let dest = out.unwrap_or_else(|| strip_d(&input));
            let bytes = bntx
                .write()
                .with_context(|| format!("serialize `{}`", dest.display()))?;
            write_file(&dest, &bytes)?;
            println!(
                "packed {}/ -> {} ({})",
                input.display(),
                dest.display(),
                fmt_bytes(bytes.len() as u64)
            );
            Ok(())
        }
    }
}

fn info(input: &Path, list: bool) -> Result<()> {
    let bytes = read_file(input)?;
    let bntx = Bntx::parse(&bytes).with_context(|| format!("parse `{}`", input.display()))?;
    let meta = std::fs::metadata(input).with_context(|| format!("stat `{}`", input.display()))?;

    let mut t = Builder::default();
    let mut row = |k: &str, v: String, extra: String| t.push_record([label(k), value(v), extra]);
    let (maj, min, mic) = bntx.version;
    row("Platform", bntx.platform.name().to_string(), String::new());
    row("Version", format!("{maj}.{min}.{mic}"), String::new());
    row("Name", bntx.name.clone(), String::new());
    row(
        "Alignment",
        format!("{:#x}", bntx.alignment()),
        format!("{} bytes", bntx.alignment()).dimmed().to_string(),
    );
    row("Textures", bntx.textures.len().to_string(), String::new());
    let total = meta.len();
    row("Total size", fmt_bytes(total), extra_bytes(total));

    println!();
    println!("  {}", input.display().bold());
    println!();
    println!("{}", finish_info_table(t));

    if list {
        for (i, tex) in bntx.textures.iter().enumerate() {
            print_texture_detail(i, tex);
        }
    } else {
        println!();
        println!("  {}", "textures:".dimmed());
        for tex in bntx.textures.iter().take(8) {
            println!("    {}  ({})", tex.name, texture_summary(tex));
        }
        if bntx.textures.len() > 8 {
            println!(
                "    {}",
                format!("... and {} more", bntx.textures.len() - 8).dimmed()
            );
        }
    }
    Ok(())
}

fn texture_summary(tex: &Texture) -> String {
    format!(
        "{}x{} {} {} mip{}",
        tex.info.width,
        tex.info.height,
        tex.info.format.name(),
        tex.info.mip_count,
        plural(usize::from(tex.info.mip_count)),
    )
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn print_texture_detail(i: usize, tex: &Texture) {
    println!();
    println!("  {} {}", format!("[{i}]").dimmed(), tex.name.bold());
    let mut b = Builder::default();
    let mut row = |k: &str, v: String| b.push_record([label(k), value(v)]);
    row(
        "Dimensions",
        format!("{}x{}", tex.info.width, tex.info.height),
    );
    row("Format", tex.info.format.name());
    row("Mips", tex.info.mip_count.to_string());
    row("Array", tex.info.array_count.to_string());
    row(
        "Tiling",
        format!(
            "{} (block height {})",
            if tex.info.tile_mode == 0 {
                "optimal"
            } else {
                "linear"
            },
            1u32 << tex.info.block_height_log2(),
        ),
    );
    row("Image data", fmt_bytes(u64::from(tex.info.image_size)));
    let mut tt = b.build();
    tt.with(Style::blank()).with(Padding::new(4, 2, 0, 0));
    println!("{tt}");
}

pub(crate) fn write_bundle(bntx: &Bntx, dir: &Path, preview: bool) -> Result<usize> {
    std::fs::create_dir_all(dir).with_context(|| format!("create `{}`", dir.display()))?;
    let stems = unique_stems(&bntx.textures);
    let with_user_data: Vec<&str> = bntx
        .textures
        .iter()
        .filter(|t| !t.user_data.is_empty())
        .map(|t| t.name.as_str())
        .collect();
    if !with_user_data.is_empty() {
        eprintln!(
            "warning: {} texture(s) carry user data that the bundle does not preserve; it will be \
             lost when packing:",
            with_user_data.len()
        );
        for n in &with_user_data {
            eprintln!("  - {n}");
        }
    }
    for (tex, stem) in bntx.textures.iter().zip(&stems) {
        write_file(&dir.join(format!("{stem}.bin")), &tex.image_data)?;
        if preview
            && let Ok(img) = image::decode_texture_rgba(tex, 0)
            && let Ok(png) = image::rgba_to_png(&img)
        {
            write_file(&dir.join(format!("{stem}.png")), &png)?;
        }
    }
    let manifest = Manifest::from_bntx(bntx, &stems);
    let json = serde_json::to_vec_pretty(&manifest)?;
    write_file(&dir.join("meta.json"), &json)?;
    Ok(bntx.textures.len())
}

fn read_bundle(dir: &Path, from_png: bool) -> Result<Bntx> {
    if !dir.is_dir() {
        bail!("`{}` is not a bundle directory", dir.display());
    }
    let json = std::fs::read(dir.join("meta.json"))
        .with_context(|| format!("read `{}/meta.json`", dir.display()))?;
    let manifest: Manifest = serde_json::from_slice(&json)
        .with_context(|| format!("parse `{}/meta.json`", dir.display()))?;
    let stems: Vec<String> = manifest
        .textures
        .iter()
        .map(|t| t.data.strip_suffix(".bin").unwrap_or(&t.data).to_string())
        .collect();
    let mut bntx = manifest.into_bntx(dir)?;
    if from_png {
        let mut reencoded = 0usize;
        let mut skipped: Vec<String> = Vec::new();
        for (tex, stem) in bntx.textures.iter_mut().zip(&stems) {
            match apply_png_edit(dir, tex, stem)? {
                PngEdit::Reencoded => reencoded += 1,
                PngEdit::NotEncodable => {
                    skipped.push(format!("{} ({})", tex.name, tex.info.format.name()));
                }
                PngEdit::NoPng => {}
            }
        }
        if reencoded > 0 {
            println!("re-encoded {reencoded} texture(s) from PNG");
        }
        if !skipped.is_empty() {
            eprintln!(
                "warning: {} texture(s) have no encoder for their format; their PNG edits were \
                 ignored and the original `.bin` data was kept:",
                skipped.len()
            );
            for n in &skipped {
                eprintln!("  - {n}");
            }
        }
    }
    Ok(bntx)
}

enum PngEdit {
    Reencoded,
    NotEncodable,
    NoPng,
}

fn apply_png_edit(dir: &Path, tex: &mut Texture, stem: &str) -> Result<PngEdit> {
    let png_path = dir.join(format!("{stem}.png"));
    if !png_path.exists() {
        return Ok(PngEdit::NoPng);
    }
    if !image::can_encode(&tex.info) {
        return Ok(PngEdit::NotEncodable);
    }
    let png = std::fs::read(&png_path).with_context(|| format!("read `{}`", png_path.display()))?;
    let img =
        image::png_to_rgba(&png).with_context(|| format!("decode `{}`", png_path.display()))?;
    let image_data = image::encode_mips_swizzled(&img, tex)
        .with_context(|| format!("re-encode `{}`", tex.name))?;
    tex.image_data = image_data;
    Ok(PngEdit::Reencoded)
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: [u16; 3],
    byte_order: String,
    alignment_log2: u8,
    target_address_size: u8,
    flag: u16,
    name: String,
    platform: String,
    textures: Vec<TexManifest>,
}

#[derive(Serialize, Deserialize)]
struct TexManifest {
    name: String,
    format: String,
    width: u32,
    height: u32,
    depth: u32,
    mip_count: u16,
    array_count: u32,
    sample_count: u32,
    gpu_access: u32,
    flags: u8,
    dim: u8,
    surface_dim: u32,
    tile_mode: u16,
    swizzle: u16,
    alignment: u32,
    texture_layout: u32,
    texture_layout2: u32,
    channels: [u8; 4],
    reserved: String,
    mip_offsets: Vec<u64>,
    data: String,
}

impl Manifest {
    fn from_bntx(bntx: &Bntx, stems: &[String]) -> Self {
        Self {
            version: [
                bntx.version.0,
                u16::from(bntx.version.1),
                u16::from(bntx.version.2),
            ],
            byte_order: match bntx.byte_order {
                ByteOrder::Little => "little",
                ByteOrder::Big => "big",
            }
            .to_string(),
            alignment_log2: bntx.alignment_log2,
            target_address_size: bntx.target_address_size,
            flag: bntx.flag,
            name: bntx.name.clone(),
            platform: String::from_utf8_lossy(&bntx.platform.magic()).into_owned(),
            textures: bntx
                .textures
                .iter()
                .zip(stems)
                .map(|(t, stem)| TexManifest {
                    name: t.name.clone(),
                    format: format!("{:#x}", t.info.format.raw()),
                    width: t.info.width,
                    height: t.info.height,
                    depth: t.info.depth,
                    mip_count: t.info.mip_count,
                    array_count: t.info.array_count,
                    sample_count: t.info.sample_count,
                    gpu_access: t.info.gpu_access,
                    flags: t.info.flags,
                    dim: t.info.dim,
                    surface_dim: t.info.surface_dim,
                    tile_mode: t.info.tile_mode,
                    swizzle: t.info.swizzle,
                    alignment: t.info.alignment,
                    texture_layout: t.info.texture_layout,
                    texture_layout2: t.info.texture_layout2,
                    channels: [
                        t.info.channel_r,
                        t.info.channel_g,
                        t.info.channel_b,
                        t.info.channel_a,
                    ],
                    reserved: crate::hex::encode(&t.info.reserved),
                    mip_offsets: t.mip_offsets.clone(),
                    data: format!("{stem}.bin"),
                })
                .collect(),
        }
    }

    fn into_bntx(self, dir: &Path) -> Result<Bntx> {
        let byte_order = match self.byte_order.as_str() {
            "little" => ByteOrder::Little,
            "big" => ByteOrder::Big,
            other => bail!("unknown byte_order `{other}`"),
        };
        let mut magic = [b' '; 4];
        for (slot, byte) in magic.iter_mut().zip(self.platform.bytes()) {
            *slot = byte;
        }
        let mut textures = Vec::with_capacity(self.textures.len());
        for t in self.textures {
            let image_data = std::fs::read(dir.join(&t.data))
                .with_context(|| format!("read `{}/{}`", dir.display(), t.data))?;
            let reserved = crate::hex::decode_fixed::<20>(&t.reserved)
                .with_context(|| format!("texture `{}` reserved field", t.name))?;
            let info = TextureInfo {
                flags: t.flags,
                dim: t.dim,
                tile_mode: t.tile_mode,
                swizzle: t.swizzle,
                mip_count: t.mip_count,
                sample_count: t.sample_count,
                format: ImageFormat::from_raw(parse_hex_u32(&t.format)?),
                gpu_access: t.gpu_access,
                width: t.width,
                height: t.height,
                depth: t.depth,
                array_count: t.array_count,
                texture_layout: t.texture_layout,
                texture_layout2: t.texture_layout2,
                reserved,
                image_size: u32::try_from(image_data.len()).unwrap_or(0),
                alignment: t.alignment,
                channel_r: t.channels[0],
                channel_g: t.channels[1],
                channel_b: t.channels[2],
                channel_a: t.channels[3],
                surface_dim: t.surface_dim,
            };
            textures.push(Texture {
                name: t.name,
                info,
                mip_offsets: t.mip_offsets,
                user_data: Vec::new(),
                image_data,
            });
        }
        Ok(Bntx {
            byte_order,
            version: (
                self.version[0],
                u8::try_from(self.version[1]).unwrap_or(0),
                u8::try_from(self.version[2]).unwrap_or(0),
            ),
            alignment_log2: self.alignment_log2,
            target_address_size: self.target_address_size,
            flag: self.flag,
            block_offset: 0,
            name: self.name,
            platform: Platform::from_magic(magic),
            textures,
        })
    }
}

pub(crate) fn convert_to_bundle(bytes: &[u8], out_dir: &Path, preview: bool) -> Result<u64> {
    let bntx = Bntx::parse(bytes)?;
    write_bundle(&bntx, out_dir, preview)?;
    let mut total = 0u64;
    if let Ok(rd) = std::fs::read_dir(out_dir) {
        for e in rd.flatten() {
            total += e.metadata().map_or(0, |m| m.len());
        }
    }
    Ok(total)
}

fn unique_stems(textures: &[Texture]) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    textures
        .iter()
        .map(|tex| {
            let base = sanitize_name(&tex.name);
            let mut name = base.clone();
            let mut i = 1u32;
            while seen.contains(&name) {
                name = format!("{base}_{i}");
                i += 1;
            }
            seen.insert(name.clone());
            name
        })
        .collect()
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn strip_d(dir: &Path) -> PathBuf {
    if dir.extension().is_some_and(|e| e == "d") {
        dir.with_extension("")
    } else {
        append_ext(dir, "bntx")
    }
}

fn parse_hex_u32(s: &str) -> Result<u32> {
    let body = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u32::from_str_radix(body, 16).with_context(|| format!("`{s}` is not a hex u32"))
}
