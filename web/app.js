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
const checkNowBtn = $('checkNowBtn');
const probeCard = $('probeCard');
const probeBody = $('probeBody');
const probeStatusEl = $('probeStatus');
const orderHint = $('orderHint');
const progress = $('progress');
const progressFill = $('progressFill');
const statusEl = $('status');
const errorEl = $('error');
const resultCard = $('resultCard');
const resultSummary = $('resultSummary');
const reportEl = $('report');
const downloadBtn = $('downloadBtn');

let wasmReady = false;
let modEntries = [];   // { file, id, sessionIndex }
let nextModId = 1;
let direction = 'last'; // 'last' = later wins, 'first' = earlier wins
let gameFile = null;    // File
let resultPak = null;
let mergeSession = null;
let sessionKey = null;
let probeGeneration = 0;
let probeTimer = null;
let probeTask = null;
let probeBusy = false;
let merging = false;

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
  const count = modEntries.length;
  mergeBtn.disabled = !(wasmReady && count > 0 && gameFile) || merging;
  mergeBtn.textContent = count
    ? `Merge ${count} mod pak${count > 1 ? 's' : ''}`
    : 'Merge mods';
  if (checkNowBtn) {
    checkNowBtn.disabled = !(wasmReady && count > 0 && gameFile) || merging;
  }
}

function updateOrderHint() {
  if (!orderHint) return;
  orderHint.textContent =
    direction === 'last'
      ? 'Applied top to bottom. Later entries win.'
      : 'Applied bottom to top. Earlier entries win.';
}

function renderModList() {
  modList.innerHTML = '';
  const last = modEntries.length - 1;
  modEntries.forEach((entry, i) => {
    const f = entry.file;
    const wins =
      modEntries.length > 1 &&
      ((direction === 'last' && i === last) || (direction === 'first' && i === 0));
    const li = document.createElement('li');
    li.className = wins ? 'modrow wins' : 'modrow';
    li.draggable = true;
    li.dataset.index = String(i);
    li.innerHTML = `<span class="drag" title="Drag to reorder">⠿</span>
      <span class="idx">${i + 1}.</span>
      <span class="fname">${escapeHtml(f.name)}</span>
      <span class="fsize">${fmtBytes(f.size)}</span>
      ${wins ? '<span class="badge">wins</span>' : ''}
      <span class="rowbtns">
        <button class="iconbtn" data-action="up" ${i === 0 ? 'disabled' : ''} title="Move up">↑</button>
        <button class="iconbtn" data-action="down" ${i === last ? 'disabled' : ''} title="Move down">↓</button>
        <button class="iconbtn" data-action="remove" title="Remove">✕</button>
      </span>`;
    modList.appendChild(li);
  });
  updateOrderHint();
}

function applicationEntries() {
  return direction === 'last' ? modEntries : [...modEntries].reverse();
}

function applicationIndices() {
  return applicationEntries().map((entry) => entry.sessionIndex);
}

function moveEntry(from, to) {
  if (from === to || from < 0 || to < 0 || from >= modEntries.length || to >= modEntries.length) {
    return;
  }
  const [entry] = modEntries.splice(from, 1);
  modEntries.splice(to, 0, entry);
  renderModList();
  scheduleProbe();
}

function removeEntry(index) {
  if (index < 0 || index >= modEntries.length) return;
  modEntries.splice(index, 1);
  renderModList();
  refreshUi();
  sessionKey = null;
  scheduleProbe();
}

// ── File picking ──────────────────────────────────────────────────────────

function addModFiles(files) {
  const incoming = [...files].filter((f) => f.name.toLowerCase().endsWith('.pak'));
  let added = false;
  for (const f of incoming) {
    if (!modEntries.some((m) => m.file.name === f.name && m.file.size === f.size)) {
      modEntries.push({ file: f, id: nextModId++, sessionIndex: -1 });
      added = true;
    }
  }
  if (added) {
    renderModList();
    refreshUi();
    sessionKey = null;
    scheduleProbe();
  }
}

modList.addEventListener('click', (event) => {
  const button = event.target.closest('button[data-action]');
  if (!button) return;
  const row = event.target.closest('li[data-index]');
  if (!row) return;
  const index = Number(row.dataset.index);
  if (button.dataset.action === 'up') moveEntry(index, index - 1);
  else if (button.dataset.action === 'down') moveEntry(index, index + 1);
  else if (button.dataset.action === 'remove') removeEntry(index);
});

