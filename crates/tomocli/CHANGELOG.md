# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.7.0](https://github.com/alexislours/tomo/compare/tomocli-v1.6.0...tomocli-v1.7.0) - 2026-06-20

### Added

- resolve BNTX channel swizzle on decode

## [1.6.0](https://github.com/alexislours/tomo/compare/tomocli-v1.5.1...tomocli-v1.6.0) - 2026-06-09

### Added

- add NSP RomFs extraction with BKTR update merging

## [1.5.1](https://github.com/alexislours/tomo/compare/tomocli-v1.5.0...tomocli-v1.5.1) - 2026-06-07

### Fixed

- emit BNTX relocation entry for header data pointer

## [1.5.0](https://github.com/alexislours/tomo/compare/tomocli-v1.4.0...tomocli-v1.5.0) - 2026-05-30

### Added

- add BFRES format support

## [1.4.0](https://github.com/alexislours/tomo/compare/tomocli-v1.3.0...tomocli-v1.4.0) - 2026-05-23

### Added

- add rstbl patch subcommand
- add AMTA format support
- add BNVIB format support

## [1.3.0](https://github.com/alexislours/tomo/compare/tomocli-v1.2.0...tomocli-v1.3.0) - 2026-05-23

### Added

- refuse to overwrite existing output unless --force

### Fixed

- apply --color to clap's own help and error output

### Other

- unify extract/pack success messages
