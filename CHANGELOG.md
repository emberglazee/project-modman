# Changelog

## [v0.5.0] — 2026-10-22

**Game audio comes out as Ogg, and the desktop app arrives.** `modman audio`
lists the sound assets in a pak and writes each SoundWave out as the `.ogg`
stream it already contains. `modman-gui` puts the merge in a window, with no
browser engine and no terminal.

### Added
- `modman-gui`, a native desktop front end. Drop `.rar`, `.7z`, `.zip` or `.pak`
  files on the window and it extracts them in-process, merges, shows the
  conflict report and installs into `~mods` with a timestamped backup. Archive
  support is pure Rust (`rars` Apache-2.0, `sevenz-rust` MIT, `zip` MIT), so
  nothing has to be installed on the machine, on Windows or anywhere else. egui,
  no browser engine, 10.7 MB binary.
- `modman audio list` prints the SoundWave assets in a pak, or in the game's own
  pak when no path is given, with `--filter` to narrow by path substring.
- `modman audio extract` writes each SoundWave to a mirrored tree of `.ogg`
  files, with `--output`, `--filter` and `--manifest` (a CSV of what was
  written). Non-sound assets are counted and skipped, and an asset that fails to
  parse is reported rather than aborting the run.
- Both storage layouts are handled: the stream inline in the `.uexp`, and the
  streamed form where a 256 KB prefix sits in the `.uexp` and the rest in a
  sibling `.ubulk`. The streamed form is what the long music tracks use, and
  what any mod that imports audio with streaming enabled produces.
- Output is the game's own Vorbis stream, trimmed at the Ogg end-of-stream page,
  so nothing is re-encoded. Durations match the engine's own `Duration` field to
  the decimal: the three Kings tracks decode at 424.615 s, 380.308 s and
  162.000 s from the base game, and 248.611 s, 154.681 s and 162.000 s from the
  Archange replacement mod.
- Whole-pak run on the retail build: 5,118 `.ogg` files, 345 MB, every one
  decoding cleanly. One Frontline-59 track (`Music/M05/PW_MF_M5_LOOP`) is stored
  with UE bulk compression rather than as a raw stream, so it is named and
  skipped.
- Two guides written for people who are not tool authors:
  [`docs/merging-in-the-browser.md`](docs/merging-in-the-browser.md) and
  [`docs/command-line-walkthrough.md`](docs/command-line-walkthrough.md). The
  second one explains how to open a terminal, what each part of the command
  means, and what every common error means.
- The browser page opens with a four-step "first time here" block, and every
  failure gets a titled explanation instead of a generic message: out of memory,
  not a pak, stuck reading the game pak, unreadable file, and internal error are
  each recognised and answered with the next step.
- Failed merges produce a **copyable diagnostics block**: the page, browser, mod
  list with sizes, game pak, number of reads and where it stopped. Reports built
  from that are actionable without a follow-up round trip.

### Changed
- Pass-through files (textures, meshes, audio: everything that cannot be
  field-merged) are streamed from the source pak into the output instead of
  being held in memory. Measured on the same 29-mod set (4,410 MB of paks,
  4.3 GB output), released 0.4.5 versus 0.5.0:

      peak RSS   8,852 MB  ->  98 MB   (CLI)
                               631 MB  (GUI, including extracting 29 archives)
      time          25 s   ->  10 s    (CLI)
      output     sha256 555dca0d... identical in both

  An 8 GB machine could not run that merge on 0.4.5 at all; 0.5.0 uses a tenth
  of it.
- `combine::run_to_dir` is the single filesystem implementation behind both
  `modman combine` and the GUI, verified byte-identical against the previous
  inline path. Game detection moved from the CLI to `core::game` so both use it.
- The browser's practical ceiling is measured and stated rather than implied:
  with aircraft addons of about 140 MB each, 4 merged and 6 ran out, so roughly
  500 MB of mod paks is a safe batch. The page hint, the out-of-memory message
  and the guide all say so, and point at the desktop build, which does the same
  29-mod set in about 10 seconds from the command line, or under a minute in the
  GUI including archive extraction.
- Dependencies are current. `cargo update` moved 37 packages, and the GUI moved
  to eframe/egui 0.37, rfd 0.17 and zip 9, which required migrating to the new
  `App::ui` trait and the `Panel::top`/`Panel::bottom` API, plus `x11`/`wayland`
  features that are no longer implied once `default-features` is off. Measured on
  the 29-mod set, upgraded versus before: **+2.7% CPU** (7.31 → 7.51 s of user+sys
  time), memory unchanged at 98 MB, output unchanged, GUI binary 10.7 → 9.6 MB.
  Wall time on a loaded machine swings 10–20 s and is not a usable metric; the
  CPU figure is from an interleaved A/B run.
- Release binaries are built with thin LTO, one codegen unit and stripped
  symbols, from a workspace-level profile. The GUI went from 17.2 MB to 10.7 MB
  and the CLI from 3.9 MB to 3.1 MB, and the 29-mod merge got faster (15 s to
  9 s). The profile that had been sitting in the GUI crate's own manifest was
  silently ignored: cargo reads profiles only at the workspace root.