let dragFrom = null;
modList.addEventListener('dragstart', (event) => {
  if (event.target.closest('button')) {
    event.preventDefault();
    return;
  }
  const row = event.target.closest('li[data-index]');
  if (!row) return;
  dragFrom = Number(row.dataset.index);
  event.dataTransfer.effectAllowed = 'move';
  try {
    event.dataTransfer.setData('text/plain', String(dragFrom));
  } catch (e) {
    // Some browsers restrict setData; the in-page dragFrom is enough.
  }
});
modList.addEventListener('dragover', (event) => {
  if (dragFrom === null) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = 'move';
});
modList.addEventListener('drop', (event) => {
  if (dragFrom === null) return;
  event.preventDefault();
  const row = event.target.closest('li[data-index]');
  if (row) moveEntry(dragFrom, Number(row.dataset.index));
  dragFrom = null;
});
modList.addEventListener('dragend', () => {
  dragFrom = null;
});

for (const radio of document.querySelectorAll('input[name="priority"]')) {
  radio.addEventListener('change', () => {
    if (!radio.checked) return;
    direction = radio.value;
    renderModList();
    scheduleProbe();
  });
}

checkNowBtn?.addEventListener('click', () => scheduleProbe(0));

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
    mods: modEntries.map((e) => ({ name: e.file.name, size: e.file.size })),
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
  let lastPanic = '';
  try {
    if (typeof wasm_bindgen !== 'undefined' && wasm_bindgen.last_panic) {
      lastPanic = wasm_bindgen.last_panic() || '';
    }
  } catch (_) {
    // The module may be unusable after a trap; fall back to the raw text.
  }

  if (lastPanic && /unreachable|RuntimeError/i.test(raw)) {
    return {
      title: 'The merge engine hit an internal error',
      body:
        `This one is a bug in modman, not something you did. The engine reported:\n\n${lastPanic}\n\n` +
        `Please report it with the diagnostics text attached; it is what makes these fixable.`,
    };
  }
  // "unreachable" is unanchored on purpose: an allocation failure while mods
  // are being loaded arrives wrapped as "could not load X as a pak: unreachable",
  // and an anchored match sent that to the generic handler. Rust aborts on
  // allocation error without unwinding, so this trap is what an exhausted wasm
  // heap looks like from JS.
  if (/out of memory|allocation failed|Cannot enlarge memory|memory access out of bounds|Array buffer allocation|unreachable|RuntimeError/i.test(raw)) {
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

// ── Conflict probe ────────────────────────────────────────────────────────
// A probe runs the same core combine as the merge, with pass-through bytes
// deferred, and never writes a pak. It shares one session with the merge so
// the mod bytes and the game-pak ranges are read once; a reorder only changes
// the application indices.

function sessionKeyFor() {
  const mods = modEntries
    .map((e) => `${e.file.name}:${e.file.size}:${e.file.lastModified}`)
    .sort()
    .join('|');
  return `${gameFile.name}:${gameFile.size}:${gameFile.lastModified}|${mods}`;
}

async function ensureSession(onStatus) {
  const key = sessionKeyFor();
  if (mergeSession && sessionKey === key) return mergeSession;
  if (mergeSession) {
    try {
      mergeSession.free();
    } catch (e) {
      console.error('modman: freeing the previous session failed:', e);
    }
    mergeSession = null;
    sessionKey = null;
  }
  const session = new wasm_bindgen.MergeSession();
  try {
    for (let i = 0; i < modEntries.length; i++) {
      const entry = modEntries[i];
      if (onStatus) onStatus(`Loading ${entry.file.name} (${fmtBytes(entry.file.size)})…`);
      let bytes;
      try {
        bytes = new Uint8Array(await entry.file.arrayBuffer());
      } catch (e) {
        throw new Error(`could not read ${entry.file.name} from disk: ${(e && e.message) || e}`);
      }
      try {
        session.add_mod(entry.file.name, bytes);
      } catch (e) {
        throw new Error(`could not load ${entry.file.name} as a pak: ${(e && e.message) || e}`);
      }
      entry.sessionIndex = i;
    }
    session.set_game_pak(gameFile.size);
    const TAIL = 4 * 1024 * 1024;
    const tailStart = Math.max(0, gameFile.size - TAIL);
    session.provide(tailStart, await readRange(gameFile, tailStart, gameFile.size - tailStart));
    mergeSession = session;
    sessionKey = key;
    return session;
  } catch (e) {
    try {
      session.free();
    } catch (err) {
      console.error('modman: freeing the failed session failed:', err);
    }
    throw e;
  }
}

function probeSay(text) {
  if (probeStatusEl) probeStatusEl.textContent = text;
}

function scheduleProbe(delay = 400) {
  if (!wasmReady || !gameFile || modEntries.length === 0 || merging) return;
  probeGeneration += 1;
  const generation = probeGeneration;
  if (probeTimer) {
    clearTimeout(probeTimer);
    probeTimer = null;
  }
  probeTimer = setTimeout(() => {
    probeTimer = null;
    startProbe(generation);
  }, delay);
}

function startProbe(generation) {
  if (generation !== probeGeneration || merging) return;
  if (probeBusy) {
    probeTimer = setTimeout(() => startProbe(generation), 100);
    return;
  }
  probeBusy = true;
  if (probeCard) probeCard.hidden = false;
  probeSay('Checking conflicts…');
  probeTask = runProbe(generation).finally(() => {
    probeBusy = false;
    probeTask = null;
  });
}

async function runProbe(generation) {
  try {
    const session = await ensureSession((text) => probeSay(text));
    if (generation !== probeGeneration || merging) return;
    session.set_order(new Uint32Array(applicationIndices()));
    for (let step = 0; step < MAX_STEPS; step++) {
      if (generation !== probeGeneration || merging) return;
      const result = session.probe_step();
      if (result.status === 'need') {
        session.provide(result.offset, await readRange(gameFile, result.offset, result.length));
        continue;
      }
      if (generation === probeGeneration) {
        showProbe(JSON.parse(result.report));
      }
      return;
    }
    throw new Error(`step-limit: gave up after ${MAX_STEPS} reads from the game pak`);
  } catch (e) {
    if (generation === probeGeneration) {
      showProbeError(String((e && e.message) || e));
    }
  }
}

function showProbe(report) {
  if (!probeCard || !probeBody) return;
  probeCard.hidden = false;
  probeSay('Checked.');
  const conflicts = report.conflicts || [];
  const passThrough = report.passThroughConflicts || [];
  const warnings = report.warnings || [];
  let html = `<p class="hint">${report.merged} datatable(s), ${report.files} file(s): ${conflicts.length} field conflict(s), ${passThrough.length} file(s) that can't be combined, ${warnings.length} warning(s).</p>`;
  const list = (title, items) => {
    if (!items.length) return '';
    return `<h4>${escapeHtml(title)} (${items.length})</h4><ul class="conflicts">${items
      .map((item) => `<li>${escapeHtml(item)}</li>`)
      .join('')}</ul>`;
  };
  html += list('Field conflicts', conflicts);
  html += list("Files that can't be combined", passThrough);
  html += list('Warnings', warnings);
  if (!conflicts.length && !passThrough.length && !warnings.length) {
    html += '<p class="ok">No conflicts detected.</p>';
  }
  probeBody.innerHTML = html;
}

function showProbeError(message) {
  if (!probeCard || !probeBody) return;
  probeCard.hidden = false;
  probeSay('Check failed.');
  probeBody.innerHTML = `<p class="warn">${escapeHtml(message)}</p>`;
}

async function runMerge() {
  setError('');
  resultCard.hidden = true;
  progress.hidden = false;
  mergeBtn.disabled = true;
  resultPak = null;
  lastRun = newRunRecord();
  wasm_bindgen.clear_last_panic();
  merging = true;
  probeGeneration += 1;
  if (probeTimer) {
    clearTimeout(probeTimer);
    probeTimer = null;
  }
  const pendingProbe = probeTask;
  let session = null;

  try {
    if (pendingProbe) {
      try {
        await pendingProbe;
      } catch (e) {
        // The probe loop reports its own errors; a merge still proceeds.
      }
    }
    const totalModBytes = modEntries.reduce((a, e) => a + e.file.size, 0);
    // Measured: ~870 MB of mod paks already exhausts a Chromium tab during the
    // merge, because the set is held in memory twice (inputs, then output).
    if (totalModBytes > 1_500_000_000) {
      throw new Error(
        `out of memory: this set is ${fmtBytes(totalModBytes)}; a browser tab cannot hold that much`,
      );
    }

    session = await ensureSession((text) => setStatus(text, 0.02));
    session.set_order(new Uint32Array(applicationIndices()));

    // The pak index + footer live at the end of the file; ensureSession seeded
    // the tail, and the step loop asks for anything else it needs.
    const TAIL = 4 * 1024 * 1024;
    let bytesRead = Math.min(TAIL, gameFile.size);

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
    showError(e);
  } finally {
    // Free the session explicitly. It holds every mod pak and every game-pak
    // range it was handed, and nothing else points at it once this run ends.
    // wasm-bindgen does register a FinalizationRegistry, but that only fires
    // when the JS collector runs, and wasm growth alone does not make it:
    // measured, twelve 64 MB sessions without this line climb linearly to
    // 807 MB and reclaim nothing, while the same loop with free() stays flat at
    // 65 MB. On this page the collector happens to keep up, because reading each
    // mod pak allocates a large array in the JS heap; freeing makes the release
    // deterministic instead of dependent on that pressure.
    if (session) {
      // A trap while freeing must not hide the error the user is being shown.
      try {
        session.free();
      } catch (e) {
        console.error('modman: freeing the merge session failed:', e);
      }
      session = null;
    }
    mergeSession = null;
    sessionKey = null;
    merging = false;
    mergeBtn.disabled = false;
    refreshUi();
  }
}

function showReport(report) {
  resultSummary.textContent = `${report.merged} datatable(s) merged, ${report.files} file(s) total`;
  resultCard.hidden = false;

  const skipped = new Set(report.skipped || []);
  let html = `<h3>Merge order <span class="dim">(as listed; ${
    direction === 'last' ? 'later' : 'earlier'
  } entries win on conflicts)</span></h3><ol class="order">`;
  for (const entry of modEntries) {
    const label = entry.file.name;
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
  // Hosted build: fetch the .wasm next to the page. Carry this script tag's own
  // cache-busting query, or a new build can pair with a module cached from the
  // previous one; the version lives in index.html and nowhere else.
  const bust = (document.currentScript && new URL(document.currentScript.src).search) || '';
  return wasm_bindgen('./pkg/modman_wasm_bg.wasm' + bust);
}

initEngine()
  .then(() => {
    wasmReady = true;
    refreshUi();
  })
  .catch((e) => {
    setError(`Failed to load the merge engine: ${e}`);
  });

// ── Translations ─────────────────────────────────────────────────────────
// Small files, no game pak needed: parse in wasm, patch the strings, hand the
// result back as a download. Nothing leaves the page.

const locresFileEl = $('locresFile');
const locresCsvEl = $('locresCsv');
const locresToCsvBtn = $('locresToCsvBtn');
const locresPatchBtn = $('locresPatchBtn');
const locresStatus = $('locresStatus');

let locresBytes = null;
let locresName = 'ProjectWingman.locres';
let locresCsvText = null;

function locresSay(msg) {
  if (locresStatus) locresStatus.textContent = msg;
}

function downloadBytes(bytes, name, type) {
  const blob = new Blob([bytes], { type: type || 'application/octet-stream' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10000);
}

async function readFileBytes(file) {
  return new Uint8Array(await file.arrayBuffer());
}

if (locresFileEl) {
  locresFileEl.addEventListener('change', async () => {
    const f = locresFileEl.files[0];
    if (!f) return;
    try {
      locresBytes = await readFileBytes(f);
      locresName = f.name;
      locresToCsvBtn.disabled = !wasmReady;
      locresPatchBtn.disabled = !(wasmReady && locresCsvText);
      locresSay(`${f.name} loaded (${fmtBytes(f.size)}).`);
    } catch (e) {
      locresSay(`Could not read ${f.name}: ${e}`);
    }
  });
}

if (locresCsvEl) {
  locresCsvEl.addEventListener('change', async () => {
    const f = locresCsvEl.files[0];
    if (!f) return;
    locresCsvText = await f.text();
    locresPatchBtn.disabled = !(wasmReady && locresBytes);
    const rows = locresCsvText.split(/\r?\n/).filter((l) => l.trim()).length - 1;
    locresSay(`CSV loaded: about ${rows} row(s).`);
  });
}

if (locresToCsvBtn) {
  locresToCsvBtn.addEventListener('click', () => {
    if (!wasmReady || !locresBytes) return;
    try {
      wasm_bindgen.clear_last_panic();
      const csv = wasm_bindgen.locres_to_csv(locresBytes);
      const name = locresName.replace(/\.locres$/i, '') + '.csv';
      downloadBytes(csv, name, 'text/csv');
      locresSay(`Saved ${name}.`);
    } catch (e) {
      locresSay(`Error: ${e}`);
    }
  });
}

if (locresPatchBtn) {
  locresPatchBtn.addEventListener('click', () => {
    if (!wasmReady || !locresBytes || !locresCsvText) return;
    let patched = null;
    try {
      wasm_bindgen.clear_last_panic();
      patched = wasm_bindgen.locres_patch(locresBytes, locresCsvText);
      downloadBytes(patched.bytes, locresName);
      locresSay(`Patched ${patched.patched} string(s); downloaded ${locresName}.`);
    } catch (e) {
      locresSay(`Error: ${e}`);
    } finally {
      // Same reason as the merge session: a wasm object is not reclaimed until
      // the collector happens to run, and this one holds the whole table.
      if (patched) patched.free();
    }
  });
}

