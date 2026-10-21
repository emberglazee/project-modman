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
  gameInfo.textContent = gameFile ? `${gameFile.name} (${fmtBytes(gameFile.size)})` : '';
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

// What the last run looked like, for the diagnostics block.
let lastRun = null;

function newRunRecord() {
  return {
    startedAt: new Date().toISOString(),
    mods: modFiles.map((f) => ({ name: f.name, size: f.size })),
    game: gameFile ? { name: gameFile.name, size: gameFile.size } : null,
    steps: 0,
    needs: 0,
    bytesRead: 0,
    status: '',
    error: null,
  };
}

// Errors we can explain. Anything else falls through to the generic handler.
function classifyError(e) {
  const raw = String((e && e.message) || e || '');
  const run = lastRun || {};

  if (/out of memory|allocation failed|Cannot enlarge memory|memory access out of bounds|Array buffer allocation|^unreachable$|RuntimeError/i.test(raw)) {
    return {
      title: 'Not enough memory for this mod set',
      body:
        `The browser ran out of memory while merging ${run.mods ? run.mods.length : 'these'} mods. ` +
        `That is a limit of running the merger inside a web page, not a problem with your mods.\n\n` +
        `What to try, in order:\n` +
        `1. Merge in smaller batches: 3 or 4 aircraft-sized mods at a time, then add the merged file to the next round.\n` +
        `2. Close other tabs and apps, reload this page, and try again.\n` +
        `3. Use the desktop build instead. It has no such limit: the same 29-mod set merged in 17 seconds there.\n` +
        `   modman combine "C:\\...\\~mods\\*.pak" --install-path "C:\\...\\Project Wingman" --output out\n` +
        `   See docs/command-line-walkthrough.md for the step-by-step version.`,
    };
  }
  if (/invalid|magic|not a pak|unsupported|bad header|version/i.test(raw)) {
    return {
      title: 'One of the files is not a mod pak',
      body:
        `The merger could not read one of the files you added. Common causes:\n` +
        `- A .zip/.7z/.rar archive was renamed to .pak instead of being extracted first.\n` +
        `- The game pak was dropped into the mod list, or a mod pak into the game pak slot.\n` +
        `- A download was cut short (the file is smaller than it should be).\n\n` +
        `Re-extract the archive and add the .pak files inside it.`,
    };
  }
  if (/step-limit|stalled/i.test(raw)) {
    return {
      title: 'The merge stopped making progress',
      body:
        `The merger kept asking for the same part of the game pak without moving forward. ` +
        `That usually means the game pak is not the one these mods were built against ` +
        `(for example a mod for a different game version).\n\n` +
        `Check that the game pak is pakchunk0-WindowsNoEditor.pak from your own install.`,
    };
  }
  if (/internal error:/i.test(raw)) {
    return {
      title: 'The merge engine hit an internal error',
      body:
        `This one is a bug in modman, not something you did. The message below names the internal step. ` +
        `Please report it with the diagnostics text attached; it is what makes these fixable.`,
    };
  }
  if (/could not read|NotReadableError|NotFoundError/i.test(raw)) {
    return {
      title: 'A file could not be read from disk',
      body:
        `The browser lost access to one of the files. This happens when a file is moved, ` +
        `renamed or deleted while the page is open, or when the disk is busy.\n\n` +
        `Re-add the files and try again.`,
    };
  }
  return {
    title: 'The merge stopped',
    body:
      `The message below is the raw error. If it means nothing to you, paste the diagnostics ` +
      `text into a report and we can read it from our side.`,
  };
}

function buildDiagnostics(rawError) {
  const run = lastRun || {};
  const lines = [];
  lines.push('Project Modman: browser merge diagnostics');
  lines.push(`when: ${run.startedAt || new Date().toISOString()}`);
  lines.push(`page: ${location.href}`);
  lines.push(`browser: ${navigator.userAgent}`);
  lines.push(`hardwareConcurrency: ${navigator.hardwareConcurrency || 'n/a'}`);
  lines.push('');
  lines.push(`mods (${(run.mods || []).length}, ${fmtBytes((run.mods || []).reduce((a, m) => a + m.size, 0))} total):`);
  for (const m of run.mods || []) lines.push(`  - ${m.name} (${fmtBytes(m.size)})`);
  lines.push(`game pak: ${run.game ? `${run.game.name} (${fmtBytes(run.game.size)})` : 'none'}`);
  lines.push('');
  lines.push(`steps: ${run.steps || 0} (game-pak reads: ${run.needs || 0}, ${fmtBytes(run.bytesRead || 0)} read)`);
  lines.push(`last status: ${run.status || 'n/a'}`);
  lines.push(`error: ${rawError}`);
  return lines.join('\n');
}

