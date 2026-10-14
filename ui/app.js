const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const icons = () => { try { lucide.createIcons(); } catch (e) { /* sin iconos */ } };
let busy = false;
let versions = [];
let settings = null;
let serverRunning = false;
let lastInfo = null;

// ---------- pestañas ----------
document.querySelectorAll('.tab').forEach((tb) => {
  tb.onclick = () => {
    document.querySelectorAll('.tab').forEach((x) => x.classList.toggle('on', x === tb));
    document.querySelectorAll('.pane').forEach((p) => (p.hidden = p.id !== 'pane-' + tb.dataset.tab));
    if (tb.dataset.tab === 'log') loadLog();
    if (tb.dataset.tab === 'srv') pollServer();
  };
});

function setBusy(v) {
  busy = v;
  $('update').disabled = v || !versions.length;
  document.querySelectorAll('#bkList button').forEach((b) => (b.disabled = v));
}
function showErr(e) { $('err').textContent = e ? terr(e) : ''; }

// ---------- artifacts ----------
async function refresh() {
  const i = await invoke('get_info');
  lastInfo = i;
  $('locate').hidden = !i.error;
  $('main').hidden = !!i.error;
  if (i.error) { $('path').textContent = terr(i.error); return; }
  $('path').textContent = i.artifacts;
  renderInfo(i);
  if (!settings) settings = await invoke('get_settings');
  renderKeep(i);
  await loadVersions(i.recommended);
  setBusy(busy);
  await loadBackups();
  await loadTask();
}

function renderInfo(i) {
  $('installed').textContent = i.installed || t('unknown');
  $('recommended').textContent = i.recommended || (i.net_error ? t('no_conn') : '—');
  const badge = $('instBadge');
  badge.className = 'badge';
  badge.textContent = '';
  if (i.installed_status === 'OK') { badge.classList.add('ok'); badge.textContent = t('badge_ok'); }
  else if (i.installed_status === 'BROKEN') { badge.classList.add('bad'); badge.textContent = t('badge_broken'); }
  const note = $('verNote');
  if (!i.installed) note.textContent = t('note_nover');
  else if (i.installed_status === 'BROKEN') note.textContent = t('note_broken', i.installed_reason || '');
  else if (i.recommended && i.installed !== i.recommended) note.textContent = t('note_newer');
  else note.textContent = t('note_current');
  if (i.fx_running) note.textContent += ' · ' + t('note_running');
}

function renderKeep(i) {
  const list = $('keepList');
  const prev = new Set([...list.querySelectorAll('input:checked')].map((c) => c.value));
  const first = list.dataset.init !== '1';
  list.dataset.init = '1';
  list.innerHTML = '';
  if (!i.resources.length) list.innerHTML = `<span class="hint">${esc(t('keep_none'))}</span>`;
  for (const r of i.resources) {
    const on = first ? settings.keep.includes(r) : prev.has(r);
    const l = document.createElement('label');
    l.className = 'chk';
    l.innerHTML = `<input type="checkbox" value="${esc(r)}" ${on ? 'checked' : ''}> <span>${esc(r)}</span>` +
      (['chat', 'monitor'].includes(r) ? '' : `<small>${esc(t('keep_rare'))}</small>`);
    list.appendChild(l);
  }
}

async function loadVersions(recommended) {
  const sel = $('verSelect');
  const current = sel.value;
  try { versions = await invoke('list_versions'); } catch (e) { versions = []; $('selStatus').textContent = terr(e); return; }
  sel.innerHTML = '';
  for (const v of versions) {
    const o = document.createElement('option');
    o.value = v.version;
    o.textContent = `${v.version}` + (v.tags.length ? ` (${v.tags.join(', ')})` : '');
    sel.appendChild(o);
  }
  sel.value = versions.some((v) => v.version === current) ? current : (recommended && versions.some((v) => v.version === recommended) ? recommended : versions[0].version);
  await showSelStatus();
}

async function showSelStatus() {
  const v = $('verSelect').value;
  const el = $('selStatus');
  el.textContent = '';
  if (!v) return;
  try {
    const r = await invoke('check_version', { version: v });
    el.textContent = r.status === 'OK' ? t('sel_ok', v) : t('sel_bad', v, r.reason || '');
  } catch (e) { /* sin red */ }
}
$('verSelect').onchange = showSelStatus;

