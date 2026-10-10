# Translation mods (`.locres`)

Project Wingman's text — menus, missions, dialogue, everything — lives in Unreal
localization binaries at:

```
ProjectWingman/Content/Localization/ProjectWingman/<lang>/ProjectWingman.locres
```

Languages shipped: `en`, `en-US`*, `de-DE`, `es-ES`, `fr-FR`, `ja`, `ko-KR`, `ru-RU`,
`zh-CN`. modman reads and patches them with full structural fidelity: every namespace
and key hash, entry order and string reference count is preserved, so the game's
lookups can never miss.

\* `en-US` holds the Frontline-59 DLC text; `en` holds the base game.

## The workflow

```bash
# 1) Pull the language file straight out of the game's pak
modman locres extract --lang en-US -o ProjectWingman.locres
#    --csv → straight to the spreadsheet (no intermediate file)
#    no --lang → dump every language at once

# 2) Dump it to a translator-friendly CSV
modman locres read ProjectWingman.locres -o translations.csv

# 3) Translate the third column. The CSV format is key,source,Translation —
#    the same format UEExtractor uses, so existing translator tooling and
#    workflows apply as-is.

# 4) Build a ready-to-install mod in one step
modman locres pak ProjectWingman.locres translations.csv --lang en-US -o MyTranslation_P.pak
```

Drop the result in `~mods`. That's the whole process — the pak contains one file, the
patched locres, staged at exactly the right path.

## Reviewing what a mod changes

Before patching, or to review someone else's translation:

```bash
modman locres diff Original.locres Mod.locres --csv changes.csv --limit 40
```

Shows every changed entry with before/after previews, counts added/removed entries,
and can dump the full list to CSV.

## Tips

- **`--dedup`** merges identical strings into single entries (summing reference
  counts) — the compaction other community tools use. A rebuild of an existing mod
  with `--dedup` comes out byte-identical to the original (verified against the
  famous ":3" mod).
- **Shared strings**: some strings are referenced by several keys. Editing one key's
  translation also affects the other keys pointing at the same string — `locres diff`
  shows exactly which entries changed.
- **Patch mode preserves, it doesn't rebuild**: the game's own hashes and ordering are
  kept, so the engine finds every entry exactly as before. Rebuilding from scratch is
  not needed and not offered — preservation is strictly safer.
- **Test in-game**: a partial translation (patch a handful of visible menu strings)
  is the quickest way to confirm a build works before doing 10,000 lines.

## Format notes (v3, the optimized layout)

For anyone writing their own tooling — the file layout as shipped:

```
[16] magic   0E 14 74 75 67 4A 03 FC 4A 15 90 9D C3 37 7F 1B
[1]  version (3 = CityHash64-over-UTF16 hashes; 2 = CRC32)
[8]  string table offset (i64, absolute)

key section:
  u32 entry count
  u32 namespace count
  per namespace: u32 hash, FString name, u32 key count
    per key: u32 key hash, FString key, u32 source hash, u32 string index

string table (at the offset):
  u32 count
  per string: FString value, u32 reference count
```

Three subtleties that byte-fidelity depends on:

1. **Empty FStrings are serialized as length 1 + a null byte** (5 bytes) — not as
   length 0 (4 bytes).
2. **Reference counts are real** — a string used by 9 keys has count 9.
3. **FString length rule**: positive length = UTF-8 bytes + 1; negative length =
   UTF-16 code units + 1.

Verified: parse → write reproduces all nine shipped language files byte-for-byte.
