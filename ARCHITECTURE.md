# Architecture

modman reads, merges and writes Project Wingman's cooked Unreal assets. It is a
Rust workspace with one crate per layer: the pak format, the uasset format, the
merge logic, and the frontends. This file says what lives where, how data moves
between the layers, and where a new feature belongs.

## Crates

| Crate | Package name | Role | Depends on |
|---|---|---|---|
| `modman-pak` | `modman-pak` | Reads and writes `.pak` archives (V3, V8, V11 in, V3 out). Owns the footer, index and entry tables. | none |
| `modman-uasset` | `modman-uasset` | Reads and splices cooked `.uasset` and `.uexp` pairs: header, name and import tables, the DataTable walker, the byte editor, table rewriting, object-import insertion, SoundWave extraction, name hashing. Knows nothing about mods. | none |
| `modman-core` | `modman-core` | Everything in between: manifests, templates, fragments, patch parsing, resolution, length-changing row operations, the merge pipeline, `combine`, locres, audio, component discovery. | pak, uasset |
| `modman-cli` | `modman` | The command line. Argument parsing, path handling, progress and report output, file writing. | core, pak, uasset |
| `modman-gui` | `modman-gui` | The native desktop app (egui): one tab per job (merge, build, audio, translations, pak, tools). Native dialogs, archive extraction, progress, install-with-backup. Every tab calls the same core function the CLI calls. | core, pak |
| `modman-wasm` | `modman-wasm` | The browser frontend. Exposes `combine` to JavaScript and reads the game pak through a sparse `Read + Seek` source. | core, pak |

`modman-wasm` deliberately shares `combine_sources` and the `PakSource` trait
with the CLI rather than reimplementing them. A merge in the browser and a merge
on the command line are the same code over different byte sources.

The same rule applies one level up: `combine::run_to_dir` owns the filesystem
side of a merge (open the game pak, open each mod, merge, stage, pack), and the
CLI and the GUI both call it. Game detection lives in `core::game` for the same
reason. A frontend decides *what* to merge and *how to show it*; it does not get
its own copy of the merge.

The CLI's package name is `modman` while the directory is `modman-cli`, so
build it with `cargo build -p modman`.

## Module map (`modman-core`)

| Module | Responsibility |
|---|---|
| `manifest.rs` | The Sicario data model: `WingmanMod`, `WingmanPreset`, `MetaRequest`, patch sets and patches. Lenient parsing (case-insensitive keys, comments, trailing commas, explicit nulls). |
| `discovery.rs` | `scan_paks_dir`: find embedded `_meta` build records and `.dtp` presets inside a Paks directory. |
| `components.rs` | Turn a scan plus loose files into ordered `MergeComponent`s (embedded presets, loose presets, embedded requests, custom skins) and drain them into mods in priority order. |
| `templating/` | `mod.rs` is the renderer (`{{ vars.x \| filter }}`, the Sicario filter set). `apply.rs` is the per-mod pipeline that renders vars, gates `enableSteps`, and rewrites patch values, templates and windows. |
| `fragment.rs` | Parses a patch template (`datatable:['Row'].{'Prop'}`) into fragments. |
| `patch.rs` | Parses a patch value (`BoolProperty:true`, arithmetic modifications, array modes) into a `PatchOp`. |
| `resolver.rs` | Walks a parsed `DataTable` and resolves fragments to concrete nodes, giving byte spans. |
| `apply.rs` | Plans and applies same-size edits (byte splices) to a `.uexp`. |
| `rowops.rs` | Length-changing edits: duplicate a row, copy and sub-edit it, rewrite tables and fix up sizes. |
| `objectref.rs` | Appends object references (import entries plus name table additions) for skin and asset references. |
| `merge.rs` | The per-mod pipeline: templating, resolver, apply, rowops, objectref, then hex patches. Later mods win. |
| `hexpatch.rs` | `filePatches`: the HexPatch port, including its quirks. |
| `combine.rs` | Merging plain override mods that carry no Sicario metadata: pair detection, class-based dispatch, three-way datatable delta merge against vanilla, import and name table merging, report assembly. |
| `build.rs` | The Sicario build: gather components in provider order, apply variables, merge every mod's patches against the vanilla tables, write one pak. `gather` is shared with the CLI's dry-run preview so the preview and the build see one order. |
| `locres.rs` | `.locres` parse, patch, diff and CSV interop. |
| `audio.rs` | Lists and extracts SoundWave assets as Ogg. |
| `combine::FilePakSource` | The public `PakSource` over a `.pak` on disk; the audio and locres readers take it too. |

