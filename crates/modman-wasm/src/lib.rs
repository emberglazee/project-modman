//! modman-wasm — browser (wasm) frontend for modman's no-metadata merge.
//!
//! The page reads the user's game pak in-place via JS `File.slice` and feeds
//! the byte ranges this module asks for; everything stays local. The merge
//! itself runs `modman_core::combine::combine_sources` — the exact same
//! semantics as the CLI.

use std::cell::RefCell;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::rc::Rc;

use modman_core::combine::{combine_sources, PakSource};
use wasm_bindgen::prelude::*;

/// A Read+Seek over a sparse set of provided byte regions of a big file.
/// Reads outside the provided regions fail with an io error and record the
/// missing range, so the JS side can fetch it and retry.
struct SparseReader {
    size: u64,
    regions: Vec<(u64, Vec<u8>)>,
    missing: Rc<RefCell<Option<(u64, u64)>>>,
    pos: u64,
}

impl SparseReader {
    fn whole(bytes: Vec<u8>) -> Self {
        let size = bytes.len() as u64;
        SparseReader {
            size,
            regions: vec![(0, bytes)],
            missing: Rc::new(RefCell::new(None)),
            pos: 0,
        }
    }

    fn region_containing(&self, off: u64, len: usize) -> Option<&[u8]> {
        self.regions.iter().find_map(|(start, data)| {
            let end = start + data.len() as u64;
            if off >= *start && off + len as u64 <= end {
                let s = (off - start) as usize;
                Some(&data[s..s + len])
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
        let want = len.min((self.size - self.pos) as usize);
        if let Some(slice) = self.region_containing(self.pos, want) {
            buf[..want].copy_from_slice(slice);
            self.pos += want as u64;
            Ok(want)
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
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "sparse region missing",
            ))
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
}

enum MergeErr {
    Missing(u64, u64),
    Other(String),
}

fn map_err(e: repak::Error) -> MergeErr {
    MergeErr::Other(e.to_string())
}

/// A merge session: add mod paks, provide game-pak byte ranges, step until
/// done. All state lives here between JS calls.
#[wasm_bindgen]
pub struct MergeSession {
    mods: Vec<(String, Vec<u8>)>,
    game_size: u64,
    regions: Rc<RefCell<Vec<(u64, Vec<u8>)>>>,
    missing: Rc<RefCell<Option<(u64, u64)>>>,
}

#[wasm_bindgen]
impl MergeSession {
    #[wasm_bindgen(constructor)]
    pub fn new() -> MergeSession {
        MergeSession {
            mods: Vec::new(),
            game_size: 0,
            regions: Rc::new(RefCell::new(Vec::new())),
            missing: Rc::new(RefCell::new(None)),
        }
    }

    /// Add a mod pak (label = filename shown in the report).
    pub fn add_mod(&mut self, label: String, bytes: Vec<u8>) {
        self.mods.push((label, bytes));
    }

    /// Tell the session how big the game pak is (bytes).
    pub fn set_game_pak(&mut self, size: f64) {
        self.game_size = size as u64;
    }

    /// Provide a byte range read from the game pak by the JS side.
    pub fn provide(&mut self, offset: f64, bytes: Vec<u8>) {
        let off = offset as u64;
        let mut regions = self.regions.borrow_mut();
        regions.push((off, bytes));
        // Keep regions sorted by offset for deterministic lookup.
        regions.sort_by_key(|(o, _)| *o);
    }

    /// Run (or resume) the merge. Returns a JS object:
    ///   {status:"need", offset, length}  — fetch that range and provide() it
    ///   {status:"done", pak: Uint8Array, report: <json string>}
    pub fn step(&mut self) -> Result<JsValue, JsValue> {
        *self.missing.borrow_mut() = None;
        match self.try_merge() {
            Ok((pak, report)) => {
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"status".into(), &"done".into());
                let arr = js_sys::Uint8Array::from(&pak[..]);
                let _ = js_sys::Reflect::set(&obj, &"pak".into(), &arr.into());
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

    fn try_merge(&self) -> Result<(Vec<u8>, String), MergeErr> {
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

        // 2. The mod paks (whole files in memory).
        let mut mod_cells: Vec<RefCell<SparseReader>> = Vec::new();
        let mut mod_paks: Vec<repak::PakReader> = Vec::new();
        for (_, bytes) in &self.mods {
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
        let labels: Vec<String> = self.mods.iter().map(|(l, _)| l.clone()).collect();

        // 3. The shared combine.
        let outcome = match combine_sources(&base, &src_refs, &labels) {
            Ok(o) => o,
            Err(e) => {
                if let Some((s, e2)) = *self.missing.borrow() {
                    return Err(MergeErr::Missing(s, e2));
                }
                return Err(MergeErr::Other(e.to_string()));
            }
        };

        // 4. Write the merged V3 pak (same params as the CLI).
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = repak::PakBuilder::new().writer(
                &mut cursor,
                repak::Version::V3,
                "../../../".to_string(),
                None,
            );
            let mut paths: Vec<&String> = outcome.files.keys().collect();
            paths.sort();
            for p in paths {
                let data = outcome.files.get(p).unwrap().clone();
                writer.write_file(p, false, data).map_err(map_err)?;
            }
            writer.write_index().map_err(map_err)?;
        }

        // 5. Report JSON.
        let report = serde_json::json!({
            "merged": outcome.merged,
            "files": outcome.files.len(),
            "order": labels,
            "conflicts": outcome.field_conflicts,
            "passThroughConflicts": outcome.pass_through_conflicts,
        });
        Ok((cursor.into_inner(), report.to_string()))
    }
}

impl Default for MergeSession {
    fn default() -> Self {
        Self::new()
    }
}
