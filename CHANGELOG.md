# Changelog

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
