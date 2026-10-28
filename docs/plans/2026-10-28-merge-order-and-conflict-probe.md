# Merge order lists and conflict probe: implementation plan

> **For Hermes:** implement in order. Core and wasm first; the two frontends can
> then run in parallel because they share only the core contract. Verify every
> gate on the real corpus, not on a child's report.

**Goal:** Give the desktop app and the browser an explicit, reorderable merge
list with a Later/Earlier-wins switch and a debounced conflict probe, folded
into the unreleased 0.6.0, without changing the CLI.

**Architecture:** `core::combine::probe` shares one open-and-combine helper with
`run_to_dir`, so the probe and the merge cannot disagree. The desktop app owns a
probe thread outside its single job slot; the browser reuses one `MergeSession`
with `set_order` and `probe_step`. The frontends derive application order from
their display list; the engine's later-applied-wins rule is untouched.

**Spec:** `docs/superpowers/specs/2026-10-28-merge-order-and-conflict-probe-design.md`

**Tech:** Rust 2021, egui/eframe 0.37, wasm-bindgen, vanilla JS, playwright.

---

## Preflight

- [ ] `git status --short` is clean and `git log -1` shows the approved spec.
- [ ] Record the baseline gates before touching code:
  - `cargo test --workspace`
  - 29-pak CLI merge: `sha256 555dca0d12d2ac24…`, 4,607,949,935 B
  - GUI 7z merge: `sha256 534a8e19…`, 77,324 B
  - wasm four-pak merge: `sha256 101ce6d6…`, 88,193 B
- [ ] Confirm the 29-pak gate's argv is lexical (`printf '%s\n' aralanche/*.pak`).

## Phase 1: Core

### Task 1: `CombineReport` and the report builder

**Files:** `crates/modman-core/src/combine.rs`

**Step 1: Write the failing test**

Add to `combine_options_tests`:

```rust
#[test]
fn report_counts_match_the_outcome() {
    let mut outcome = CombineOutcome::default();
    outcome.merged = 2;
    outcome.files.insert("a".into(), vec![1]);
    outcome.pass_through.push(("b".into(), 0, "b".into()));
    outcome.field_conflicts.push("x".into());
    let r = report_from_outcome(&outcome, vec!["m".into()]);
    assert_eq!(r.merged, 2);
    assert_eq!(r.files, 2);
    assert_eq!(r.order, vec!["m".to_string()]);
    assert_eq!(r.field_conflicts, vec!["x".to_string()]);
}
```

**Step 2: Run it and confirm it fails**

`cargo test -p modman-core report_counts_match_the_outcome`

**Step 3: Implement**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombineReport {
    pub merged: usize,
    pub files: usize,
    pub order: Vec<String>,
    pub field_conflicts: Vec<String>,
    pub pass_through_conflicts: Vec<String>,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
}

fn report_from_outcome(outcome: &CombineOutcome, order: Vec<String>) -> CombineReport {
    CombineReport {
        merged: outcome.merged,
        files: outcome.files.len() + outcome.pass_through.len(),
        order,
        field_conflicts: outcome.field_conflicts.clone(),
        pass_through_conflicts: outcome.pass_through_conflicts.clone(),
        warnings: outcome.warnings.clone(),
        skipped: outcome.skipped.clone(),
    }
}
```

**Step 4:** Run the test. It must pass. Then `cargo fmt --all`.

**Step 5: Commit**

`git add crates/modman-core/src/combine.rs` and
`git commit -m "feat(combine): add a report type without an output path"`.

### Task 2: One open-and-combine helper, `probe`, and `run_to_dir`

**Files:** `crates/modman-core/src/combine.rs`

**Step 1: Write the failing tests**

Add a temp-fixture helper and three tests:

- `probe_matches_run_to_dir`: pack a base pak from `DB_LevelList` vanilla and two
  mod paks from the two `mk_mod` variants in
  `different_asset_same_field_still_conflicts`; call `probe` and `run_to_dir`
  with the same `[a, b]` order; assert every `CombineReport` field equals the
  corresponding `CombineRun` field and `files` equals `run.files`.
- `probe_writes_nothing`: call `probe` with `--output`-style temp dir that does
  not exist; assert it still returns and the directory stays absent.
- `probe_empty_and_noop_errors`: empty slice returns `no mod paks given`; a set
  whose only mod is `SicarioCombine_P.pak` returns `nothing to combine`.

Pack with `modman_pak::pack(dir, out, modman_pak::Version::V3, "../../../")`.
Use a temp root named with `std::process::id()`; remove it at the end.

**Step 2: Run and confirm failure**

`cargo test -p modman-core probe_`

**Step 3: Implement**

```rust
fn open_and_combine(
    base_pak: &Path,
    mod_paks: &[PathBuf],
    options: &CombineOptions,
) -> Result<(CombineOutcome, Vec<String>, Vec<modman_pak::PakArchive>), String> {
    // open base + mods in order, build labels, call combine_sources_with with
    // defer_passthrough: true, return outcome/labels/archives
}

