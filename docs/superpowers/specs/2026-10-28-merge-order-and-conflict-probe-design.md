# Merge order lists and conflict probe

**Date:** 2026-10-28
**Status:** Approved design, pre-implementation
**Release:** 0.6.0, folded in before the user publishes

## Problem

Both graphical frontends throw away the order the user arranged. The desktop
app calls `self.inputs.sort()` in `MergeTab::add_paths`, and the browser sorts
`modFiles` by `localeCompare` in both `renderModList` and the merge runner. The
only way to change which mod wins a conflict is to rename a file so the ASCII
sort puts it where the user wants it, which is a CLI workaround leaking into a
GUI.

The interfaces also give no warning before a merge. A user learns about a
field-level conflict by running the merge and reading the report afterwards.

The engine does not share this limitation. `combine_sources_with` applies mods
in the exact slice order it receives, and `run_to_dir` preserves the pak order.
The frontends are the only place the order is lost.

## Goals

- An explicit merge order list in the desktop app and in the browser.
- Drag to reorder, plus up and down buttons and a remove button.
- A direction switch: later entries win or earlier entries win.
- A conflict probe that runs automatically after a reorder, debounced, with a
  manual refresh, and reports the same conflicts a merge would without writing
  an output pak.
- The list order is the priority; no renaming.
- The CLI keeps its alphabetical order. This pass does not add a CLI flag or
  order file.
- Documentation and changelog updates in the same change, folded into 0.6.0.

## Non-goals

- A CLI order flag or an order file.
- Persisting an order between sessions.
- Expanding an archive into separate rows for each pak it contains.
- Resolving conflicts automatically or editing values in the UI.
- Changing the merger's rule that the later applied mod wins.

## Verified current behavior

- `combine_sources_with` iterates `mods` in slice order. No sort inside core.
- `run_to_dir` opens `mod_paks` in the given order and builds `labels` in that
  order.
- The desktop app sorts in `MergeTab::add_paths`. The browser sorts in
  `renderModList` and again before calling `session.add_mod`.
- `CombineOutcome` carries the output file map, `merged`, `field_conflicts`,
  `pass_through_conflicts`, `pass_through`, `warnings` and `skipped`;
  `CombineRun` adds the output path, the total file count and `order`.
- The desktop merge job sorts the extracted pak list at `job.rs:485`
  (`paks.sort()`), so that sort must be removed or block order is lost.
- `collect_paks` walks an extraction directory in filesystem order, which is
  not stable across runs or platforms.
- `run_to_dir` reads pass-through files while writing the staging tree. The
  conflict computation itself does not need their bytes.

## Order model

The list is always the order the user arranged, called the display order.

- **Later in the list wins** (default): application order is the display order,
  top to bottom. The bottom entry is applied last and wins a conflict.
- **Earlier in the list wins**: application order is the display order
  reversed, bottom to top. The top entry is applied last and wins.

The engine keeps one rule: the later applied mod wins. The switch only decides
which way the display list is fed to the engine.

The winning end carries a small badge, and the caption states the rule:

- Later wins: "Applied top to bottom. Later entries win."
- Earlier wins: "Applied bottom to top. Earlier entries win."

The switch re-runs the probe. The merge and the probe both receive the
application order derived from the current switch.

The engine report keeps its existing meaning: `order` is the application order,
and conflict strings name the winning mod. The interfaces render the order
section from their own display list and mark skipped entries by label, so a user
reads the list they arranged rather than its reverse. The GUI log keeps the
engine report and prepends one line naming the direction.

The CLI is untouched. Its sort stays alphabetical, its report stays in
application order, and the docs explain the difference.

## Architecture

### Core

Add a report type without an output path:

```rust
pub struct CombineReport {
    pub merged: usize,
    pub files: usize,
    pub order: Vec<String>,
    pub field_conflicts: Vec<String>,
    pub pass_through_conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
}
```

`files` is the merge's total output-file count: merged datatables plus
pass-through entries, matching `CombineRun.files`.

Add:

```rust
pub fn probe(
    base_pak: &Path,
    mod_paks: &[PathBuf],
    options: &CombineOptions,
) -> Result<CombineReport, String>;
```

Refactor `run_to_dir` and `probe` onto one private helper that:

1. opens the base archive,
2. opens every mod archive in the given order,
3. builds the labels,
4. calls `combine_sources_with` with `defer_passthrough: true`,
5. returns the outcome, the labels and the archives.

