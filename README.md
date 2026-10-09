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
- **`combine`** merges conflicting override mods that carry **no Sicario metadata** (the common case for 2024+ mods) — three-way, field-level, with a full conflict report

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
| `objectRef` (import-table writes) + `customSkins` PSM slot merging | ✅ byte-exact + **in-game verified** (AJS-37 skin slot live) |
| Multi-mod merge order + conflict semantics | ✅ oracle-verified |
| Components, parameters/inputs, engine-version gate, `GetLabel` | ✅ |
| Merge report (`--report`) | ✅ byte-identical |
| `preset-pack` | ✅ byte-identical output pak |
| In-game acceptance | ✅ (user-verified) |

**All known v1 parity gaps are closed.** `customSkins` skin-slot merging works with real
PSM skin paks (`ProjectWingman/Content/Assets/Skins/<aircraft-row>/…`), live-verified in-game.

### Beyond parity

- **Self-contained merges** (default): the merged pak embeds detected skin files, so one pak
  installs everything (`--no-embed-skins` restores the strict C# output).
- **No-metadata merging** (`combine`): three-way, field-level datatable merging for plain
  override mods, with import carrying and conflict reports.
- **End-of-merge reports** for both `build` and `combine`: merge order, field-level
  conflicts (which mod overwrote which value), and warnings.

## Merging mods without metadata (`combine`)

Project Sicario's merge metadata (`.dtp` presets, embedded requests) was never updated for
Project Wingman 2.0 / UE 4.27, so most 2024+ mods are plain file overrides — and when two
of them edit the same uasset, the game loads only one. `modman combine` merges them anyway:

```bash
modman combine <pak-or-dir...> --install-path <game> --output <dir>
```

- DataTables are merged **three-way** (vanilla + each mod's delta, later mods win) at
  **field level** — different fields of the same row all survive; genuine same-field
  clashes resolve later-wins and are reported.
- Mods that add object references (textures, materials) keep working: import-table
  entries are carried with all FName and package-index references remapped.
- Non-datatable files (maps, models, textures) pass through single-winner, with a warning
  when two mods fight over one — those file types genuinely cannot be combined.
- Every run ends with a **merge report**: the order, every conflict (with mod names and
  values), and warnings.

### Web UI — no install needed

**<https://emberglazee.github.io/project-modman/>** — the same merge engine, compiled to
WebAssembly and running entirely in your browser:

- Drop your conflicting `*_P.pak` files, point at your game's
  `pakchunk0-WindowsNoEditor.pak`, hit merge, download the combined pak.
- **Nothing is uploaded** — the game pak is read in place (a few MB of it, even from a
  ~16 GB file) via the browser's File API; the page makes zero network requests after
  loading.
- Shows the same merge report (order + conflicts) with a friendly UI.

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

# Merge conflicting override mods — no metadata required
modman combine <pak-or-dir...> --install-path <game> --output <dir>
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
