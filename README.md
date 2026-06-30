# Project Modman

> A Rust modding utility for Project Wingman (PW) — inspired by Project Sicario.

## Status

**Pre-release (0.1.0).** This is a scaffold — the CLI exists but commands are not yet implemented.

## Goals

- **v1.0:** Drop-in CLI replacement for Project Sicario — read `.dtm` patches, merge mods, apply uasset patches, pack `.pak` files. No .NET dependency.
- **v2.0 (future):** Broader PW modding toolkit.

## Building

Requires Rust 2021 edition or later.

```bash
# Build all crates
cargo build --workspace

# Build the CLI only
cargo build -p modman

# Run tests
cargo test
```

## Usage

```bash
modman --help
modman --version
```

## Project Structure

```
crates/
├── modman-core/       Data models, patch engine, mod merging
├── modman-uasset/     UE4 .uasset binary parser (standalone)
├── modman-pak/        PAK file operations (wraps repak)
└── modman-cli/        CLI binary
```

## License

MIT
