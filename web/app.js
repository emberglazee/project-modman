// Project Modman — in-browser mod merge.
// All processing happens locally; the game pak is read in-place via File.slice.
//
// Classic script (no ES modules) so it works both:
//  - hosted: wasm_bindgen loads from pkg/modman_wasm.js and fetches the .wasm
//  - single-file / file://: the builder inlines everything and sets
//    window.MODMAN_WASM_BASE64, initialized synchronously from bytes.

const $ = (id) => document.getElementById(id);
const modFilesEl = $('modFiles');
const modDrop = $('modDrop');
const modList = $('modList');
const gameFileEl = $('gameFile');
const gameDrop = $('gameDrop');
const gameInfo = $('gameInfo');
const mergeBtn = $('mergeBtn');
const progress = $('progress');
const progressFill = $('progressFill');
const statusEl = $('status');
const errorEl = $('error');
const resultCard = $('resultCard');
const resultSummary = $('resultSummary');
const reportEl = $('report');
const downloadBtn = $('downloadBtn');

let wasmReady = false;
let modFiles = [];   // File[]
let gameFile = null; // File
let resultPak = null;

const fmtBytes = (n) => {
  if (n >= 1024 ** 3) return (n / 1024 ** 3).toFixed(2) + ' GB';
  if (n >= 1024 ** 2) return (n / 1024 ** 2).toFixed(1) + ' MB';
  if (n >= 1024) return (n / 1024).toFixed(0) + ' KB';
  return n + ' B';
};

function setError(msg) {
  errorEl.hidden = !msg;
  errorEl.textContent = msg || '';
}

function refreshUi() {
  mergeBtn.disabled = !(wasmReady && modFiles.length > 0 && gameFile);
  mergeBtn.textContent = modFiles.length
    ? `Merge ${modFiles.length} mod pak${modFiles.length > 1 ? 's' : ''}`
    : 'Merge mods';
}

function renderModList() {
  modList.innerHTML = '';
  const sorted = [...modFiles].sort((a, b) => a.name.localeCompare(b.name));
  for (const f of sorted) {
    const li = document.createElement('li');
    li.innerHTML = `<span class="fname">${f.name}</span><span class="fsize">${fmtBytes(f.size)}</span>`;
    modList.appendChild(li);
  }
}

// ── File picking ──────────────────────────────────────────────────────────

function addModFiles(files) {
  const incoming = [...files].filter((f) => f.name.toLowerCase().endsWith('.pak'));
  for (const f of incoming) {
    if (!modFiles.some((m) => m.name === f.name && m.size === f.size)) modFiles.push(f);
  }
  renderModList();
  refreshUi();
}

modFilesEl.addEventListener('change', () => addModFiles(modFilesEl.files));

for (const [zone, handler] of [[modDrop, addModFiles], [gameDrop, null]]) {
  zone.addEventListener('dragover', (e) => { e.preventDefault(); zone.classList.add('over'); });
  zone.addEventListener('dragleave', () => zone.classList.remove('over'));
  zone.addEventListener('drop', (e) => {
    e.preventDefault();
    zone.classList.remove('over');
    if (handler) handler(e.dataTransfer.files);
  });
}

gameFileEl.addEventListener('change', () => {
  gameFile = gameFileEl.files[0] || null;
  gameInfo.textContent = gameFile ? `${gameFile.name} — ${fmtBytes(gameFile.size)}` : '';
  refreshUi();
});

// ── The merge flow ────────────────────────────────────────────────────────

function setStatus(text, fraction) {
  statusEl.textContent = text;
  if (fraction !== undefined) progressFill.style.width = `${Math.min(100, fraction * 100)}%`;
}

async function readRange(file, offset, length) {
  const buf = await file.slice(offset, offset + length).arrayBuffer();
  return new Uint8Array(buf);
}