async function loadBackups() {
  const box = $('bkList');
  box.innerHTML = '';
  $('bkTitle').textContent = t('bk_title', settings ? settings.max_backups : 5);
  let list = [];
  try { list = await invoke('list_backups'); } catch (e) { return; }
  if (!list.length) box.innerHTML = `<span class="hint">${esc(t('bk_none'))}</span>`;
  for (const b of list) {
    const row = document.createElement('div');
    row.className = 'row';
    row.innerHTML = `<span style="direction:ltr">${esc(b.name.replace(/_/g, ' '))}</span>`;
    const btn = document.createElement('button');
    btn.className = 'sec';
    btn.innerHTML = `<i data-lucide="rotate-ccw"></i><span>${esc(t('bk_restore'))}</span>`;
    btn.onclick = () => restore(b.name);
    const del = document.createElement('button');
    del.className = 'sec danger';
    del.innerHTML = `<i data-lucide="trash-2"></i><span>${esc(t('bk_delete'))}</span>`;
    del.onclick = () => removeBackup(b.name);
    const acts = document.createElement('span');
    acts.style.display = 'flex';
    acts.style.gap = '6px';
    acts.append(btn, del);
    row.appendChild(acts);
    box.appendChild(row);
  }
  icons();
  setBusy(busy);
}

async function run(fn) {
  if (busy) return;
  showErr('');
  setBusy(true);
  try { await fn(); } catch (e) { showErr(e); $('fill').style.width = '0'; $('msg').textContent = ''; }
  setBusy(false);
  await refresh();
}

function removeBackup(name) {
  if (!confirm(t('confirm_del', name))) return;
  run(() => invoke('delete_backup', { name }));
}

function restore(name) {
  const extra = serverRunning ? '\n' + t('confirm_srv') : '';
  if (!confirm(t('confirm_restore', name) + extra)) return;
  run(() => invoke('restore', { name }));
}

$('update').onclick = () => {
  const keep = [...document.querySelectorAll('#keepList input:checked')].map((c) => c.value);
  const version = $('verSelect').value;
  const entry = versions.find((v) => v.version === version);
  if (!entry) return;
  const a = keep.length ? t('keep_yes', keep.join(', ')) : t('keep_no');
  const srv = serverRunning ? '\n' + t('upd_srv') : '';
  if (!confirm(`${a}\n${t('upd_body', version)}${srv}\n${t('upd_continue')}`)) return;
  run(() => invoke('update', { keep, url: entry.url, version }));
};

$('setBase').onclick = async () => {
  try { await invoke('set_base', { path: $('basePath').value }); await refresh(); }
  catch (e) { $('path').textContent = terr(e); }
};

listen('progress', (ev) => {
  const p = ev.payload;
  $('fill').style.width = p.percent + '%';
  $('msg').textContent = t('p_' + p.stage, ...(p.args || []));
});

// ---------- tarea automatizada ----------
async function loadTask() {
  const tk = await invoke('task_status');
  $('taskDaily').checked = tk.daily;
  $('taskLogon').checked = tk.logon;
  if (settings) $('taskTime').value = settings.task_time || '05:00';
  const parts = [];
  if (tk.daily) parts.push(t('task_d'));
  if (tk.logon) parts.push(t('task_l'));
  $('taskInfo').textContent = (parts.length ? t('task_active', parts.join(' · ')) : t('task_none')) + ' · ' + t('task_exe', tk.exe);
}
$('taskCreate').onclick = async () => {
  $('taskErr').textContent = '';
  try { await invoke('task_create', { time: $('taskTime').value, daily: $('taskDaily').checked, logon: $('taskLogon').checked }); settings = await invoke('get_settings'); await loadTask(); }
  catch (e) { $('taskErr').textContent = terr(e); }
};
$('taskDelete').onclick = async () => {
  $('taskErr').textContent = '';
  try { await invoke('task_delete'); await loadTask(); } catch (e) { $('taskErr').textContent = terr(e); }
};

// ---------- servidor ----------
const con = $('console');
// ---- ANSI -> HTML (colores de la consola) ----
const ANSI16 = ['#000000', '#cd3131', '#0dbc79', '#e5e510', '#2472c8', '#bc3fbc', '#11a8cd', '#e5e5e5',
  '#666666', '#f14c4c', '#23d18b', '#f5f543', '#3b8eea', '#d670d6', '#29b8db', '#ffffff'];
