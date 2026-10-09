# Project Modman

> A Rust modding utility for Project Wingman (PW) — inspired by Project Sicario.

## Status

**v1 (Sicario-merger parity) is essentially complete and byte-verified against the C# merger.** Working today:

- PAK I/O (`info`, `list`, `unpack`, `pack`, `scan`)
- `.dtm`/`.dtp` patch parsing, Fragment DSL, every Sicario patch type
- Template rendering (Fluid-compatible: `vars`/`inputs` + the full Sicario filter set)
- **Verified DataTable engine** (byte-exact walker + same-size splice + length-changing rowops)
- **HexPatch engine** (`filePatches`) with the C# stream-walk semantics and the `.uexp` length auto-correct
- **Merge pipeline**: component model (embeddedPresets → loosePresets → sicarioRequests), engine-major phases, `.uexp`/`.uasset` sidecar handling
- **`build`** writes a real merged `SicarioMerge_P.pak`; **`preset-pack`** builds standalone preset packs with the preset embedded at `Content/sicario/`; **`--report`** writes the C#-identical merge report

In-game acceptance passed (SPEAR Unlock + Improved Chimera verified in Project Wingman 2.1.1A).

## V1 Scope — 1:1 parity with the Project Sicario merger

**The v1 goal is 1:1 parity with the Sicario merger — no more, no less.**

A drop-in replacement for the Sicario merger (`SicarioPatch.Loader`) against Project Wingman 2.1.1A / UE 4.27 / Pak V11:

- **Inputs:** identical `.dtm`/`.dtp` WingmanMod JSON, with `_vars`/`_inputs` templating.
- **Patch semantics:** every Sicario fragment type, every Sicario patch type, identical matching and value behavior.
- **Merge semantics:** identical multi-mod merging to Sicario's engine.
- **Output:** a Pak V3 mod pack (mount `../../../`) for the game's `~mods/` directory — same as the merger.
- **Zero .NET dependency** — single native binary.

## Parity status

Verified against the C# merger at byte level (oracle harness in `~/modding/project-wingman/sicario-oracle`):

| Surface | Status |
|---|---|
| DataTable patches (propertyValue, modify, array, text, duplicate*, delete) | ✅ byte-exact (uassets identical; uexps modulo random FText keys) |
| `filePatches` hex engine (all types, windows, filters, length fix-up) | ✅ byte-exact, including the destructive absent-`value` path |
| Multi-mod merge order + conflict semantics | ✅ oracle-verified |
| Components, parameters/inputs, engine-version gate, `GetLabel` | ✅ |
| Merge report (`--report`) | ✅ byte-identical |
| `preset-pack` | ✅ byte-identical output pak |
| In-game acceptance | ✅ (user-verified) |

### Known gap

- **`customSkins` (legacy PSM skin-slot merging) and `objectRef` application.** The loader synthesizes an `objectRef` mod from installed `*_P.pak` skins (see `SkinSlotLoader`): it appends object references to `SkinLibraryLegacy` arrays and adds new **import entries** (FObjectImport) plus name-table additions to `DB_Aircraft.uasset`. Our `objectRef` patch type parses but is not yet applied; implementing it requires UE4 import-table writing (the same machinery a fuller UAssetAPI writer port would provide). Modern PNG-pipeline skins (2.x) do not need this merge.

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

# Merge mods/presets into a mod pak (like `ProjectSicario build`)
modman build <paths...> --install-path <game> --output <dir> --report merge-report.json

# Build standalone preset packs (preset embedded at Content/sicario/)
modman preset-pack <preset.dtp...> -n MyPack --install-path <game>
```

## Project Structure

```
crates/
├── modman-core/       Data models, patch engine, merge pipeline, components
├── modman-uasset/     UE4 .uasset binary parser (standalone)
├── modman-pak/        PAK file operations (wraps repak)
└── modman-cli/        CLI binary
```

## License

MIT
