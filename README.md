# tomo

Tooling for **Tomodachi Life: Living the Dream** game data: inspect, extract, convert, and repack romfs files.

The workspace is two crates:

| Crate     | Kind    | Purpose                                                   |
| --------- | ------- | --------------------------------------------------------- |
| `tomolib` | library | Parsers, encoders, and data types for the game formats.   |
| `tomocli` | binary  | `tomo`, a command-line front-end built on top of the lib. |

## Install

### Prebuilt binary

Grab the archive for your platform from the [latest release](https://github.com/alexislours/tomo/releases/latest). Builds are published for Linux (x86_64, aarch64), macOS (Apple Silicon and Intel), and Windows (x86_64).

### From crates.io

```sh
cargo install tomocli
tomo --help
```

### From source

```sh
cargo install --git https://github.com/alexislours/tomo tomocli
```

Building from source needs a C/C++ toolchain (for `zstd` and the bundled texture encoders).

## Usage

Commands follow a `tomo <format> <verb>` pattern. Most formats support the same three verbs:

| Verb      | What it does                                                  |
| --------- | ------------------------------------------------------------- |
| `info`    | Print a human-readable summary. Read-only.                    |
| `extract` | Decompose a file into an editable form (a directory or YAML). |
| `pack`    | Rebuild the original format from an `extract` output.         |

```sh
tomo sarc info  some.byml
tomo sarc extract some.byml          # -> some.byml.yml
tomo sarc pack some.byml.yml --out some.byml
```

`extract`/`pack` round-trips try to be lossless where possible

## Supported formats

| Format                                       | `info` | `extract` | `pack` |
| -------------------------------------------- | :----: | :-------: | :----: |
| `.zs`                                        |   ✓    |     ✓     |   ✓    |
| `.sarc` `.pack` `.blarc` `.bfarc` `.baatarc` |   ✓    |     ✓     |   ✓    |
| `.byml` `.bgyml`                             |   ✓    |     ✓     |   ✓    |
| `.rsizetable`                                |   ✓    |     ✓     |   ✓    |
| `.msbt`                                      |   ✓    |     ✓     |   ✓    |
| `.msbp`                                      |   ✓    |     ✓     |   ✓    |
| `.bntx`                                      |   ✓    |     ✓     |   ✓    |
| `.bwav`                                      |   ✓    |     ✓     |   ✓    |
| `.bars`                                      |   ✓    |     ✓     |   ✓    |
| `.ainb`                                      |   ✓    |     ✓     |   ✓    |
| `.nca`                                       |   ✓    |     ✓     |   ✗    |
| `.nsp`                                       |   ✓    |     ✓     |   ✗    |

A `romfs extract` command recursively unpacks a whole directory tree.

## Build

```sh
cargo build --workspace
cargo run -p tomocli -- --help
cargo test --workspace
```

## Acknowledgements

AINB format research and reference: [dt-12345/AINB](https://github.com/dt-12345/AINB).

## License

[GPL-3.0-or-later](./LICENSE).

> **Disclaimer:** This is an unofficial, fan-made project and is not affiliated with, endorsed by, or sponsored by Nintendo. It ships no game assets. You must supply files dumped from your own legally owned copy of the game. All trademarks are the property of their respective owners.