`modman-uasset` is organised the same way: `header.rs` (package header and
counts), `names.rs` (name table), `export.rs` (export map), `walk.rs` (the
verified DataTable walker), `edit.rs` (single-value splices), `rewrite.rs`
(tables rewritten after an append), `hash.rs` (name hashes), `soundwave.rs`
(Ogg stream extraction).

## Data flow

### `modman build` (Sicario compatibility)

```
mods / presets / embedded _meta
        |
   discovery + components          embedded presets -> loose presets -> embedded requests -> custom skins
        |
   take_ordered_mods               ascending priority
        |
   templating::apply_variables_to_mod        vars -> enableSteps -> value -> template -> window
        |
   fragment -> patch -> resolver    template + value -> nodes with byte spans
        |
   apply / rowops / objectref        same-size splices, or rewrites with fixups
        |
   merge::merge_mods                 sequential, later mod wins, hex patches after
        |
   modman-pak write                  SicarioMerge_P.pak + report
```

### `modman combine` (no metadata)

```
paks -> PakSource -> (.uasset, .uexp) pairs -> export class
                                                    |
             DataTable: walk -> three-way delta vs vanilla -> field-level merge
             other:     single winner passes through, with a warning
                                                    |
              name + import table merge, reference remap
                                                    |
              output pak + report (order, conflicts, warnings, skipped)
```

The delta is the point: rows identical to vanilla are skipped, so one mod's
untouched rows never clobber another mod's edits.

### `modman locres` and `modman audio`

Both read the game pak directly and write files, with no merge step. `locres`
preserves namespaces, hashes, order and ref counts and swaps only string values.
`audio` copies the Vorbis stream out of the `.uexp` (plus the `.ubulk` when the
asset is streamed) and trims it at the end-of-stream page.

## Conventions

**A module is one `.rs` file.** It becomes a directory only when it has
submodules to hold, and then the parent is `mod.rs` with re-exports so call sites
read the concept, not the internal path. `templating/` is the only such
directory.

**Names say what the thing is.** `combine` merges metadata-free mods, `build`
merges Sicario mods, `merge` is the per-mod pipeline inside `build`. Avoid a
second name for a concept that already has one: the pair that prompted this rule
was `template` and `templating` living side by side.

**One crate, one layer.** Format knowledge stays in `modman-uasset` and
`modman-pak`. A module in `modman-core` may call them but must not reimplement
byte layouts, and must not contain CLI concerns like printing or path guessing.

**Edits are minimal splices.** Never reserialize a whole asset to change part of
it. Planned edits are applied end to start, with size and offset checks, so every
byte that was not edited stays identical. This is also what makes byte parity
with the C# reference achievable.

**Name indices resolve by string, never by position.** A mod cooked against a
different base orders the same strings differently, so positional identity
silently re-points property tags.

**Every merge run ends with a report.** Conflicts are named, never silent, and
same-field clashes say which mod set which value.

**Divergences from the C# merger are deliberate and documented.** Where our
behaviour is better (or fixes a bug the reference has), it is a documented
decision with a parity escape hatch where one makes sense, not an accident.

## Adding things

| Task | Where to start |
|---|---|
| A new CLI command | `modman-cli/src/main.rs`: a `Commands` variant, a `Cmd` enum if it has subcommands, then a `cmd_*` function. Keep printing in the CLI, keep logic in `modman-core`. |
| A new asset format to read | A new module in `modman-uasset` that parses the `.uasset`/`.uexp` pair, plus a dispatcher in `modman-core` keyed on the export class. `audio.rs` is the smallest worked example. |
| A new patch type | `patch.rs` to parse it, `apply.rs` or `rowops.rs` to apply it. If the C# merger supports it, port its grammar rather than inventing one, and check it against the reference build. |
| A new merge rule | `combine.rs` for metadata-free merging, `merge.rs` for the Sicario pipeline. Both have corpus gates; add a fixture rather than only a unit test. |
| A pak-level feature | `modman-pak`. Check the version matrix (V3, V8, V11 read, V3 write) before assuming one layout. |

## Tests

Unit tests live beside the code. Fixtures live in `crates/*/tests/fixtures/`
and must be small. Anything that needs the retail game or the mod corpus is
gated on the path existing and skips with a message otherwise, so CI stays green
without them. Byte-parity tests compare against oracle output where the
reference is deterministic, and mask the non-deterministic bytes where it is not.

## History worth knowing

`core/engine/` used to hold a prototype patch engine that reported what it would
do instead of doing it. It was deleted once the real pipeline existed. If you
find a comment referring to `compile_mod` or `plan_build`, it is stale.
