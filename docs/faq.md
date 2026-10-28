# FAQ

## General

**Does modman touch my game install?**
Not unless you ask it to. Every command reads the game's pak (and `~mods` when
building) and writes only where `--output` points. The desktop app's Install button
is the one exception: it copies the merged pak into `~mods` and keeps a timestamped
backup of whatever it replaces.

**Where is my game's data pak?**
`…/steamapps/common/Project Wingman/ProjectWingman/Content/Paks/pakchunk0-WindowsNoEditor.pak`
(note the nested `ProjectWingman` folder). It is still about 16 GB in the 2.x era,
which is normal. modman never reads more than a few MB of it.

**What is `--install-path` for?**
It points modman at your game so it can use the original files as a reference: the
merge needs the vanilla data to tell what each mod changed, and the translation tools
need the shipped language files. It's optional: modman auto-detects common Steam
locations (Linux, Flatpak, Windows, WSL) and respects the `PW_INSTALL` environment
variable.

**Something flashed and closed on Windows.**
You double-clicked the CLI with no arguments. It now shows a welcome screen and
pauses; the CLI itself takes arguments (`modman --help`). For no-install use, the
web page or `modman-merge.html` need no terminal at all.

**Windows says "Windows protected your PC".**
The binaries are unsigned. *More info → Run anyway*. (Same for SmartScreen flags on
every release.)

**macOS refuses to open the binary.**
Gatekeeper blocks unsigned binaries: right-click → Open once, or run
`xattr -d com.apple.quarantine modman`.

## Merging mods

**Which mods can be merged?**
Any mods that override DataTables (`DB_*` files), which is the vast majority of
gameplay mods. Aircraft stats, weapons, mission tables, level lists all merge
field-by-field.

**Why did a map/texture/model not merge?**
Those file types can't be combined: the payloads (pixels, audio samples, level
geometry) aren't field-structured. The tool keeps the last mod's version and warns
you. If two mods genuinely both need to change the same map, that's a manual
decision, not something any tool can automate.

**Two mods changed the same stat. Who wins?**
The later mod in the application order, which the report shows. In the browser and
the desktop app the list is the order and the Priority switch chooses which end
wins; drag a row or use its arrows. The command line sorts paks by filename, so
renaming changes priority only there. The conflict is listed with both values so
you can see it happened. If you want the other one to win, move the entry in the
app or rename the paks for the CLI.

**What does the conflict check do?**
It runs the merge's conflict pass without writing a pak: the same engine reads the
datatables, reports field conflicts, files that cannot be combined, and warnings,
and leaves the output folder alone. It runs after a reorder with a short debounce,
or when you press Check now. It is not free, because it reads every datatable, but the
29-pak aircraft set probes in about 0.4 seconds.

**How do I know a merge worked?**
The report shows the order, every conflict and every warning. The tool's outputs are
byte-verified in the project's test suite. In practice: merge, install, and look at it
in-game. A full acceptance run of three previously-conflicting F59 skin mods is
documented in the project's notes.

**Does the merged pak need the original mods installed too?**
No. The output is self-contained: it includes the merged files plus any referenced
assets the tool carried over. Remove the original conflicting paks after installing
the merged one.

## Translating

**Which file do I translate for the main game?**
`en` holds the base game's text; `en-US` holds Frontline-59 DLC text (in the 2.1.1A
build). If in doubt, dump both: `modman locres extract -o dir/` gets every language.

**Can I use my existing translation workflow?**
Yes. The CSV is the same `key,source,Translation` format UEExtractor uses, so
spreadsheet tools, scripts and the LLM-translation flows built for that ecosystem work
unchanged.

**Can I test a translation quickly?**
Yes. Extract, patch a handful of visible menu strings (e.g. "Options"), build, and
look at the pause menu. Everything else stays untouched, which is the point of the
surgical patch.

**Will a translation mod break when the game updates?**
The patch preserves the game's structure, but new keys added by a game update won't
be covered until you re-extract and re-patch (the diff tool makes comparisons easy).
Keep your translations as CSV: that's your source of truth, and re-patching a new
game build is one command.

## The web version

**Is anything uploaded when I use the web page?**
No. The merge runs entirely in your browser (WebAssembly); your game pak is read in
place via the browser's file API and never leaves your machine. The page makes zero
network requests after it loads, and the offline `modman-merge.html` works with no
network at all.

**Why is it so large for a web page?**
The merge engine is embedded (~0.57 MB single file). It runs the same code as the
command-line tool, byte for byte, which the project's test suite checks.

## Credits

The `.locres` handling follows the format work of
[SolicenTEAM/UEExtractor](https://github.com/SolicenTEAM/UEExtractor) (MIT), whose
CSV format this tool is compatible with. The merger behavior is validated against the
Project Sicario merger as a reference oracle.
