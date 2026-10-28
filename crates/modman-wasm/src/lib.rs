//! modman-wasm — browser (wasm) frontend for modman's no-metadata merge.
//!
//! The page reads the user's game pak in-place via JS `File.slice` and feeds
//! the byte ranges this module asks for; everything stays local. The merge
//! itself runs `modman_core::combine::combine_sources` — the exact same
//! semantics as the CLI.

use std::cell::RefCell;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::rc::Rc;

use modman_core::combine::{combine_sources_with, CombineOptions, PakSource};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// Print to the browser console (js_sys-only, no web-sys dependency).
fn console_error(msg: &str) {
    let global = js_sys::global();
    if let Ok(console) = js_sys::Reflect::get(&global, &"console".into()) {
        if let Ok(err) = js_sys::Reflect::get(&console, &"error".into()) {
            if let Some(f) = err.dyn_ref::<js_sys::Function>() {
                let _ = f.call1(&console, &JsValue::from_str(msg));
            }
        }
    }
}

thread_local! {
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Report panics with their real message instead of a bare "unreachable"
/// trap, and remember the message for `last_panic()`.
#[wasm_bindgen(start)]
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = info.to_string();
        LAST_PANIC.with(|slot| *slot.borrow_mut() = Some(message.clone()));
        console_error(&format!(
            "modman-wasm panicked: {message}\n\
             (please report this with the mod names used at \
             https://github.com/emberglazee/project-modman/issues)"
        ));
    }));
}

/// The last panic message, if this module trapped. JS reads it after a
/// `RuntimeError` to tell a real panic from an out-of-memory abort.
#[wasm_bindgen]
pub fn last_panic() -> Option<String> {
    LAST_PANIC.with(|slot| slot.borrow().clone())
}

/// Clear the remembered panic before a new operation.
#[wasm_bindgen]
pub fn clear_last_panic() {
    LAST_PANIC.with(|slot| *slot.borrow_mut() = None);
}

/// Provided byte regions of the game pak, sorted by offset.
type RegionMap = Rc<RefCell<Vec<(u64, Rc<Vec<u8>>)>>>;
/// The range the sparse reader is currently missing (if any).
type MissingSlot = Rc<RefCell<Option<(u64, u64)>>>;

/// A Read+Seek over a sparse set of provided byte regions of a big file.
/// Reads outside the provided regions fail with an io error and record the
/// missing range, so the JS side can fetch it and retry.
struct SparseReader {
    size: u64,
    regions: Vec<(u64, Rc<Vec<u8>>)>,
    missing: MissingSlot,
    pos: u64,
}

impl SparseReader {
    fn whole(bytes: Rc<Vec<u8>>) -> Self {
        let size = bytes.len() as u64;
        SparseReader {
            size,
            regions: vec![(0, bytes)],
            missing: Rc::new(RefCell::new(None)),
            pos: 0,
        }
    }

    /// The provided bytes starting at `off`, however many there are. A read
    /// that spans two provided regions returns the first region's tail; the
    /// next call continues in the second.
    fn region_at(&self, off: u64) -> Option<&[u8]> {
        self.regions.iter().find_map(|(start, data)| {
            let end = start + data.len() as u64;
            if off >= *start && off < end {
                Some(&data[(off - start) as usize..])
            } else {
                None
            }
        })
    }
}

impl Read for SparseReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let len = buf.len();
        if len == 0 {
            return Ok(0);
        }
        if self.pos >= self.size {
            return Ok(0);
        }
        // Do the clamp in u64: casting the remaining length to usize truncates
        // a 17 GB game pak on wasm32.
        let remaining = self.size - self.pos;
        let want = len.min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if let Some(slice) = self.region_at(self.pos) {
            let n = want.min(slice.len());
            buf[..n].copy_from_slice(&slice[..n]);
            self.pos += n as u64;
            Ok(n)
        } else {
            // Record the missing range (coalesced; min 64 KiB window so we
            // never ask the browser for tiny reads).
            let start = self.pos;
            let mut end = start + (want.max(65536) as u64);
            if end > self.size {
                end = self.size;
            }
            let mut m = self.missing.borrow_mut();
            *m = Some(match *m {
                Some((s, e)) => (s.min(start), e.max(end)),
                None => (start, end),
            });
            Err(std::io::Error::other("sparse region missing"))
        }
    }
}

