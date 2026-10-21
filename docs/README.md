# Documentation

## Guides

- [**Merging conflicting mods**](merging-mods.md): how `modman combine` merges the
  2024+ no-metadata mods that normally can't coexist, covering field-level DataTable
  merging, conflict semantics, what can't merge and why, and reading the report.
- [**Translation mods**](translation-mods.md): the full `.locres` workflow from
  extract to read to translate to build, the CSV format, tips, and the binary format
  notes for tool authors.
- [**FAQ**](faq.md): install paths, first-run warnings (SmartScreen/Gatekeeper),
  merge conflicts, translation questions, browser-page privacy.
- [**Merging in the browser**](merging-in-the-browser.md): the no-install route,
  step by step, and what to do when a large set runs out of memory.
- [**Command line walkthrough**](command-line-walkthrough.md): the same job
  without jargon, for someone who has never opened a terminal.
- [**Game audio**](audio.md): listing and extracting the game's sounds as `.ogg`,
  the streamed-track trap, and how to check a result.

## Historical

The founding documents of the project (June 2026), kept as a record of how the tool
was built milestone by milestone:

- `superpowers/specs/2026-06-30-modman-design.md`: the original design.
- `plans/`: the plan for each early milestone (0.1.0 scaffold through 0.8.0
  Fragment DSL).

## See also

- The [README](../README.md): overview, parity status, usage.
- The [CHANGELOG](../CHANGELOG.md): what changed in each release.