`run_to_dir` then writes the staging tree and pak as it does today. `probe`
converts the outcome to a report and returns without writing an output pak or a
staging tree. The probe cannot disagree with the merge because they share this
helper and the same `combine_sources_with` call.

`CombineOptions` does not need an order flag. Order is the slice order.

### Desktop app

`MergeTab` holds:

```rust
pub inputs: Vec<PathBuf>,          // display order
pub direction: PriorityDirection,  // LaterWins default
```

`add_paths` appends instead of sorting. Duplicate paths are still skipped. The
"Add a folder" button sorts its collected pak list before appending, because a
bulk folder import has no user order and `read_dir` order is not stable;
individual files and drops append in the order given.

Pure helpers cover the logic and are unit tested:

```rust
fn move_up(&mut self, index: usize);
fn move_down(&mut self, index: usize);
fn application_inputs(&self) -> Vec<PathBuf>;
```

The application inputs are the display inputs, or their reverse when the switch
is EarlierWins.

Each row shows an index, the file name, its size, a drag handle, up and down
buttons, and a remove button. The first row's up button and the last row's down
button are disabled. The winning end carries a "wins" badge. The switch and the
caption sit above the list.

Drag uses egui 0.37's `dnd_drag_source` and `dnd_drop_zone`. If row-level drop
zones fight the scroll area, the up and down buttons remain the complete
fallback and drag is optional; the order logic is the same either way.

The merge button asks the tab for `application_inputs()` and passes that to
`Job::Merge`.

The probe runs on its own thread, not through the application's single job
slot, so a probe never disables the Merge button. `MergeTab` owns:

```rust
struct ProbeState {
    generation: u64,
    due: Option<Instant>,
    receiver: Option<Receiver<Result<CombineReport, String>>>,
    result: Option<CombineReport>,
    error: Option<String>,
}
```

A change to the list or the switch sets `due = now + 400ms` and increments the
generation. When the deadline passes and no probe is running, the tab spawns a
thread with the base pak and the current application inputs. The thread extracts
archives into temporary directories with the same helper the merge job uses,
calls `core::combine::probe`, and sends the result. The tab polls the receiver
each frame; a result whose generation is stale is discarded. Temporary
directories are removed when the thread finishes, success or error.

The probe result appears in its own panel: counts for field conflicts,
pass-through conflicts and warnings, each expandable into the list. Errors and
"nothing to combine" appear there too and never block merging. A "Check now"
button clears the debounce and starts a probe immediately; it is disabled while
one is running. The merge report continues to go to the log.

`job.rs` factors the current archive extraction out of the merge job into a
helper that both the merge job and the probe thread call:

```rust
pub(crate) fn prepare_inputs(
    inputs: &[PathBuf],
) -> Result<(Vec<PathBuf>, TempDirs), String>;
```

`prepare_inputs` walks the inputs in the given order. A pak is pushed as it is;
an archive is extracted and its paks are sorted by path before being appended,
so one archive is deterministic while archives keep their block order. The
helper does not call `paks.sort()` globally; the existing sort at `job.rs:485`
is deleted. The 29-pak gate passes its paks alphabetically, so its hash is
unchanged.

The merge job keeps its existing `TempDirs` lifetime and cleanup. The probe
thread owns its own `TempDirs` for the duration of the probe.

For headless testing, the GUI gains the same kind of hook as `--merge-now`:

- `--probe-now`: run the probe for the given inputs, print the report, and exit
  with `--exit-when-done`.
- `--first-wins`: derive the application order from the reversed input list, so
  the order switch is exercised without a pointer.

Both flags are test hooks, not the main interface.

### Browser

`web/index.html` gains:

- a direction switch above the list,
- list rows with a drag handle, index, name, size, up and down buttons, and a
  remove button,
- a probe panel with a summary and lists for conflicts, pass-through conflicts
  and warnings.

`web/app.js`:

- `modFiles` is the display order. The two `localeCompare` sorts are removed.
- Each file entry carries a stable session index, assigned when the session is
  built. The session's `mods` vector stays in that canonical order and never
  moves when the list is reordered; `applicationIndices()` returns the
  canonical indices in application order, always a permutation of `0..n`.
  Adding or removing a mod rebuilds the session and reassigns the indices.
- `direction` is `'last'` or `'first'`; the application order is the display
  order or its reverse.
