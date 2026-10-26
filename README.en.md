# Sift

[简体中文](README.md) · English

[![Platform](https://img.shields.io/badge/platform-macOS-000000?logo=apple&logoColor=white)](#requirements)
[![Version](https://img.shields.io/badge/version-0.1.0-4D6BFE)](CHANGELOG.md)
[![Rust](https://img.shields.io/badge/Rust-stable-CE422B?logo=rust&logoColor=white)](Cargo.toml)
[![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white)](package.json)

Sift is a macOS disk-space manager. It scans the disk, finds what can be cleaned, removes it safely, and turns the conclusions into reusable scheduled tasks. Deletions always go to the Trash, and the model can only corroborate a verdict — never authorize one.

## Features

- **Scan** a whole disk or a single directory with live progress and cancel.
- **Explore** a size-aware tree and a smart list with a "selected only" filter.
- **Clean** only into the Trash, with per-item results, cancellation and history.
- **Decide with AI** using your own provider, or stay fully local.
- **Schedule** recurring cleanup as routines or directory + file-name regex rules.
- **Watch** free space from the menu bar with low-space warnings.

## Requirements

- macOS 13 or later (Apple Silicon / Intel)
- Full Disk Access for the first scan
- Network access and an API key only if you enable AI features

## Install

Build from source (requires Rust, Node.js and pnpm):

```bash
pnpm install
pnpm tauri build      # release bundle in src-tauri/target/release/bundle
```

For development:

```bash
pnpm tauri dev
```

## Configuration

AI is optional. Open **Settings → AI**, add a profile and select it:

| Provider | Endpoint |
| --- | --- |
| Volcengine ARK | `/api/v3` (platform key) or `/api/plan/v3` (Agent Plan key) |
| DeepSeek | `https://api.deepseek.com/chat/completions` |
| OpenAI | `https://api.openai.com/v1/chat/completions` |
| Anthropic Claude | `https://api.anthropic.com/v1/messages` |

Keys are stored locally in `~/Library/Application Support/Sift/` (mode 0600) and are never written to settings or the repository.

## Documentation

- [Product description (English)](docs/product/description.en.md) · [软件说明（中文）](docs/product/description.zh.md)
- [Release notes v0.1.0](docs/release/v0.1.0/release-notes.en.md) · [中文](docs/release/v0.1.0/release-notes.zh.md)
- [Changelog](CHANGELOG.md) · [Architecture](docs/architecture)

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets
pnpm check
```

## Contributing

Issues and pull requests are welcome. Please keep changes focused, include a short description of the problem and how you verified the fix, and run the checks above before opening a PR.