impl Seek for SparseReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.pos = match pos {
            SeekFrom::Start(o) => o,
            SeekFrom::End(o) => (self.size as i64 + o).max(0) as u64,
            SeekFrom::Current(o) => (self.pos as i64 + o).max(0) as u64,
        };
        Ok(self.pos)
    }
}

/// `PakSource` over a repak reader (sparse or whole-file).
struct RepakSource<'a> {
    pak: &'a repak::PakReader,
    file: &'a RefCell<SparseReader>,
    files: Vec<String>,
}

impl PakSource for RepakSource<'_> {
    fn files(&self) -> Vec<String> {
        self.files.clone()
    }
    fn read(&self, path: &str) -> Result<Vec<u8>, modman_core::apply::ApplyError> {
        let mut f = self.file.borrow_mut();
        self.pak
            .get(path, &mut *f)
            .map_err(|e| modman_core::apply::ApplyError::Asset(e.to_string()))
    }
    fn mount_point(&self) -> String {
        self.pak.mount_point().to_string()
    }
}

enum MergeErr {
    Missing(u64, u64),
    Other(String),
}

fn map_err(e: repak::Error) -> MergeErr {
    MergeErr::Other(e.to_string())
}

/// Validate that `order` is a permutation of `0..len` and return it as usize.
fn validate_order(len: usize, order: &[u32]) -> Result<Vec<usize>, String> {
    if order.len() != len {
        return Err(format!(
            "order must list {len} entries, got {}",
            order.len()
        ));
    }
    let mut seen = vec![false; len];
    let mut out = Vec::with_capacity(len);
    for &i in order {
        let i = i as usize;
        if i >= len {
            return Err(format!("order entry {i} is out of range"));
        }
        if std::mem::replace(&mut seen[i], true) {
            return Err(format!("order entry {i} appears twice"));
        }
        out.push(i);
    }
    Ok(out)
}

/// A merge session: add mod paks, provide game-pak byte ranges, step until
/// done. All state lives here between JS calls.
#[wasm_bindgen]
pub struct MergeSession {
    mods: Vec<(String, Rc<Vec<u8>>)>,
    order: Option<Vec<usize>>,
    game_size: u64,
    regions: RegionMap,
    missing: MissingSlot,
}

#[wasm_bindgen]
impl MergeSession {
    #[wasm_bindgen(constructor)]
    pub fn new() -> MergeSession {
        MergeSession {
            mods: Vec::new(),
            order: None,
            game_size: 0,
            regions: Rc::new(RefCell::new(Vec::new())),
            missing: Rc::new(RefCell::new(None)),
        }
    }

    /// Add a mod pak (label = filename shown in the report).
    pub fn add_mod(&mut self, label: String, bytes: Vec<u8>) {
        // Rc so the per-merge readers share these bytes instead of cloning
        // them (halves peak memory for big mod paks in the browser).
        self.mods.push((label, Rc::new(bytes)));
        self.order = None;
    }

    /// Set the order the mods are applied in. `order` must be a permutation
    /// of `0..mods.len()`; the bytes stay put, so reordering re-reads nothing.
    pub fn set_order(&mut self, order: Vec<u32>) -> Result<(), JsValue> {
        let order = validate_order(self.mods.len(), &order).map_err(|e| JsValue::from_str(&e))?;
        self.order = Some(order);
        Ok(())
    }

    /// Tell the session how big the game pak is (bytes).
    pub fn set_game_pak(&mut self, size: f64) {
        self.game_size = size as u64;
    }

    /// Provide a byte range read from the game pak by the JS side.
    pub fn provide(&mut self, offset: f64, bytes: Vec<u8>) {
        let off = offset as u64;
        let mut regions = self.regions.borrow_mut();
        regions.push((off, Rc::new(bytes)));
        // Keep regions sorted by offset for deterministic lookup.
        regions.sort_by_key(|(o, _)| *o);
    }

