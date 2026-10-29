# Project Modman

> A Rust modding utility for Project Wingman (PW), inspired by Project Sicario.

## Contents

- [Status](#status)
- [V1 Scope — 1:1 parity with the Project Sicario merger](#v1-scope--11-parity-with-the-project-sicario-merger)
- [Parity status](#parity-status)
  - [Beyond parity](#beyond-parity)
- [Merging mods without metadata (`combine`)](#merging-mods-without-metadata-combine)
  - [Web UI — no install needed](#web-ui--no-install-needed)
  - [Desktop app — no terminal needed](#desktop-app--no-terminal-needed)
  - [Choosing the merge order](#choosing-the-merge-order)
  - [Downloading & first run](#downloading--first-run)
- [Translation mods (`.locres`)](#translation-mods-locres)
- [Game audio (`.ogg`)](#game-audio-ogg)
- [Documentation](#documentation)
- [Building](#building)
- [Usage](#usage)
- [Project Structure](#project-structure)
- [License](#license)

## Status

**v1 (Sicario-merger parity) is essentially complete and byte-verified against the C# merger.** Working today:

- PAK I/O (`info`, `list`, `unpack`, `pack`, `scan`), including compressed entries
  (Zlib, Gzip, Zstd, LZ4); `pack --compression` writes them.
- `.dtm`/`.dtp` patch parsing, Fragment DSL, every Sicario patch type
- Template rendering (Fluid-compatible: `vars`/`inputs` + the full Sicario filter set)
- **Verified DataTable engine** (byte-exact walker + same-size splice + length-changing rowops)
- **HexPatch engine** (`filePatches`) with the C# stream-walk semantics and the `.uexp` length auto-correct
- **Merge pipeline**: component model (embeddedPresets → loosePresets → sicarioRequests), engine-major phases, `.uexp`/`.uasset` sidecar handling
- **`build`** writes a real merged `SicarioMerge_P.pak`; **`preset-pack`** builds standalone preset packs with the preset embedded at `Content/sicario/`; **`--report`** writes the C#-identical merge report
- **`combine`** merges conflicting override mods that carry **no Sicario metadata** (the common case for 2024+ mods), three-way and field-level, with a full conflict report

In-game acceptance passed (SPEAR Unlock + Improved Chimera verified in Project Wingman 2.1.1A).

## V1 Scope — 1:1 parity with the Project Sicario merger

**The v1 goal is 1:1 parity with the Sicario merger.**

A drop-in replacement for the Sicario merger (`SicarioPatch.Loader`) against Project Wingman 2.1.1A / UE 4.27 / Pak V11:

- **Inputs:** identical `.dtm`/`.dtp` WingmanMod JSON, with `_vars`/`_inputs` templating.
- **Patch semantics:** every Sicario fragment type, every Sicario patch type, identical matching and value behavior.
- **Merge semantics:** identical multi-mod merging to Sicario's engine.
- **Output:** a Pak V3 mod pack (mount `../../../`) for the game's `~mods/` directory, same as the merger.
- **Zero .NET dependency**: a single native binary.

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
| In-game acceptance | ✅ |

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
Project Wingman 2.0 / UE 4.27, so most 2024+ mods are plain file overrides. When two of
them edit the same uasset, the game loads only one. `modman combine` merges them anyway:

```bash
modman combine <pak-or-dir...> --install-path <game> --output <dir>
```

- DataTables are merged **three-way** (vanilla + each mod's delta, later mods win) at
  **field level**: different fields of the same row all survive, and genuine same-field
  clashes resolve later-wins and are reported.
- Mods that add object references (textures, materials) keep working: import-table
  entries are carried with all FName and package-index references remapped.
- Non-datatable files (maps, models, textures) pass through single-winner, with a warning
  when two mods fight over one. Those file types cannot be combined.
- Every run ends with a **merge report**: the order, every conflict (with mod names and
  values), and warnings.

### Web UI — no install needed

**<https://emberglazee.github.io/project-modman/>**: the same merge engine compiled to
WebAssembly, running entirely in your browser:

- Drop your conflicting `*_P.pak` files, point at your game's
  `pakchunk0-WindowsNoEditor.pak`, merge, and download the combined pak.
- **Nothing is uploaded.** The game pak is read in place through the browser's File API
  (a few MB of it, even from a ~16 GB file), and the page makes zero network requests
  after loading.
- Shows the same merge report (order + conflicts) with a friendly UI.
- This is the no-metadata `combine` path: it does not apply `.dtm`/`.dtp`
  patches. For those, use the desktop **Build presets** tab or `modman build`.

### Desktop app — no terminal needed

`modman-gui` is the same engine in a native window: egui, no browser engine, one
binary at about 13.0 MB. Download it from the release page and double-click; it
finds the game on its own, and asks if it cannot.

- **Merge mods**: drag `.rar`, `.7z`, `.zip` or `.pak` files onto the window.
  Archives are extracted in-process (`rars`, `sevenz-rust2`, `zip`, all pure Rust,
  so nothing has to be installed) and merged; reorder the list to choose which
  mod wins, and the conflict check updates as you move entries. One button then
  installs the result into `~mods`, keeping a timestamped backup of whatever it
  replaced.
- **Build presets**: build a Sicario preset set into one `SicarioMerge_P.pak`.
  Accepts `.dtp`, `.dtm` and `.json` presets plus folders; this is the metadata
  path. **Merge mods** is the metadata-less `combine` and does not apply patches.
- **Game audio**: list the sound assets in a pak and extract them as `.ogg`.
- **Translations**: pull a `.locres` out of the game pak, export it as CSV, and
  patch a translated CSV back in.
- **Pak files**: inspect, unpack and repack.
- **Scan & inspect**: list the installed Sicario components, or inspect a
  `.dtm`/`.dtp` patch file.

Every tab calls the same `modman-core` function the command line calls, so the
window cannot produce output the CLI would not. Linux and Windows builds; on
macOS run the Windows build under Whisky or CrossOver.

### Choosing the merge order

In the browser and the desktop app, the list is the order. Drag a row, or use
its up and down buttons; new files append. The Priority switch decides which
end wins a conflict: **Later in the list wins** applies the list top to bottom,
and **Earlier in the list wins** applies it bottom to top. The winning end is
marked.

Both interfaces check conflicts after a reorder, with a 400 ms debounce, and a
**Check now** button forces the check. It runs the same merge engine and reports
the exact field conflicts, the files that cannot be combined, and warnings,
without writing a pak. The check is not free because it reads every datatable, but the 29-pak aircraft
set probes in about 0.4 seconds; the desktop app keeps the Merge button
available while it runs.

The command line still sorts paks alphabetically by full path, so renaming is
only needed there.

The **Output** menu next to the Priority switch writes the merged pak
uncompressed, Zlib, or Gzip. The merged pak is V3, so zstd and LZ4 are not
offered there; `modman pack --version V11 --compression zstd|lz4` covers those.
`modman combine --compression zlib|gzip` writes the same compressed output from
the command line.

### Downloading & first run

- **Windows**: double-clicking `modman.exe` shows a welcome screen (it pauses so you
  can read it) pointing at the browser option; with arguments it runs as a normal CLI
  (`modman --help`). Windows may show a SmartScreen prompt for a new
  unsigned binary: *More info → Run anyway*.
- **macOS**: the binaries are unsigned, so Gatekeeper blocks the first run. Right-click
  → Open once, or run `xattr -d com.apple.quarantine modman`.
- **Linux**: `chmod +x modman` and run it from a terminal.
- Or skip all of that: the **web page**, or the **offline `modman-merge.html`**, one file
  you double-click with no install, no server and no network.

### Verify what you downloaded

These binaries are unsigned, so there is no publisher name to check. What there is
instead is a signed build provenance statement: every release binary is built by this
repository's CI, and that claim is cryptographically attested by GitHub. Anybody can
check it, and it takes one command with the GitHub CLI (2.49 or newer):

```bash
gh attestation verify ./modman-linux-amd64 \
  --repo emberglazee/project-modman \
  --signer-workflow emberglazee/project-modman/.github/workflows/rust-build.yml
```

The command hashes the file, fetches the attestation GitHub stores for that digest,
and checks the signature against the workflow identity. A pass means the file is the
exact output of a build of this repository at a recorded commit. A fail means the file
is not that, whatever else it is. The same works for the GUI binaries and for the
Windows builds:

```bash
gh attestation verify ./modman-gui-linux-amd64 --repo emberglazee/project-modman
```

`SHA256SUMS` on each release still serves the older workflow: it proves the download
is intact, not who built it. The attestation proves both.

## Translation mods (`.locres`)

The game's text lives in Unreal localization binaries at
`ProjectWingman/Content/Localization/ProjectWingman/<lang>/ProjectWingman.locres`.
modman reads and patches them with full structural fidelity. Every namespace and key
hash, entry order and string reference count is preserved, so in-game lookups never
miss. (Verified: parse → write reproduces all nine shipped language files
byte-for-byte.)

```bash
# 1) Pull the language file straight out of the game's pak
#    (finds the install automatically; --pak <file> to use another pak)
modman locres extract --lang en-US -o ProjectWingman.locres
#    ...or go straight to the spreadsheet: add --csv
#    ...or omit --lang to dump every language at once

# 2) Dump a locres to a translator-friendly CSV (key,source,Translation)
modman locres read ProjectWingman.locres -o translations.csv

# 3) Translate the third column — the CSV matches the UEExtractor format,
#    so the existing translator tooling and workflows apply as-is

# 4) Build a ready-to-install translation mod in one step
modman locres pak ProjectWingman.locres translations.csv --lang en-US -o MyTranslation_P.pak
#    (--dedup merges identical strings, matching other community tools — a
#    rebuild of an existing mod comes out byte-identical to it, verified
#    against the ":3" mod)

# Bonus: see exactly what a mod changes vs the game
modman locres diff Original.locres Mod.locres --csv changes.csv --limit 40
```

Drop the result into `~mods` and you're done.

## Game audio (`.ogg`)

The game stores its sound as Ogg Vorbis inside the pak, so extraction is a strip
rather than a transcode:

```bash
# List the sound assets (with no path, reads the game's own pak)
modman audio list --filter Dialog

# Write every SoundWave as .ogg, mirroring the asset tree
modman audio extract -o pw-audio --filter "Dialog/C_M6"
modman audio extract -o pw-audio --manifest audio.csv    # the whole game
```

The output mirrors the asset path, so `Assets/Audio/Dialog/C_M6/...` stays
navigable. The long music tracks are stored streamed, split between the `.uexp`
and a `.ubulk`; `extract` reassembles them, so each track arrives as one
complete file rather than a 20 second fragment.

## Documentation

In-depth guides live in [`docs/`](docs/README.md):

- [**Architecture**](ARCHITECTURE.md): crate and module map, data flow for each
  command, the naming rules, and where a new feature belongs.
- [**Merging conflicting mods**](docs/merging-mods.md): the no-metadata merge,
  conflict semantics, what can and can't merge, reading the report.
- [**Translation mods**](docs/translation-mods.md): the full `.locres` workflow,
  the CSV format, tips, and binary format notes for tool authors.
- [**FAQ**](docs/faq.md): install paths, first-run warnings (SmartScreen,
  Gatekeeper), merge conflicts, browser-page privacy.
- [**Game audio**](docs/audio.md): listing and extracting the game's sounds as
  `.ogg`, the streamed-track trap, and how to check a result.

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

# Pack a folder; zstd and lz4 need V8+, zlib and gzip work with V3
modman pack <dir> -o out.pak --version V11 --compression zstd
```

## Project Structure

```
crates/
├── modman-core/       Data models, patch engine, merge pipeline, components,
│                      locres reader/writer
├── modman-uasset/     UE4 .uasset binary parser (standalone)
├── modman-pak/        PAK file operations (wraps repak)
├── modman-cli/        CLI binary
└── modman-wasm/       WebAssembly frontend (wasm-bindgen) powering the web UI

web/                   GitHub Pages site: hosted page + offline single-file build
```

## License

MIT
