# Changelog

## [v0.4.1] — 2026-10-10

**Vortex-native.** The CLI is now a drop-in for the exact invocation GUI
integrations issue (Project Sicario Manager's hook in Vortex), verified
against the real PSM build — which surfaced and fixed two integration-level
bugs that only appear when a Windows GUI drives the tool.

### Project Sicario / Vortex compatibility
- `--installPath` / `--outputPath` (camelCase) aliases and a
  `--non-interactive` flag — the exact call shape Vortex extensions use.
- C#-parity exit codes: 404 (install dir missing), 412 (a patch targets a
  missing file — now fatal, matching `SourceFileNotFoundException`), 422
  (patch application failure). Generic errors keep exit 1.
- Quoted path values (`--installPath="C:\Game"`) are accepted — GUI
  integrations spawn without a shell, so the quotes arrive literally.
- The output folder now contains exactly `mergeReport.json` +
  `SicarioMerge_P.pak`; the scratch `staging/` directory is no longer left
  behind (in the Vortex flow it would have been deployed as junk files).
- The no-args `build` discovers presets from `~mods`/`~presets` +
  `Content/Presets` — covering both Vortex's deploy location and the C#
  merger's directory.

### Verification
- Ran the real C# PSM (Linux build) with the exact invocation: identical
  output shape; identical data + index bytes (one 1-byte diff in the
  footer's legacy unused-count slot, no functional impact).
- Ran the bundled Windows binary under Wine 11: a full merge produced a
  **byte-identical** output to the native Linux binary
  (`faa692a0…` sha256), confirming the Windows builds for integrations.

## [v0.4.0] — 2026-10-10

**Translation modding.** Full `.locres` support — extract from the game,
translate a CSV, build a ready-to-install mod — plus comfort fixes and a
proper link embed for sharing.

### Localization (`.locres`)
- `modman locres extract` — pull language files straight from the game's
  pak (single language, `--csv` straight to the spreadsheet, or all nine
  at once).
- `modman locres read` — dump to the UEExtractor-compatible CSV
  (`key,source,Translation`), so existing translator tooling applies as-is.
- `modman locres patch` / `pak` — rebuild with every namespace/key hash,
  entry order and string reference count preserved, staged at
  `ProjectWingman/Content/Localization/ProjectWingman/<lang>/` as a
  drop-in mod. `--dedup` compacts identical strings like other tools do.
- `modman locres diff` — see exactly what a mod changes versus the game,
  with before/after previews and a full CSV export.
- Fidelity gates: parse → write reproduces **all nine shipped language
  files byte-for-byte** (10,553 entries for `en`); rebuilding the ":3" joke
  mod through the pipeline produces a **byte-identical** file to the
  original modder's; a partial translation was **verified in-game**
  (patched menu entries change, everything else untouched).

### Comfort & polish
- Game auto-detection now covers modern Linux Steam (`~/.local/share`),
  Flatpak and native Windows paths; `PW_INSTALL` takes precedence.
- Double-clicking the binary shows a readable welcome screen (pauses when
  opened from a file manager, never in scripts) pointing at the browser
  and offline options.
- The web page has a proper link embed (og/twitter metadata + a 1200×630
  preview card).

## [v0.3.0] — 2026-10-10

**Community-hardened.** Real Frontline-59 skin mods (the K-9 liveries) exposed
a family of merge bugs that only appear with the 2.0-era mods — every one
fixed and in-game verified, plus an offline single-file page.

### Merge fixes (found with real f59 mods)
- **Replaced imports**: mods that swap an import entry in place (the F59
  skins swap a texture reference to their own asset) are now carried into
  the merged import table with every uexp reference remapped to preserve its
  semantic target — previously the refs silently resolved to the vanilla
  entry and the skins reverted.
- **Mount-aware paths**: mods that mount at a nested path (e.g.
  `../../../ProjectWingman/Plugins/MagadanFront/Content/`) with short
  records now resolve to their real game paths in the merged pak.
- **Opaque assets**: textures, audio and meshes that ship a `.uasset` +
  `.uexp` pair pass through single-winner cleanly (their payloads cannot be
  field-merged) instead of failing the datatable parse.
- **Diagnostics**: merge warnings reach the report again, and an unmergeable
  datatable falls back to the last override's version instead of aborting.

### Single-file offline page
- The web UI builds into **one self-contained HTML file** (wasm embedded):
  double-click it, no server, no network, works offline. Shipped as a
  release asset (`modman-merge.html`) and linked from the Pages site.

### Verification
- The reporter's three conflicting f59 mods (K-9 skins ×2 + FS-15) merge
  into one pak: FS-15 model, F-15SMTD row, mission manifest and all K-9
  liveries confirmed in-game.
- Browser output byte-identical to the CLI (sha256 `5bb7882d…` on identical
  inputs), in both hosted and single-file modes.

## [v0.2.0] — 2026-10-09

**Merging without metadata.** The headline feature: `modman combine` merges
conflicting override mods that carry no Sicario metadata — the common case
for 2024+ Project Wingman 2.0 mods, where two mods edit the same uasset and
the game loads only one of them.

### `modman combine` (no-metadata merging)
- **Three-way datatable merge**: vanilla rows are the base, each mod
  contributes only its delta, later mods win — a mod's untouched rows never
  clobber another mod's edits.
- **Field-level merging**: two mods editing different fields of the same row
  both survive; genuine same-field clashes resolve later-wins.
- **Conflict reporting**: every overwrite is listed with the mod names and
  values — nothing is silently lost.
- **Import carrying**: mods that add object references (textures, materials)
  have their import-table entries copied with all FName and package-index
  references remapped.
- **Row order**: the merged table follows the last mod's row order (mods can
  insert rows mid-table).