function showError(e) {
  const { title, body } = classifyError(e);
  const raw = String((e && e.message) || e || '').trim();
  const diag = buildDiagnostics(raw);

  errorEl.hidden = false;
  errorEl.innerHTML = '';

  const h = document.createElement('p');
  h.className = 'errtitle';
  h.textContent = title;
  errorEl.appendChild(h);

  const b = document.createElement('p');
  b.className = 'errbody';
  b.textContent = body;
  errorEl.appendChild(b);

  const det = document.createElement('details');
  const sum = document.createElement('summary');
  sum.textContent = 'Diagnostics (copy this into a bug report)';
  det.appendChild(sum);

  const ta = document.createElement('textarea');
  ta.readOnly = true;
  ta.value = diag;
  ta.rows = 12;
  det.appendChild(ta);

  const copy = document.createElement('button');
  copy.className = 'btn secondary';
  copy.textContent = 'Copy diagnostics';
  copy.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(diag);
      copy.textContent = 'Copied';
    } catch {
      ta.select();
      copy.textContent = 'Press Ctrl+C';
    }
  });
  det.appendChild(copy);

  errorEl.appendChild(det);
}

const MAX_STEPS = 100_000;

async function runMerge() {
  setError('');
  resultCard.hidden = true;
  progress.hidden = false;
  mergeBtn.disabled = true;
  resultPak = null;
  lastRun = newRunRecord();
  let bytesRead = 0;
  let loadingMod = null;

  try {
    const totalModBytes = modFiles.reduce((a, f) => a + f.size, 0);
    // Measured: ~870 MB of mod paks already exhausts a Chromium tab during the
    // merge, because the set is held in memory twice (inputs, then output).
    if (totalModBytes > 1_500_000_000) {
      throw new Error(
        `out of memory: this set is ${fmtBytes(totalModBytes)}; a browser tab cannot hold that much`,
      );
    }

    const session = new wasm_bindgen.MergeSession();
    const sorted = [...modFiles].sort((a, b) => a.name.localeCompare(b.name));
    for (const f of sorted) {
      loadingMod = f.name;
      setStatus(`Loading ${f.name} (${fmtBytes(f.size)})…`, 0.02);
      let bytes;
      try {
        bytes = new Uint8Array(await f.arrayBuffer());
      } catch (e) {
        throw new Error(`could not read ${f.name} from disk: ${(e && e.message) || e}`);
      }
      try {
        session.add_mod(f.name, bytes);
      } catch (e) {
        throw new Error(`could not load ${f.name} as a pak: ${(e && e.message) || e}`);
      }
    }
    loadingMod = null;
    session.set_game_pak(gameFile.size);

    // The pak index + footer live at the end of the file; grab a generous
    // tail first, then let the session ask for anything else it needs.
    const TAIL = 4 * 1024 * 1024;
    const tailStart = Math.max(0, gameFile.size - TAIL);
    session.provide(tailStart, await readRange(gameFile, tailStart, gameFile.size - tailStart));
    bytesRead += gameFile.size - tailStart;

    const expected = gameFile.size;
    let lastRequest = '';
    let repeats = 0;

    for (let step = 0; ; step++) {
      if (step >= MAX_STEPS) {
        throw new Error(`step-limit: gave up after ${MAX_STEPS} reads from the game pak`);
      }
      const r = session.step();
      lastRun.steps = step + 1;
      if (r.status === 'need') {
        // A merge that keeps asking for the same range is stuck, not slow.
        const key = `${r.offset}:${r.length}`;
        repeats = key === lastRequest ? repeats + 1 : 0;
        lastRequest = key;
        if (repeats > 50) throw new Error(`stalled: asked for ${key} ${repeats} times`);

        lastRun.needs++;
        lastRun.status = `reading ${fmtBytes(bytesRead)} of the game pak`;
        setStatus(
          `Reading game data… ${fmtBytes(bytesRead)} (${((bytesRead / expected) * 100).toFixed(2)}% of the pak)`,
          0.1 + 0.8 * Math.min(1, bytesRead / (32 * 1024 * 1024)),
        );
        session.provide(r.offset, await readRange(gameFile, r.offset, r.length));
        bytesRead += r.length;
        lastRun.bytesRead = bytesRead;
        continue;
      }
      // done
      resultPak = r.pak;
      const report = JSON.parse(r.report);
      lastRun.status = `merged ${report.merged} datatable(s), ${report.files} file(s)`;
      setStatus(`Merged ${report.merged} datatable(s), ${report.files} file(s) total.`, 1);
      showReport(report);
      break;
    }
  } catch (e) {
    lastRun.error = String((e && e.message) || e);
    if (loadingMod) lastRun.error = `while loading ${loadingMod}: ${lastRun.error}`;
    showError(e);
  } finally {
    mergeBtn.disabled = false;
    refreshUi();
  }
}

function showReport(report) {
  resultSummary.textContent = `${report.merged} datatable(s) merged, ${report.files} file(s) total`;
  resultCard.hidden = false;

  const skipped = new Set(report.skipped || []);
  let html = '<h3>Merge order <span class="dim">(later entries win on conflicts)</span></h3><ol class="order">';
  for (const label of report.order) {
    html += skipped.has(label)
      ? `<li>${escapeHtml(label)} <span class="dim">(skipped: previous output)</span></li>`
      : `<li>${escapeHtml(label)}</li>`;
  }
  html += '</ol>';

  if (skipped.size) {
    html += '<p class="warn">Skipped a previous combine output (SicarioCombine_P.pak). Merge the original mod paks instead.</p>';
  }

  if (report.conflicts && report.conflicts.length) {
    html += '<h3 class="warn">Conflicts (later mod won)</h3><ul class="conflicts">';
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
