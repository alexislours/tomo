use std::io::IsTerminal;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

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
    /// When to use colored output.
    #[arg(long, value_enum, default_value_t = ColorWhen::Auto, global = true)]
    color: ColorWhen,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
enum ColorWhen {
    #[default]
    Auto,
    Always,
    Never,
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
    /// Generate a shell completion script.
    Completions {
        /// Shell to generate completions for.
        shell: Shell,
    },
}

fn want_color(when: ColorWhen) -> bool {
    match when {
        ColorWhen::Always => true,
        ColorWhen::Never => false,
        ColorWhen::Auto => {
            let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
            !no_color && std::io::stdout().is_terminal()
        }
    }
}

fn main() -> Result<()> {
    let cmd = Cli::command();
    let early = cmd
        .clone()
        .ignore_errors(true)
        .try_get_matches()
        .ok()
        .and_then(|m| m.get_one::<ColorWhen>("color").copied())
        .unwrap_or_default();
    let color_choice = if want_color(early) {
        clap::ColorChoice::Always
    } else {
        clap::ColorChoice::Never
    };
    let cli = cmd
        .color(color_choice)
        .try_get_matches()
        .and_then(|m| <Cli as clap::FromArgMatches>::from_arg_matches(&m))
        .unwrap_or_else(|e| e.exit());
    owo_colors::set_override(want_color(cli.color));

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
        Command::Completions { shell } => {
            let mut cmd = Cli::command();
            let bin = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, bin, &mut std::io::stdout());
            Ok(())
        }
    }
}