- **Single-winner pass-through** for non-datatable files (maps, models) with
  explicit warnings; datatable sidecar records are consumed so merges are
  never clobbered.

### Merge reports (both paths)
- Consolidated **end-of-merge report** for `build` (with metadata) and
  `combine` (without): the merge order, every field-level conflict (which
  mod overwrote which value), and warnings.

### Verification
- Single-override byte-identity: a real 2026 V11 mod's datatable reproduces
  exactly (59,162 B uexp); an import-adding skin DB is byte-identical
  (39,336 B uasset + 99,330 B uexp).
- Two-mod merge: 2026 mod + a second LevelList mod = 60,592 B / 46 rows with
  both mods' edits present, zero spurious conflicts.

## [v0.1.0] — 2026-10-09

First release: **1:1 parity with the Project Sicario merger**, verified
byte-for-byte against the C# reference implementation and accepted in the
real game (Project Wingman 2.1.1A).

### Merge engine
- Full `.dtm`/`.dtp`/embedded-request parsing, Fragment DSL, and templating
  (Fluid-compatible: `vars`/`inputs`, the complete Sicario filter set).
- DataTable patches — every Sicario patch type (propertyValue,
  modifyPropertyValue, arrayPropertyValue, textProperty, duplicateEntry,
  duplicateProperty, duplicateArrayItem, deleteEntry, objectRef), byte-exact
  against the C# merger's output (uassets identical; uexps identical modulo
  the merger's own random FText keys).
- `filePatches` hex engine — every HexPatch quirk replicated (fixed
  replacement-type order, stream-walk semantics, windows, the `.uexp`
  length auto-correct).
- Component model — `embeddedPresets → loosePresets → sicarioRequests →
  customSkins` ordering, parameter/input merging, engine-version gating,
  merge-report generation (byte-identical to the C# merger's report).
- `customSkins` PSM skin-slot merging via `objectRef` import-table writes —
  live-verified in-game (AJS-37 skin slot).

### Commands
- `modman build` — merge mods/presets into a real `SicarioMerge_P.pak`.
- `modman preset-pack` — standalone preset packs with the preset embedded
  at `Content/sicario/`.
- `modman scan`, `modman patch`, `modman info`, `modman list`,
  `modman unpack`, `modman pack`.

### Beyond parity
- **Self-contained merges** (default): the merged pak embeds detected skin
  files so one pak installs everything (`--no-embed-skins` restores the
  strict C# output).
- `--report` writes the C#-identical merge report.