function ansi256(n) {
  if (n < 16) return ANSI16[n];
  if (n >= 232) { const v = 8 + (n - 232) * 10; return `rgb(${v},${v},${v})`; }
  n -= 16;
  const c = [Math.floor(n / 36), Math.floor((n % 36) / 6), n % 6].map((x) => (x ? 55 + x * 40 : 0));
  return `rgb(${c[0]},${c[1]},${c[2]})`;
}
function ansiToHtml(line) {
  let st = { fg: null, bg: null, b: false, d: false, i: false, u: false };
  let out = '';
  const clean = String(line).replace(/\x1b\][^\x07\x1b]*(\x07|\x1b\\)/g, '').replace(/\r/g, '');
  const re = /\x1b\[([0-9;?]*)([A-Za-z])/g;
  let last = 0, m;
  const emit = (txt) => {
    if (!txt) return;
    const css = [];
    if (st.fg) css.push('color:' + st.fg);
    if (st.bg) css.push('background:' + st.bg);
    if (st.b) css.push('font-weight:700');
    if (st.d) css.push('opacity:.7');
    if (st.i) css.push('font-style:italic');
    if (st.u) css.push('text-decoration:underline');
    out += css.length ? `<span style="${css.join(';')}">${esc(txt)}</span>` : esc(txt);
  };
  while ((m = re.exec(clean))) {
    emit(clean.slice(last, m.index));
    last = re.lastIndex;
    if (m[2] !== 'm') continue; // otros codigos (cursor, borrado) se ignoran
    const p = m[1] === '' ? [0] : m[1].split(';').map((x) => parseInt(x, 10) || 0);
    for (let k = 0; k < p.length; k++) {
      const c = p[k];
      if (c === 0) st = { fg: null, bg: null, b: false, d: false, i: false, u: false };
      else if (c === 1) st.b = true;
      else if (c === 2) st.d = true;
      else if (c === 3) st.i = true;
      else if (c === 4) st.u = true;
      else if (c === 22) { st.b = false; st.d = false; }
      else if (c === 23) st.i = false;
      else if (c === 24) st.u = false;
      else if (c >= 30 && c <= 37) st.fg = ANSI16[c - 30];
      else if (c >= 90 && c <= 97) st.fg = ANSI16[c - 90 + 8];
      else if (c === 39) st.fg = null;
      else if (c >= 40 && c <= 47) st.bg = ANSI16[c - 40];
      else if (c >= 100 && c <= 107) st.bg = ANSI16[c - 100 + 8];
      else if (c === 49) st.bg = null;
      else if (c === 38 || c === 48) {
        let col = null;
        if (p[k + 1] === 5) { col = ansi256(p[k + 2] || 0); k += 2; }
        else if (p[k + 1] === 2) { col = `rgb(${p[k + 2] || 0},${p[k + 3] || 0},${p[k + 4] || 0})`; k += 4; }
        if (c === 38) st.fg = col; else st.bg = col;
      }
    }
  }
  emit(clean.slice(last));
  return out;
}

function appendLog(line) {
  const atEnd = con.scrollTop + con.clientHeight >= con.scrollHeight - 30;
  const d = document.createElement('div');
  d.innerHTML = ansiToHtml(line);
  con.appendChild(d);
  while (con.children.length > 800) con.removeChild(con.firstChild);
  if (atEnd) con.scrollTop = con.scrollHeight;
}
listen('server-log', (ev) => appendLog(ev.payload));

let lastSrv = null;
function renderServer() {
  const s = lastSrv;
  if (!s) return;
  serverRunning = s.running;
  const dot = $('srvDot');
  dot.className = 'badge ' + (s.running ? 'ok' : 'warn');
  dot.textContent = s.running ? 'ON' : 'OFF';
  $('srvState').textContent = s.running ? t('srv_on', s.pids.join(', ')) + (s.managed ? '' : ' · ' + t('srv_ext')) : t('srv_off');
  $('srvStart').disabled = s.running;
  $('srvStop').disabled = !s.running;
}
async function pollServer() {
  try { lastSrv = await invoke('server_status'); } catch (e) { return; }
  renderServer();
}
$('srvStart').onclick = async () => {
  $('srvErr').textContent = '';
  $('srvStart').disabled = true;
  try { await invoke('server_start'); } catch (e) { $('srvErr').textContent = terr(e); }
  setTimeout(pollServer, 800);
};
$('srvStop').onclick = async () => {
  if (!confirm(t('confirm_stop'))) return;
  $('srvErr').textContent = '';
  $('srvStop').disabled = true;
  try { await invoke('server_stop'); } catch (e) { $('srvErr').textContent = terr(e); }
  pollServer();
};
setInterval(pollServer, 3000);