    /// Run (or resume) the merge. Returns a JS object:
    ///   {status:"need", offset, length}  — fetch that range and provide() it
    ///   {status:"done", pak: Uint8Array, report: <json string>}
    pub fn step(&mut self) -> Result<JsValue, JsValue> {
        self.step_with(true)
    }

    /// Run (or resume) the conflict probe. Same need/provide protocol as
    /// `step`, but the done object carries only `report`; no pak is written.
    pub fn probe_step(&mut self) -> Result<JsValue, JsValue> {
        self.step_with(false)
    }

    fn step_with(&mut self, pack: bool) -> Result<JsValue, JsValue> {
        *self.missing.borrow_mut() = None;
        // A panic here aborts the wasm module (panic = abort); the hook records
        // the message for `last_panic()`, which JS reads after the trap.
        match self.try_merge(pack) {
            Ok((pak, report)) => {
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"status".into(), &"done".into());
                if let Some(pak) = pak {
                    let arr = js_sys::Uint8Array::from(&pak[..]);
                    let _ = js_sys::Reflect::set(&obj, &"pak".into(), &arr.into());
                }
                let _ = js_sys::Reflect::set(&obj, &"report".into(), &report.into());
                Ok(obj.into())
            }
            Err(MergeErr::Missing(start, end)) => {
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"status".into(), &"need".into());
                let _ =
                    js_sys::Reflect::set(&obj, &"offset".into(), &JsValue::from_f64(start as f64));
                let _ = js_sys::Reflect::set(
                    &obj,
                    &"length".into(),
                    &JsValue::from_f64((end - start) as f64),
                );
                Ok(obj.into())
            }
            Err(MergeErr::Other(e)) => Err(JsValue::from_str(&e)),
        }
    }

    fn try_merge(&self, pack: bool) -> Result<(Option<Vec<u8>>, String), MergeErr> {
        if self.mods.is_empty() {
            return Err(MergeErr::Other("no mod paks given".to_string()));
        }

        // 1. The game pak over the sparse regions.
        let sparse = SparseReader {
            size: self.game_size,
            regions: self.regions.borrow().clone(),
            missing: self.missing.clone(),
            pos: 0,
        };
        let sparse_cell = RefCell::new(sparse);
        let base_pak = {
            let mut b = sparse_cell.borrow_mut();
            repak::PakBuilder::new().reader(&mut *b).map_err(|e| {
                if let Some((s, e2)) = *self.missing.borrow() {
                    return MergeErr::Missing(s, e2);
                }
                map_err(e)
            })?
        };
        let base = RepakSource {
            pak: &base_pak,
            file: &sparse_cell,
            files: base_pak.files(),
        };

        // 2. The mod paks (whole files in memory), in the caller's order.
        let indices: Vec<usize> = self
            .order
            .clone()
            .unwrap_or_else(|| (0..self.mods.len()).collect());
        let mut mod_cells: Vec<RefCell<SparseReader>> = Vec::new();
        let mut mod_paks: Vec<repak::PakReader> = Vec::new();
        for &index in &indices {
            let (_, bytes) = &self.mods[index];
            let cell = RefCell::new(SparseReader::whole(bytes.clone()));
            let pak = {
                let mut b = cell.borrow_mut();
                repak::PakBuilder::new().reader(&mut *b).map_err(map_err)?
            };
            mod_cells.push(cell);
            mod_paks.push(pak);
        }
        let mod_srcs: Vec<RepakSource> = mod_paks
            .iter()
            .zip(mod_cells.iter())
            .map(|(pak, cell)| RepakSource {
                pak,
                file: cell,
                files: pak.files(),
            })
            .collect();
        let src_refs: Vec<&dyn PakSource> = mod_srcs.iter().map(|s| s as &dyn PakSource).collect();
        let labels: Vec<String> = indices.iter().map(|&i| self.mods[i].0.clone()).collect();

        // 3. The shared combine. A probe records pass-through files without
        // reading their bytes; a merge needs them for the output pak.
        let options = CombineOptions {
            datatables_only: false,
            defer_passthrough: !pack,
        };
        let mut outcome = match combine_sources_with(&base, &src_refs, &labels, &options) {
            Ok(o) => o,
            Err(e) => {
                if let Some((s, e2)) = *self.missing.borrow() {
                    return Err(MergeErr::Missing(s, e2));
                }
                return Err(MergeErr::Other(e.to_string()));
            }
        };

        // 4. Report JSON, before the pak writer consumes the file map.
        let file_count = outcome.files.len() + outcome.pass_through.len();
        if !pack && file_count == 0 {
            return Err(MergeErr::Other(
                "nothing to combine (no conflicting overrides found)".to_string(),
            ));
        }
        let report = serde_json::json!({
            "merged": outcome.merged,
            "files": file_count,
            "order": labels,
            "conflicts": outcome.field_conflicts,
            "passThroughConflicts": outcome.pass_through_conflicts,
            "warnings": outcome.warnings,
            "skipped": outcome.skipped,
        });
        if !pack {
            return Ok((None, report.to_string()));
        }

        // 5. Write the merged V3 pak (same params as the CLI). The file map is
        // consumed, not cloned: the browser holds every byte in wasm memory, so
        // a second copy of the output is fatal on the same sets.
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = repak::PakBuilder::new().writer(
                &mut cursor,
                repak::Version::V3,
                "../../../".to_string(),
                None,
            );
            let files = std::mem::take(&mut outcome.files);
            for (p, data) in files {
                writer.write_file(&p, false, data).map_err(map_err)?;
            }
            writer.write_index().map_err(map_err)?;
        }
        Ok((Some(cursor.into_inner()), report.to_string()))
    }
}

