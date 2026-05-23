use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod fmt;
mod hex;
mod paths;

#[derive(Debug, Parser)]
#[command(
    name = "tomo",
    version,
    about = "Work with Tomodachi Life data formats",
    long_about = "Work with Tomodachi Life data formats: inspect, extract, and mod \
                  save files, Miis, textures, and other game data.\n\n\
                  Subcommands are organised by format (`tomo <format> <verb>`) \
                  and follow a `info` / `extract` / `pack` convention.",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Work with `.ainb` (AI node graph) files.
    Ainb(commands::ainb::AinbArgs),
    /// Work with `.bars` (audio resource) archives.
    Bars(commands::bars::BarsArgs),
    /// Work with `.bntx` (Switch texture) files.
    Bntx(commands::bntx::BntxArgs),
    /// Work with `.bwav` (binary waveform) files.
    Bwav(commands::bwav::BwavArgs),
    /// Work with `.byml` / `.bgyml` files.
    Byml(commands::byml::BymlArgs),
    /// Work with `.msbt` (`LibMessageStudio`) message files.
    Msbt(commands::msbt::MsbtArgs),
    /// Work with `.msbp` (`LibMessageStudio`) project files.
    Msbp(commands::msbp::MsbpArgs),
    /// Work with `.nca` (Nintendo Content Archive) files.
    Nca(commands::nca::NcaArgs),
    /// Work with `.nsp` (Nintendo Submission Package) files.
    Nsp(commands::nsp::NspArgs),
    /// Recursively unpack a directory.
    Romfs(commands::romfs::RomfsArgs),
    /// Work with `.rsizetable` (RESTBL) resource size tables.
    Rstbl(commands::rstbl::RstblArgs),
    /// Work with `.sarc` (sead archive) files.
    Sarc(commands::sarc::SarcArgs),
    /// Work with `.zs` (zstd-compressed) files.
    Zs(commands::zs::ZsArgs),
}

fn main() -> Result<()> {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let want_color = !no_color && std::io::stdout().is_terminal();
    owo_colors::set_override(want_color);

    let cli = Cli::parse();
    match cli.command {
        Command::Ainb(args) => commands::ainb::run(args),
        Command::Bars(args) => commands::bars::run(args),
        Command::Bntx(args) => commands::bntx::run(args),
        Command::Bwav(args) => commands::bwav::run(args),
        Command::Byml(args) => commands::byml::run(args),
        Command::Msbt(args) => commands::msbt::run(args),
        Command::Msbp(args) => commands::msbp::run(args),
        Command::Nca(args) => commands::nca::run(args),
        Command::Nsp(args) => commands::nsp::run(args),
        Command::Romfs(args) => commands::romfs::run(args),
        Command::Rstbl(args) => commands::rstbl::run(args),
        Command::Sarc(args) => commands::sarc::run(args),
        Command::Zs(args) => commands::zs::run(args),
    }
}