async function runMerge() {
  setError('');
  resultCard.hidden = true;
  progress.hidden = false;
  mergeBtn.disabled = true;
  resultPak = null;
  let bytesRead = 0;

  try {
    const session = new wasm_bindgen.MergeSession();
    const sorted = [...modFiles].sort((a, b) => a.name.localeCompare(b.name));
    for (const f of sorted) {
      setStatus(`Loading ${f.name}…`, 0.02);
      session.add_mod(f.name, new Uint8Array(await f.arrayBuffer()));
    }
    session.set_game_pak(gameFile.size);

    // The pak index + footer live at the end of the file; grab a generous
    // tail first, then let the session ask for anything else it needs.
    const TAIL = 4 * 1024 * 1024;
    const tailStart = Math.max(0, gameFile.size - TAIL);
    session.provide(tailStart, await readRange(gameFile, tailStart, gameFile.size - tailStart));
    bytesRead += gameFile.size - tailStart;

    const expected = gameFile.size;
    for (let step = 0; step < 200; step++) {
      const r = session.step();
      if (r.status === 'need') {
        setStatus(
          `Reading game data… ${fmtBytes(bytesRead)} (${((bytesRead / expected) * 100).toFixed(2)}% of the pak)`,
          0.1 + 0.8 * Math.min(1, bytesRead / (32 * 1024 * 1024)),
        );
        session.provide(r.offset, await readRange(gameFile, r.offset, r.length));
        bytesRead += r.length;
        continue;
      }
      // done
      resultPak = r.pak;
      const report = JSON.parse(r.report);
      setStatus(`Merged ${report.merged} datatable(s), ${report.files} file(s) total.`, 1);
      showReport(report);
      break;
    }
  } catch (e) {
    setError(`Merge failed: ${e}`);
  } finally {
    mergeBtn.disabled = false;
    refreshUi();
  }
}

function showReport(report) {
  resultSummary.textContent = `${report.merged} datatable(s) merged, ${report.files} file(s) total`;
  resultCard.hidden = false;

  let html = '<h3>Merge order <span class="dim">(later entries win on conflicts)</span></h3><ol class="order">';
  for (const label of report.order) html += `<li>${escapeHtml(label)}</li>`;
  html += '</ol>';

  if (report.conflicts && report.conflicts.length) {
    html += '<h3 class="warn">Conflicts — later mod won</h3><ul class="conflicts">';
    for (const c of report.conflicts) html += `<li>${escapeHtml(c)}</li>`;
    html += '</ul>';
  } else {
    html += '<p class="ok">No field-level conflicts detected.</p>';
  }

  if (report.passThroughConflicts && report.passThroughConflicts.length) {
    html += '<h3 class="warn">Files that can\'t be combined <span class="dim">(single-winner, last kept)</span></h3><ul class="conflicts">';
    for (const c of report.passThroughConflicts) html += `<li>${escapeHtml(c)}</li>`;
    html += '</ul>';
  }

  if (report.warnings && report.warnings.length) {
    html += '<h3 class="warn">Warnings</h3><ul class="conflicts">';
    for (const w of report.warnings) html += `<li>${escapeHtml(w)}</li>`;
    html += '</ul>';
  }

  reportEl.innerHTML = html;
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

downloadBtn.addEventListener('click', () => {
  if (!resultPak) return;
  const blob = new Blob([resultPak], { type: 'application/octet-stream' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob);
  a.download = 'SicarioCombine_P.pak';
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
});

mergeBtn.addEventListener('click', runMerge);

// ── Init ─────────────────────────────────────────────────────────────────

function initEngine() {
  if (typeof window.MODMAN_WASM_BASE64 === 'string') {
    // Single-file build: the wasm bytes are embedded — no fetch, works on
    // file:// with zero network access.
    const bin = atob(window.MODMAN_WASM_BASE64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    wasm_bindgen.initSync(bytes);
    return Promise.resolve();
  }
  // Hosted build: fetch the .wasm next to the page.
  return wasm_bindgen('./pkg/modman_wasm_bg.wasm');
}

initEngine()
  .then(() => {
    wasmReady = true;
    refreshUi();
  })
  .catch((e) => {
    setError(`Failed to load the merge engine: ${e}`);
  });
