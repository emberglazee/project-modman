# Merging conflicting mods (`modman combine`)

## The problem

Project Sicario's merge metadata (`.dtp` presets, embedded requests inside paks) was
never updated for Project Wingman 2.0 / UE 4.27. Every mod built since then is a plain
file override — and when two mods override the same game file, **the game loads only
one of them**. The other mod's edits silently don't exist.

`modman combine` merges them anyway, without any metadata.

```bash
modman combine <pak-or-dir...> --install-path <game> --output <dir>
```

## What can and cannot be merged

| File kind | Behavior |
|---|---|
| **DataTables** (`DB_*` — aircraft, weapons, levels, missions…) | **Fully merged**, field by field |
| Anything adding object references (skins, textures referenced by tables) | Merged — import-table entries are carried with all references remapped |
| **Maps, models, meshes, textures, audio** | **Not mergeable** — single winner (the last mod's version). Their payloads (pixels, samples, vertices) cannot be combined; a warning is printed when two mods fight over one |

## How the DataTable merge works

- **Three-way**: the game's original file is the base; each mod contributes only its
  *delta* (rows/properties it actually changed). A mod's untouched entries never
  clobber another mod's edits.
- **Field-level**: each property merges individually. Mod A editing a row's speed and
  mod B editing the same row's weapons → both survive.
- **Same-field conflicts**: when two mods set the *same* property differently, the
  later mod in the order wins, deterministically — and the conflict is **reported**,
  never silent.
- **Order** = the paks' filenames, sorted. The report shows the order used.

## Reading the report

```
=== Combine report ===
Order (later entries win on conflicts):
  1. ModA_P.pak
  2. ModB_P.pak
Merged 2 datatable(s), 7 file(s) total
Conflicts (later mod won):
  - DB_Aircraft.uasset: SPEAR.FixedLoadout: 'ModA_P.pak' sets 'true', 'ModB_P.pak' sets 'false' (later wins)
Warnings:
  - SomeMap.umap: overridden by multiple mods — only the last version is kept
```

- **Conflicts** — every same-field overwrite, with mod names and values.
- **Warnings** — nested-mount or unmergeable-file notices.
- **nothing listed** — no overlaps; everything applied cleanly.

## Installing the result

The output is a single `SicarioCombine_P.pak` (V3, mount `../../../`). Drop it into
your game's `~mods` folder and **remove the original conflicting paks** — the merged
file replaces them.

## Edge cases we've seen in the wild

- **Nested mounts**: some mods mount at a path like
  `../../../ProjectWingman/Plugins/MagadanFront/Content/` with short internal paths.
  modman resolves these to real game paths — the merged pak is always self-consistent.
- **Replaced imports**: some mods swap an asset reference *in place* (e.g. a skin mod
  pointing an existing table slot at its own texture). The merge carries those entries
  into the merged import table and remaps every reference — so the swap survives.
- **Shared texture/mesh files**: the f59-era skin packs ship aircraft meshes and
  textures too; those pass through untouched (single winner), while the tables that
  reference them merge normally.

## The browser option

No install needed: <https://emberglazee.github.io/project-modman/> — drag the same mod
paks in, point it at your game's `pakchunk0-WindowsNoEditor.pak`, and it merges
entirely in your browser (nothing is uploaded; the game pak is read in place, a few MB
of it). There is also an **offline single-file** build (`modman-merge.html` on the
releases page) that works by double-click with no server at all.

Both run the exact same merge engine as the CLI — byte-identical outputs.
