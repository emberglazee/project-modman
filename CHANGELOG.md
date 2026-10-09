# Changelog

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
