# Project Modman

> A Rust modding utility for Project Wingman (PW) — inspired by Project Sicario.

## Status

**Pre-1.0, in development.** Working today: PAK I/O (`info`, `list`, `unpack`, `pack`), `.dtm`/`.dtp` patch parsing, Fragment DSL parsing, patch-type parsing, template substitution, and a **verified DataTable engine** (`modman-uasset::walk` + `edit` — byte-exact walker with byte ranges, plus same-size splice editing; fixture-gated against real PW assets). In progress: patch application + merge (the Sicario-parity A-layer). **Not yet usable for real mods** — `build` deliberately writes no output until patch application lands.

## V1 Scope — 1:1 parity with the Project Sicario merger

**The v1 goal is 1:1 parity with the Sicario merger — no more, no less.**

A drop-in replacement for the Sicario merger (`SicarioPatch.Loader`) against Project Wingman 2.1.1A / UE 4.27 / Pak V11:

- **Inputs:** identical `.dtm`/`.dtp` WingmanMod JSON, with `_vars`/`_inputs` templating.
- **Patch semantics:** every Sicario fragment type, every Sicario patch type, identical matching and value behavior.
- **Merge semantics:** identical multi-mod merging (conflicts/dedup) to Sicario's engine.
- **Output:** a Pak V11 mod pack (mount `../../../`) for the game's `~mods/` directory.
- **Zero .NET dependency** — single native binary.

Nothing more (no new formats, no new behaviors, no UI), nothing less.

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