impl Default for MergeSession {
    fn default() -> Self {
        Self::new()
    }
}

// ── Translations ─────────────────────────────────────────────────────────
// Locres work is small-file work, so it fits the browser: nothing here needs a
// sparse read of the game pak, and no file ever leaves the page.

/// Convert a `.locres` file to CSV (one row per string).
#[wasm_bindgen]
pub fn locres_to_csv(bytes: &[u8]) -> Result<String, JsValue> {
    let parsed = modman_core::locres::LocresFile::parse(bytes)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(parsed.to_csv())
}

/// A patched locres plus how many strings the CSV actually changed.
#[wasm_bindgen]
pub struct LocresPatch {
    bytes: Vec<u8>,
    patched: usize,
}

#[wasm_bindgen]
impl LocresPatch {
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn patched(&self) -> usize {
        self.patched
    }
}

/// Apply translated strings from a CSV to a `.locres`, keeping the file's
/// namespaces, hashes and order intact.
#[wasm_bindgen]
pub fn locres_patch(bytes: &[u8], csv: &str) -> Result<LocresPatch, JsValue> {
    let mut parsed = modman_core::locres::LocresFile::parse(bytes)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    let patched = parsed.patch_from_csv(csv);
    Ok(LocresPatch {
        bytes: parsed.write(),
        patched,
    })
}

/// Differences between two `.locres` files, for translators chasing a new game
/// build.
#[wasm_bindgen]
pub fn locres_diff(a: &[u8], b: &[u8]) -> Result<String, JsValue> {
    let fa =
        modman_core::locres::LocresFile::parse(a).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let fb =
        modman_core::locres::LocresFile::parse(b).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let diff = fa.diff(&fb);
    Ok(format!(
        "{} changed, {} added, {} removed",
        diff.changed.len(),
        diff.added.len(),
        diff.removed.len()
    ))
}

#[cfg(test)]
mod order_tests {
    use super::validate_order;

    #[test]
    fn order_must_be_a_permutation() {
        assert_eq!(validate_order(3, &[2, 0, 1]).unwrap(), vec![2, 0, 1]);
        assert!(validate_order(3, &[0, 1]).is_err(), "wrong length");
        assert!(validate_order(3, &[0, 1, 3]).is_err(), "out of range");
        assert!(validate_order(3, &[0, 1, 1]).is_err(), "duplicate");
    }
}