### Fixed
- The browser page gave up after 200 reads from the game pak and could stop
  without saying so. It now runs to completion (100,000 reads as a safety net)
  and reports a stalled read as its own error instead of looping or hanging.
- Loading a file that is not a pak produced a raw repak version probe
  (`trying version V11 failed: got 111, which is not a boolean`) with no
  indication of what to do. That now reads "One of the files is not a mod pak"
  with the three usual causes.
- The page's scripts carry a version query (`?v=0.5.0`), so a returning visitor
  no longer runs a cached copy of the previous release's JavaScript.

## [v0.4.5] — 2026-10-21

**Combine reports are quieter and merged tables load.** A real mod set (5
aircraft mods plus a stale output) produced ~150 phantom conflicts and 432
pass-through warnings, and its merged `DB_MF_Aircraft` crashed the game with an
`Import_Create` error on load.

### Fixed
- `modman combine` skips a previous run's output. An input named
  `SicarioCombine_P.pak` gets a note in the order list, the warnings and stderr,
  and no longer floods the report. A merged table's shifted import indices made
  every vanilla row read as "changed". The browser page shows the skip the same
  way, as `skipped` in the report JSON.
- Conflicts on object references are compared by target, not by raw import
  number. Two mods whose refs resolve to the same `(ObjectName, ClassName)` in
  their own tables no longer conflict just because one says `-497` and the other
  `-485` for the same `F_15E_Cockpit`. Different targets still conflict, and the
  winner's reference still remaps.
- Arrays and structs now show a summary of the conflicting value, such as
  `ObjectProperty[5] f15_59_01,f15_01,…`, where the message used to read
  `'' vs ''`.
- `combine` now produces tables the game can load when a mod was cooked against
  a different name-table base. Three places assumed index positions were shared
  between the two tables:
  - name indices are resolved by string. A mod's `A-10/A-10` at position 0 used
    to be read as vanilla's `f15_icon`, which re-pointed every property tag;
  - the `None` terminator that closes a struct or array child list is remapped.
    Before, it kept the mod's index, resolved to an unrelated string, and the
    loader read past the struct. That is the `DB_MF_Aircraft` crash;
  - row-name indices come from the stored bytes. Duplicate rows such as
    `CF-105_2` carry their number in the `FName`, so a search by display name
    missed them.
- A row that one mod's deletion pass removes and a later mod re-carries now
  comes back. It re-enters the merge with the vanilla properties as its base, so
  fields identical to vanilla survive and row assembly no longer panics on a row
  it cannot find.

### Added
- `VIRUSTOTAL.md` ships with every release. The release job scans all six
  binaries and attaches the result, so a download can be checked before it runs,
  and builds scan their platform binaries and report in the run summary. The
  scan looks a file up by SHA-256 first and uploads only an unknown hash (the
  public API allows 4 requests a minute and 500 a day). A detection is reported
  and does not fail the job: one heuristic flagging an unsigned binary should not
  block a release.

## [v0.4.4] — 2026-10-11

**Datatable-only merges.** A new switch for people whose conflicts are in
the data, not the visuals.

### New
- `modman combine --datatables-only`: the output contains only the game's
  DataTable files (merged; unmergeable ones keep the last override's pair,
  as always). Textures, audio, meshes and loose files are left out of the
  output entirely, and the report counts what was excluded — so a merged
  table mod can coexist with the original skin/visual mods instead of
  swallowing them. Useful for keeping outputs small and letting visual
  mods keep loading on their own.

## [v0.4.3] — 2026-10-11

**Web page hardening.** A community report of `RuntimeError: unreachable
executed` — a Rust panic trapped inside WebAssembly, with the browser
hiding the actual message — drove three fixes to the browser build:

### Web (wasm) fixes
- Panics now log their real message and location to the browser console
  via a panic hook, so reports say *what* failed instead of a bare trap.
- Internal panics are contained with `catch_unwind`: the page shows a
  readable "internal error: …" message and keeps working instead of dying
  with a bare unreachable trap.
- The merge session now shares mod-pak bytes via `Rc` instead of cloning
  them per merge — halving peak memory for large mods (the browser's wasm
  memory ceiling is the likeliest trigger for the reported trap).
- The page's error box now explains the console + memory hints and points
  at the issue tracker.

### Verification
- The single-file page re-ran the full file:// acceptance merge of
  E5Table + SecondLevelMod against the real game pak: output
  **byte-identical** to the recorded reference (`5bb7882d…`).

## [v0.4.2] — 2026-10-11

**Friendlier on the command line.** A community report walked into two
ways `--install-path` and wildcards surprise people; both are now handled.

### Fixes
- `--install-path` pointed at a file or folder *inside* the game tree
  (a `.pak`, `Paks\`, `Content\`, or the `ProjectWingman` folder itself)
  now walks back out to the game folder, explains what it did, and runs —
  instead of failing with an I/O error.
- Wildcards (`~mods\*.pak`) are expanded by modman itself, so they work
  in cmd.exe and PowerShell (which don't expand globs for native
  executables) as well as in real shells.
- Both fixes apply to `combine` and `build`; the help texts now say so.

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