pub fn probe(
    base_pak: &Path,
    mod_paks: &[PathBuf],
    options: &CombineOptions,
) -> Result<CombineReport, String> {
    if mod_paks.is_empty() {
        return Err("no mod paks given".to_string());
    }
    let (outcome, labels, _archives) = open_and_combine(base_pak, mod_paks, options)?;
    let report = report_from_outcome(&outcome, labels);
    if report.files == 0 {
        return Err("nothing to combine (no conflicting overrides found)".to_string());
    }
    Ok(report)
}
```

Rewrite `run_to_dir` to call `open_and_combine`, keep the existing staging and
pack code, and build `CombineRun` from the report plus `output`. Its behavior
and output bytes must not change.

**Step 4:** `cargo test -p modman-core` and the 29-pak CLI hash gate. The hash
must stay `555dca0d…`.

**Step 5: Commit**

`feat(combine): add a probe sharing the merge's open-and-combine path`.

### Task 3: Order reversal changes the winner

**Files:** `crates/modman-core/src/combine.rs`

**Step 1:** Extend the Task 2 fixture with a same-field conflict (use the two
`mk_mod` variants from `different_asset_same_field_still_conflicts`). Call
`probe` with `[a, b]` and `[b, a]`.

**Step 2:** Assert the two reports' `order` vectors are swapped and the
conflict lines name different winners; assert `run_to_dir` for each order
matches its probe report.

**Step 3:** Run `cargo test -p modman-core order_`; expected PASS.

**Step 4: Commit** `test(combine): prove probe order and winner reversal`.

## Phase 2: Wasm contract

### Task 4: `set_order`, `probe_step`, and `try_merge(pack)`

**Files:** `crates/modman-wasm/src/lib.rs`

**Step 1:** Native unit test `set_order_rejects_bad_permutations`: duplicate,
out-of-range, and wrong-length vectors are errors; a valid permutation is
stored.

**Step 2:** Run `cargo test -p modman-wasm set_order`; confirm failure.

**Step 3:** Implement:

```rust
struct MergeSession {
    mods: Vec<(String, Rc<Vec<u8>>)>,
    order: Option<Vec<usize>>,
    // existing fields
}

pub fn set_order(&mut self, order: Vec<u32>) -> Result<(), JsValue> { /* validate */ }
pub fn probe_step(&mut self) -> Result<JsValue, JsValue> { /* try_merge(false) */ }

fn try_merge(&self, pack: bool)
    -> Result<(Option<Vec<u8>>, String), MergeErr> { /* ordered mods; probe uses defer */ }
```

Build ordered mods and labels from `self.order` (or identity). For `pack ==
false`, use `combine_sources_with` with `defer_passthrough: true` and skip the
pak writer; return `(None, report)`. `step` keeps the existing JS shape with
`pak`; `probe_step` returns `{status:"done", report}` without `pak`.

**Step 4:** `cargo test -p modman-wasm` and
`cargo build -p modman-wasm --target wasm32-unknown-unknown`.

**Step 5: Commit** `feat(wasm): add ordered probes to the merge session`.

## Phase 3: Desktop app

### Task 5: Order state and pure helpers

**Files:** `crates/modman-gui/src/tabs/merge.rs`

- Add `PriorityDirection { LaterWins, EarlierWins }` (default `LaterWins`),
  `pub direction`, remove `self.inputs.sort()`, and add `move_up`, `move_down`,
  `application_inputs`.
- The "Add a folder" button sorts `archive::collect_paks(&dir)` before
  `add_paths`; individual files append.
- Unit tests: move at boundaries, move up/down, application order in both
  directions.

Run `cargo test -p modman-gui order_` and `cargo clippy -p modman-gui
--all-targets -- -D warnings`. Commit `feat(gui): keep the merge list in the
user's order`.

### Task 6: `prepare_inputs` without the global sort

**Files:** `crates/modman-gui/src/job.rs`

- Extract the extraction loop from `merge` into
  `pub(crate) fn prepare_inputs(inputs: &[PathBuf]) -> Result<(Vec<PathBuf>, TempDirs), String>`.
- Walk inputs in order; push paks as-is; for archives, extract, collect paks,
  sort that archive's list, extend. Delete `paks.sort()` at line 485.
- `merge` calls `prepare_inputs`; the probe thread will too.
- A unit test for the pure ordering helper is preferred; if extraction is too
  heavy, test with two archive fixtures in a temp dir.

Run the GUI headless merge gate on `aralanche/*.pak`; hash must stay
`555dca0d…`. Commit `fix(gui): preserve archive block order when merging`.

### Task 7: The order list UI

**Files:** `crates/modman-gui/src/tabs/merge.rs`

- Rows: index, name, size, drag handle (`dnd_drag_source`/`dnd_drop_zone`),
  up/down, remove; the first up and last down are disabled.
- Direction switch (`selectable_value`) and caption; "wins" badge on the
  winning end.
- A "Check now" button placeholder; it will start the probe in Task 8.
- The Merge button passes `application_inputs()`.

Run `cargo build -p modman-gui` and `cargo clippy -p modman-gui --all-targets
-- -D warnings`. Commit `feat(gui): add reorder controls and the direction
switch`.

### Task 8: Probe thread, panel, and headless flags