$('txOpen').onclick = async () => {
  $('srvErr').textContent = '';
  try { await invoke('open_txadmin'); } catch (e) { $('srvErr').textContent = terr(e); }
};
$('txShortcut').onclick = async () => {
  $('srvErr').textContent = '';
  $('txInfo').textContent = '';
  try { $('txInfo').textContent = t('shortcut_ok', await invoke('create_txadmin_shortcut')); }
  catch (e) { $('srvErr').textContent = terr(e); }
};

// ---------- historial ----------
async function loadLog() {
  const box = $('logList');
  box.innerHTML = '';
  let list = [];
  try { list = await invoke('get_log'); } catch (e) { return; }
  if (!list.length) box.innerHTML = `<span class="hint">${esc(t('log_none'))}</span>`;
  for (const e of list) {
    const row = document.createElement('div');
    row.className = 'logrow';
    const when = new Date(e.ts * 1000).toLocaleString(LANG);
    const msg = e.ok ? '' : `<br><span style="color:#f0928c">${esc(terr(e.msg))}</span>`;
    row.innerHTML = `<span style="color:var(--mut)">${esc(when)}</span>` +
      `<span><span style="direction:ltr;unicode-bidi:embed">${esc(e.from)} → ${esc(e.to)}</span>${msg}` +
      ` <small style="color:var(--mut)">(${esc(e.source === 'auto' ? t('src_auto') : t('src_manual'))})</small></span>` +
      `<span class="badge ${e.ok ? 'ok' : 'bad'}">${e.ok ? esc(t('badge_ok')) : 'ERROR'}</span>`;
    box.appendChild(row);
  }
}

// ---------- ajustes ----------
async function loadSettings() {
  settings = await invoke('get_settings');
  $('setMax').value = settings.max_backups;
  $('setFormat').value = settings.format;
  $('setTxPort').value = settings.txadmin_port;
  $('setRestart').checked = settings.restart_after_update;
  $('setBackups').value = settings.backups_dir;
  $('setTx').value = settings.tx_data_path;
}
$('setSave').onclick = async () => {
  $('setErr').textContent = '';
  $('setInfo').textContent = '';
  const s = {
    ...settings,
    max_backups: parseInt($('setMax').value, 10) || 5,
    format: $('setFormat').value,
    txadmin_port: parseInt($('setTxPort').value, 10) || 40120,
    restart_after_update: $('setRestart').checked,
    backups_dir: $('setBackups').value.trim(),
    tx_data_path: $('setTx').value.trim(),
  };
  try {
    await invoke('save_settings', { settings: s });
    await loadSettings();
    $('setInfo').textContent = t('saved');
    await loadBackups();
  } catch (e) { $('setErr').textContent = terr(e); }
};

// ---------- idiomas ----------
function renderLangs() {
  const box = $('langList');
  box.innerHTML = '';
  for (const [code, name] of LANGS) {
    const b = document.createElement('button');
    b.className = code === LANG ? 'on' : '';
    b.innerHTML = `<i data-lucide="${code === LANG ? 'check' : 'globe'}"></i><span>${esc(name)}</span><span class="code">${code}</span>`;
    b.onclick = () => setLanguage(code, true);
    box.appendChild(b);
  }
  icons();
}

async function setLanguage(code, persist) {
  applyLang(code);
  renderLangs();
  if (persist && settings) {
    settings.lang = code;
    try { await invoke('save_settings', { settings }); } catch (e) { /* ignora */ }
  }
  if (lastInfo && !lastInfo.error) {
    renderInfo(lastInfo);
    renderKeep(lastInfo);
    await showSelStatus();
    await loadBackups();
    await loadTask();
    renderServer();
    if (!$('pane-log').hidden) loadLog();
  } else {
    $('path').textContent = lastInfo && lastInfo.error ? terr(lastInfo.error) : t('searching');
  }
  $('bkTitle').textContent = t('bk_title', settings ? settings.max_backups : 5);
}

// ---------- arranque + splash ----------
applyLang('es');
$('path').textContent = t('searching');
renderLangs();
icons();
const splashStart = Date.now();
(async () => {
  try {
    settings = await invoke('get_settings');
    if (settings.lang && settings.lang !== LANG) { applyLang(settings.lang); renderLangs(); }
  } catch (e) { /* sin carpeta: idioma por defecto */ }
  await refresh();
  await loadSettings().catch(() => {});
  await pollServer();
  try { (await invoke('server_logs')).forEach(appendLog); } catch (e) { /* sin historial */ }
  if (lastInfo && lastInfo.error) renderLangs();
})()
  .catch((e) => { $('path').textContent = terr(e); })
  .finally(() => {
    const wait = Math.max(0, 2000 - (Date.now() - splashStart));
    setTimeout(() => invoke('close_splash'), wait);
  });