- One `MergeSession` is kept per mod-set and game-pak pair. It is created on the
  first probe or merge, holds the mod bytes read once in canonical order, and
  reuses the cached base-pak regions. Adding or removing a mod, or changing the
  game pak, invalidates the session. After a successful merge the session is
  freed, matching today's memory behavior; the next probe or merge reads the
  files again.
- Before every probe or merge the page calls `session.set_order(...)` with the
  canonical application indices.
- A reorder or a switch change sets a 400 ms debounce. The probe loop calls
  `probe_step`, answers `need` with `readRange` plus `provide`, and stops on a
  stale generation. It yields once before the first step so the "Checking"
  state can paint.
- A "Check conflicts" button clears the debounce and runs a probe immediately;
  it is disabled while one is in flight.
- `showReport` renders the order section from `modFiles` and marks skipped
  entries from `report.skipped`, rather than rendering `report.order` directly.
- The probe panel is separate from the merge result panel.

`crates/modman-wasm`:

```rust
impl MergeSession {
    pub fn set_order(&mut self, order: Vec<u32>) -> Result<(), JsValue>;
    pub fn probe_step(&mut self) -> Result<JsValue, JsValue>;
}
```

`set_order` validates a permutation of `0..mods.len()` and stores it.
`probe_step` uses the same need/provide protocol as `step`, but returns
`{status:"done", report}` with no `pak` field and never packs an output. The
shared internals become
`try_merge(&self, pack: bool) -> Result<(Option<Vec<u8>>, String), MergeErr>`;
`step` passes true and returns an internal error if the pak is absent,
`probe_step` passes false and returns the report alone. A native unit test
covers permutation validation.

The single-file build inlines the new HTML, CSS and JavaScript through the
existing builder; no change is needed there. The script tags stay at
`?v=0.6.0` because this ships inside 0.6.0.

## Probe semantics and cost

The probe runs the same `combine_sources_with` as a merge. It reads every
datatable override and the base tables. Pass-through entries are recorded by
path and source index without reading their bytes, because `defer_passthrough`
is set. It writes no output staging tree and no output pak; in the desktop
app, archive extraction for a probe uses temporary directories that are removed
when the probe ends.

Both `probe` and `run_to_dir` receive the same `CombineOptions` from a caller
(the desktop app passes `CombineOptions::default()` to both) and both force
`defer_passthrough: true`. `probe` returns the same "nothing to combine" error
as `run_to_dir` when the outcome has no datatables and no pass-through entries;
the panel renders that message as a state.

A probe is therefore not free on a large set. It is the datatable half of a
merge without the multi-gigabyte write. The 400 ms debounce exists so a burst of
drags produces one probe, not one per frame. The implementation measures a
29-pak probe against a full 29-pak merge on this machine and records both
numbers in the CHANGELOG and the final report. The spinner and the "Checking"
state cover the wait; a generation counter prevents a late result from
replacing a newer list.

The browser is single-threaded. A single `probe_step` call can block while a
large datatable parses, so the page yields before the first step and never
starts a second probe while one is in flight. A merge click marks the probe
stale and waits for the current step to return.

## Error handling

- A probe error is shown in the probe panel; it never changes the merge
  availability and never replaces a previous successful result.
- No base pak in the desktop app, or no game pak in the browser, disables
  the probe and shows the same reason the merge button is disabled.
- A merge error is unchanged.
- An empty input list disables the merge and clears the probe panel.
- An archive that fails to extract fails the probe with that archive's message;
  it does not abort a later merge attempt, which retries extraction.
- A probe of a set with no datatables and no pass-through entries returns the
  merge's "nothing to combine" error and shows it in the panel.
- A stale probe result is dropped silently.
- A session with a changed mod set is rebuilt; old bytes are released.

## Testing

### Core

- `probe` and `run_to_dir` produce equal reports for the same pak list and
  order: `merged`, `files`, `order`, all three conflict lists and `skipped`.
- Reversing the pak list changes the winning value for a known conflicting
  fixture, and the report's `order` reflects the reversed input.
- A set with no overrides returns the same "nothing to combine" error as
  `run_to_dir`.
- `probe` creates no output directory contents and no pak.
- Corpus tests stay gated on the corpus directory, as the repository requires.

### Desktop app

- Unit tests for `move_up`, `move_down`, `application_inputs` in both
  directions, including the first and last boundaries.
- Headless: `modman-gui <two conflicting paks> --probe-now --exit-when-done`
  prints a report with the expected winner; `--first-wins` flips the winner in
  the same probe.