**Files:** `crates/modman-gui/src/tabs/merge.rs`,
`crates/modman-gui/src/main.rs`

- Add `ProbeState` with `generation`, `inflight_generation`, `due`,
  `receiver`, `result`, `error`.
- On list/switch change: `due = now + 400ms`, increment `generation`. When due
  and idle: set `inflight_generation`, spawn a thread with the base pak and
  `application_inputs`, call `prepare_inputs` then `core::combine::probe`, send
  `(generation, result)`. Poll each frame; discard stale generations; keep the
  last good result on error.
- Panel: counts and expandable lists for field conflicts, pass-through
  conflicts, warnings; "Checking" state; error banner; "Check now" starts
  immediately.
- `main.rs`: add `probe_now` and `first_wins` to `Args`/`parse_args`; set
  `merge.direction`; make the `auto_merge` path use `application_inputs()`.
- `--probe-now` runs the probe and exits 0/1 with the messages from the spec;
  with no base pak prints `No game pak found in <dir>` and exits 1.

Headless checks:

```
env -u WAYLAND_DISPLAY DISPLAY=:99 ./target/debug/modman-gui <two conflicting paks> --probe-now --exit-when-done
env -u WAYLAND_DISPLAY DISPLAY=:99 ./target/debug/modman-gui <two conflicting paks> --probe-now --first-wins --exit-when-done
env -u WAYLAND_DISPLAY DISPLAY=:99 ./target/debug/modman-gui <two conflicting paks> --merge-now --first-wins --exit-when-done --out /tmp/plan/gui-order
```

Record the probe duration and peak RSS on the 29-pak set here; the number goes
into the changelog. Commit `feat(gui): probe conflicts before merging`.

## Phase 4: Browser

### Task 9: Markup and styles

**Files:** `web/index.html`, `web/style.css`

- Replace the merge-order hint with the direction switch, caption and controls.
- Rows: drag handle, index, name, size, up/down, remove.
- Probe panel: summary, conflict lists, pass-through list, warnings, "Check
  conflicts" button, status/error.
- Keep the existing ids used by `app.js` and the single-file builder.

Commit `feat(web): add the merge order list and probe panel markup`.

### Task 10: Session and UI logic

**Files:** `web/app.js`

- `modFiles` entries become `{ file, id }`; remove both `localeCompare` sorts.
- `direction` state; `applicationIndices()` returns canonical ids in
  application order.
- `ensureSession()` keyed by game file name/size/lastModified plus each mod's
  name/size/lastModified; add mods once in canonical order; set the order before
  each probe/merge; free after a successful merge.
- Probe loop: 400 ms debounce, generation guard, `probe_step`, need/provide,
  yield before the first step, separate panel, last-good-result retention.
- Merge uses the same session/order; `showReport` renders the display list and
  marks skipped by label.

Run `node --check web/app.js`; build wasm and the single-file page; run the
existing browser byte gate. Commit `feat(web): merge in the arranged order with
a live probe`.

### Task 11: Browser reorder test

**Files:** the existing playwright harness under `~/modding/project-wingman`
(or the repo's test script if present).

- Add two conflicting paks, wait for the probe, assert the winner; click the
  first row's down button; wait; assert the flip; flip the direction switch and
  assert the same flip; merge and confirm the report's winner matches the
  probe. Keep the four-pak SHA-256 gate.

Commit `test(web): cover reorder, direction, and probe parity`.

## Phase 5: Docs and gates

### Task 12: Documentation and changelog

**Files:** `README.md`, `docs/faq.md`, `docs/command-line-walkthrough.md`,
`docs/merging-in-the-browser.md`, `web/index.html` hint, `CHANGELOG.md`

- README: "Choosing the merge order" for both apps; the switch and probe; the
  CLI still sorts alphabetically.
- FAQ: why the CLI sorts, what the probe does, why it can take a moment.
- Walkthrough and browser guide: the controls, with the CLI note.
- CHANGELOG 0.6.0: `### Added` for the ordered lists, switch and probe;
  `### Changed` for the removed frontend sorts; adjust the intro sentence.
- Do not bump the manifest; `bump=minor` does it at release time.

### Task 13: Full gates

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- --deny warnings
cargo test --workspace
cargo audit
node --check web/app.js
```

Then:

- 29-pak CLI merge hash `555dca0d…`, 4,607,949,935 B.
- GUI headless merge hash `555dca0d…` (same inputs, same order).
- GUI 7z merge `534a8e19…`, 77,324 B.
- wasm four-pak merge `101ce6d6…`, 88,193 B.
- Probe report equals merge report on the same two conflicting paks.
- CI Build and Pages green on the final commit; map the run to the SHA.

Do not dispatch a release; the user publishes.

## Risks to watch

- egui drag-and-drop: keep up/down as the complete fallback.
- Probe cost: measure before writing the changelog number; debounce stays 400 ms
  unless the measurement says otherwise.
- Browser memory: free the session after a merge; rebuild on a changed set.
- Stale results: the generation guard must be in both frontends.
- The 29-pak gate must stay lexical; sort explicitly if its source changes.
- Windows test legs: compare `Path` values, never rendered separators.
