# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Himalaya is a CLI to manage emails, written in Rust. It wraps the `email-lib` library and related Pimalaya ecosystem crates (`pimalaya-tui`, `secret-lib`, `mml-lib`) into a stateless command-line interface. Configuration is TOML-based (see `config.sample.toml`).

## Build Commands

```bash
cargo build                    # Debug build with default features
cargo build --release          # Release build
cargo test --lib               # Run unit tests (inline tests only, no integration test suite)
cargo check                    # Type-check without building
```

Selective feature builds:
```bash
cargo build --no-default-features --features imap,smtp,keyring --release
```

Default features: `imap`, `maildir`, `smtp`, `sendmail`, `wizard`, `pgp-commands`

All features (for dev): `notmuch`, `keyring`, `oauth2`, `pgp-gpg`, `pgp-native` (added via `[dev-dependencies]`)

## Rust Toolchain

Pinned to Rust **1.82** via `rust-toolchain.toml`. Nix shell (`nix-shell`) provides a reproducible dev environment.

## Architecture

### Command Pattern

Every CLI subcommand follows the same structure:

1. **Argument struct** — `clap::Parser` derive struct in `src/<domain>/command/<verb>.rs`
2. **`execute()` method** — async method taking `&mut impl Printer` and `&TomlConfig`, performs the operation
3. **Subcommand enum** — in `src/<domain>/command/mod.rs`, dispatches to individual command structs

Domains: `account`, `folder`, `email/envelope`, `email/envelope/flag`, `email/message`, `email/message/attachment`, `email/message/template`, `completion`, `manual`

### Module Layout (per domain)

```
src/<domain>/
├── arg/           # Clap argument structs (reusable across commands)
├── command/       # Subcommand implementations (each verb is a file)
│   └── mod.rs     # Subcommand enum with dispatch
└── config.rs      # Domain-specific config types (if any)
```

### Key Delegation Pattern

Most logic lives in external crates, not in this repo:
- **`email-lib`** — all email backend operations (IMAP, Maildir, Notmuch, SMTP, Sendmail)
- **`pimalaya-tui`** — `BackendBuilder`, `TomlConfig` (HimalayaTomlConfig), table formatting, printer, tracing setup
- **`secret-lib`** — secret/credential management
- **`mml-lib`** — MIME Meta Language compilation/interpretation

This CLI is primarily a **thin command layer** that parses arguments, loads config, builds a backend via `BackendBuilder`, calls traits from `email-lib`, and prints results via the `Printer` trait.

### Entry Point (`src/main.rs`)

- Async tokio runtime
- Special-cases `mailto:` URLs as first argument
- No subcommand defaults to `envelope list`

### Config System

`src/config.rs` is a type alias: `TomlConfig = pimalaya_tui::himalaya::config::HimalayaTomlConfig`. All config logic lives in `pimalaya-tui`. Config files load from `~/.config/himalaya/config.toml` by default, overridable with `-c`/`--config`.

## Commit Style

Conventional commits: `type(scope): message` (e.g., `feat(folder):`, `fix(message):`, `build:`, `docs:`)

## Overriding Local Dependencies

For development against local Pimalaya crates, add to `Cargo.toml`:
```toml
[patch.crates-io]
email-lib = { path = "/path/to/email-lib" }
```
See `CONTRIBUTING.md` for the full list of overridable crates when version conflicts occur.