- A headless merge with an explicit input order writes a pak whose conflicting
  field has the expected winner. The 29-pak merge hash gate stays; its inputs
  are already alphabetical, so removing the sort must not change it.
- Folder import sorts the collected paks; two archives keep their blocks in
  input order, with each archive's paks alphabetical inside the block.

### Browser

- Playwright adds two conflicting paks, waits for the probe, asserts the
  conflict winner, clicks the first row's down button, waits for the new probe,
  and asserts the winner flipped. A reorder after the session exists must move
  the same mods, proving the canonical index mapping.
- The same test with the direction switch flipped asserts the same flip without
  moving a row.
- A merge after the reorder reports the same winner as the probe, and the
  existing four-pak byte-identity gate still hashes to the CLI output.

### Gates

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked --
--deny warnings`, `cargo test --workspace`, the 29-pak combine hash, the web
byte-identity test, and the GUI headless probe. The Windows test leg is part of
the gate; the audio test failure of 2026-10-28 was a platform-separator
assertion, and new path assertions must compare `Path` values rather than
rendered strings.

## Documentation and release

- `README.md`: a "Choosing the merge order" section for the desktop app and the
  browser, with the switch, the probe and the note that the CLI sorts
  alphabetically.
- `docs/faq.md`: why the CLI still sorts, what the probe does, why a probe can
  take a moment on a large set.
- `docs/command-line-walkthrough.md`: a short note on the CLI order and a
  pointer to the graphical order controls.
- `docs/merging-in-the-browser.md`: the list, the switch and the probe panel.
- The page's help text.
- `CHANGELOG.md`: in the unreleased 0.6.0 section, add an `### Added` block for
  the ordered lists, the direction switch and the probe; add a `### Changed`
  line that the two interfaces no longer sort; update the intro sentence so the
  minor number covers the new interface feature as well as the removed
  compression flag. The script tags stay `?v=0.6.0`.
- The manifest stays at 0.5.1. The release dispatch's `bump=minor` computes
  0.6.0 and writes `Cargo.toml` and `Cargo.lock`; local feature builds report
  0.5.1, while the release build runs from the bump commit and reports 0.6.0.
  No separate version commit is part of this feature.

## Acceptance criteria

- [ ] The desktop app and the browser both show the inputs in the order the
      user arranged, with drag, up and down, and remove.
- [ ] Moving a row or flipping the direction changes which mod wins a
      conflicting field, with no file renaming.
- [ ] The direction switch is honored by both the probe and the merge, and the
      winning end is marked.
- [ ] The probe runs automatically after a reorder, is debounced, can be
      refreshed with a button in both interfaces, and never blocks the Merge
      button in the desktop app.
- [ ] The probe writes no output pak and no staging tree.
- [ ] The probe report and the merge report agree for the same order.
- [ ] The CLI behavior and report are unchanged.
- [ ] The 29-pak hash gate, the web byte-identity gate and the existing test
      suite pass.
- [ ] README, FAQ, walkthrough, browser guide and CHANGELOG are updated.
- [ ] The change is folded into the unreleased 0.6.0 section before publish.

## Risks and mitigations

- **egui drag-and-drop friction.** Use `dnd_drag_source`/`dnd_drop_zone`; the
  up and down buttons are the complete fallback and the order logic is shared.
- **Probe cost on a 29-pak set.** Measure before claiming a number; debounce;
  show a checking state; keep the merge button free.
- **Browser memory.** The probe session holds mod bytes, like a merge. Free it
  after a merge and rebuild on a changed set; the page already treats a merge
  as the terminal memory-heavy step.
- **Stale results.** A generation counter on both interfaces; a late result is
  dropped, not rendered.
- **Confusing report order.** The interfaces render their display list and
  caption the direction; the engine report keeps application order and the CLI
  is unchanged.
- **Archive cleanup.** The probe thread owns its temporary directories and
  removes them on every path; the existing `TempDirs` drop behavior is reused.

## File map

- `crates/modman-core/src/combine.rs`
- `crates/modman-wasm/src/lib.rs`
- `crates/modman-gui/src/tabs/merge.rs`
- `crates/modman-gui/src/job.rs`
- `crates/modman-gui/src/main.rs`
- `web/index.html`
- `web/app.js`
- `web/style.css`
- `docs/faq.md`
- `docs/command-line-walkthrough.md`
- `docs/merging-in-the-browser.md`
- `README.md`
- `CHANGELOG.md`
