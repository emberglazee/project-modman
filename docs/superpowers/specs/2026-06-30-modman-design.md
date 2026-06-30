# ModMan — Project Wingman Modding Utility

**Date:** 2026-06-30
**Status:** Approved design, pre-implementation
**Version:** 0.1.0 (initial scaffold)

## Overview

ModMan is a Rust modding utility for Project Wingman (PW), designed as a drop-in CLI replacement for Project Sicario (C# .NET) with the long-term goal of becoming a broader PW modding toolkit.

- **v1.0 target:** Drop-in CLI replacement for Sicario's CLI — reads `.dtm` patch files, merges mods, applies uasset patches, packs result into `.pak` files
- **v2.0 (future):** Broader PW modding toolkit with additional capabilities beyond Sicario's scope
- **Key advantages over Sicario:** Single native binary, no .NET runtime dependency, cross-platform (Linux/WSL, macOS, Windows), smaller footprint

## Workspace & Crate Layout

The project uses a Cargo workspace with four crates, each independently testable:

```
project-modman/
├── Cargo.toml                      # workspace root
├── .github/workflows/
│   ├── build.yml                   # CI: build on push to main
│   └── release.yml                 # Release on workflow_dispatch
├── crates/
│   ├── modman-core/                # Data models, JSON format, patch engine, mod merging
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── manifest/           # WingmanMod, SicarioMod JSON structs (.dtm format)
│   │       ├── patch/              # Patch types (propertyValue, modifyPropertyValue, etc.)
│   │       ├── fragment/           # Fragment DSL — navigates uasset structure
│   │       ├── merge/              # Mod merging logic
│   │       └── template/           # Patch templating (variable substitution)
│   │
│   ├── modman-uasset/              # UE4 .uasset/.uexp binary parser (standalone crate)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── reader/             # Binary deserialization
│   │       ├── writer/             # Binary serialization
│   │       ├── types/              # Property types (Int, Float, Str, Struct, Array, Byte, etc.)
│   │       ├── name_table/         # FName table handling
│   │       └── export_map/         # Import/export maps
│   │
│   ├── modman-pak/                 # PAK operations (wraps repak as PW-convenient API)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── reader/             # Read/list/extract
│   │       └── writer/             # Pack/create
│   │
│   └── modman-cli/                 # CLI binary
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs
│           ├── commands/
│           │   ├── build.rs        # Main build command (Sicario-compatible)
│           │   ├── info.rs         # PAK metadata
│           │   ├── pack.rs         # Create PAK from directory
│           │   └── unpack.rs       # Extract PAK to directory
│           └── game/               # Game install detection, config
│
├── tests/                          # Integration tests
└── docs/superpowers/specs/         # Design documents
```

### Dependency graph

```
modman-cli
  ├── modman-core
  │     └── modman-uasset    (patch engine applies modifications through uasset parser)
  └── modman-pak
        └── repak             (externally managed)
```

`modman-uasset` is fully standalone — no dependencies on other modman crates — so it can be published independently if desired.

## Data Flow & Pipeline Architecture

### High-level pipeline

```
.dtm files ──→ [WingmanMod Parser] ──→ Patch definitions
              (modman-core/manifest)         │
                                            ▼
Game .pak ──→ [modman-pak] ──→ Extracted .uasset files
              (via repak)                    │
                                            ▼
                                   [Patch Engine]
                                   modman-core + modman-uasset
                                   fragments → match → patch
                                            │
                                            ▼
                                 [modman-pak] ──→ Output .pak
```

### Sicario JSON format (`.dtm` files)

Inherited from Sicario for compatibility. A `WingmanMod` has:

```json
{
  "_id": "my-mod-id",
  "_sicario": {
    "private": false,
    "overwrites": false,
    "group": "weapons",
    "preview": false
  },
  "_vars": { "damage_mult": "1.5" },
  "_inputs": [
    { "key": "damage_mult", "default": "1.5", "type": "float" }
  ],
  "assetPatches": {
    "Game/Data/Weapons/WeaponTable.uasset": [
      {
        "template": "DataTable(\"WP_Damage\") > PropertyValue(\"FloatProperty\":*):*",
        "patches": [
          { "type": "modifyPropertyValue", "value": "*:*2.0" }
        ]
      }
    ]
  }
}
```

Key fields:
- `_id` — unique mod identifier
- `_sicario` — metadata (privacy, group, stability flags)
- `_vars` — template variables (substituted into patch values)
- `_inputs` — user-facing parameters with defaults
- `assetPatches` — map of target uasset files → patch sets
- `filePatches` — alternative file-level patches (less common)

### Fragment DSL

The `template` string is a parsing expression that navigates into the uasset structure:

```
DataTable("TableName") > PropertyValue("FloatProperty":*):*
```

Parsed as a chain of **fragments**, each filtering/matching the current data set:
- `DataTable(name)` — select a named data table
- `PropertyValue(type:value)` — match properties by type and value
- `Struct(name)` — descend into a struct property
- `Array(index)` — select array element by index
- `Flatten()` — flatten nested structures

### Patch types (from Sicario)

Implemented incrementally — all 9 Sicario-compatible types for v1.0:

| Type | Operation | Example value |
|------|-----------|---------------|
| `propertyValue` | Set a property to a value | `IntProperty:42` |
| `modifyPropertyValue` | Arithmetic modification | `FloatProperty:*2.0` |
| `arrayPropertyValue` | Modify array element values | `0:IntProperty:10` |
| `duplicateProperty` | Duplicate a property within a struct | `"source" > "target"` |
| `duplicateEntry` | Duplicate table entries | `"SourceName":0 > "TargetName":0` |
| `duplicateArrayItem` | Duplicate array items by index | `0>1` |
| `deleteEntry` | Delete matching entries | `EntryName` |
| `objectRef` | Set object references | `Package.Name` |
| `textProperty` | Modify text/string properties | `ns:key=value` |

### Templating

Simple variable substitution (`{{ var_name }}`) in patch values, using user-provided inputs or `_vars` defaults. A lightweight custom template engine avoids heavy dependencies like Tera/Handlebars for the relatively simple substitution patterns Sicario uses.

### modman-uasset parser scope

Must handle:
- UE4 uasset file header (package, version, flags)
- Name table (FName entries)
- Import map (package imports)
- Export map (object exports)
- Property serialization for common types: IntProperty, FloatProperty, BoolProperty, StrProperty, TextProperty, NameProperty, ByteProperty, StructProperty, ArrayProperty, ObjectProperty
- DataTable-specific serialization (rows as structs)
- .uexp companion files for exports exceeding the uasset inline limit

UE4.24 (v1.0.4d) and UE4.27 (v2.1.1A) formats — both use the same uasset serialization format at the property level.

## Development Roadmap (0.x releases)

Each minor version delivers a complete, testable vertical slice.

| Version | Scope | Deliverable |
|---------|-------|-------------|
| **0.1.0** | Scaffolding | Workspace with 4 crates, CI/CD workflows, CLI skeleton (`--help`), README, LICENSE |
| **0.2.0** | `modman-pak` read | `modman info`, `modman list` working against real PW pak files |
| **0.3.0** | `modman-pak` unpack | `modman unpack` extracts files from paks |
| **0.4.0** | `modman-pak` pack | `modman pack` creates paks from directories |
| **0.5.0** | `modman-core` data model | WingmanMod JSON parse + serialize |
| **0.6.0** | `modman-uasset` header/names | Parse uasset structure metadata |
| **0.7.0** | `modman-uasset` properties | Read/write basic property types |
| **0.8.0** | Fragment DSL | Template → fragment chain parsing |
| **0.9.0** | Basic patches | `propertyValue`, `modifyPropertyValue` working end-to-end |
| **0.10.0** | All patch types | All 9 Sicario patch types |
| **0.11.0** | Game detection + build command | `modman build` — drop-in Sicario CLI replacement |
| **0.12.0** | Templating + `_inputs` | User parameter prompts, template substitution |
| **1.0.0** | Release | Docs, real PW mod testing, polish |

## CI/CD

### Build workflow (`build.yml`)

```yaml
on: push to main, pull_request to main
jobs:
  check:
    runs-on: ubuntu-latest
    - cargo fmt --check
    - cargo clippy --deny warnings
    - cargo test

  build:
    needs: check
    strategy:
      matrix:
        - os: ubuntu-latest, target: x86_64-unknown-linux-gnu
        - os: windows-latest, target: x86_64-pc-windows-msvc
        - os: macos-latest, target: aarch64-apple-darwin
    - cargo build --release --target ${{ matrix.target }}

  test-cross:
    needs: check
    strategy:
      matrix: [ubuntu-latest, windows-latest, macos-latest]
    - cargo test
```

### Release workflow (`release.yml`)

```yaml
on: workflow_dispatch with input: bump (patch/minor/major/none)
- bump version in Cargo.toml
- build all 3 platform targets (via reusable workflow)
- create GitHub release with attached binaries
```

Key tooling:
- `dtolnay/rust-toolchain@stable` — Rust setup
- `Swatinem/rust-cache@v2` — dependency caching
- `softprops/action-gh-release@v3` — GitHub releases

## Key Design Decisions

1. **Separate `modman-uasset` crate** — the hardest technical challenge gets its own crate with zero internal dependencies. Can be tested in isolation with real PW uasset files.
2. **modman-pak wraps repak** — rather than forking or re-exporting, provide a PW-convenient API on top of repak's generic PAK reader/writer. This insulates the rest of the codebase from repak API changes.
3. **Incremental 0.x releases** — each release is a working vertical slice. No long-running branches. This gives us a shippable artifact at every step.
4. **CI from day one** — 0.1.0 lands with cross-platform build + test. Catches platform issues before they compound.
5. **Inherit Sicario's `.dtm` JSON format** — ensures compatibility with existing mods. No need to reinvent the metadata format.
