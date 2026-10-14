// Mock del backend de Tauri para probar la interfaz en un navegador, sin Rust ni FXServer.
// Se inyecta en la página antes de ui/app.js (ver test/index.html).
// Escenarios por parámetros de la URL: ?lang=ar&server=1&appupdate=1&fail=update|restore|server
(function () {
  const P = window.__MOCK_PARAMS || {};
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const listeners = {};
  const emit = (name, payload) => (listeners[name] || []).forEach((cb) => cb({ payload }));

  const state = {
    base: 'C:\\demo\\servidor',
    artifacts: 'C:\\demo\\servidor\\artifacts',
    installed: P.noversion ? null : '37175',
    recommended: '37287',
    running: P.server === '1',
    resources: ['chat', 'monitor', 'webpack', 'yarn'],
    versions: [
      { version: '37287', url: 'https://runtime.fivem.net/mock/37287/server.7z', tags: ['ultima', 'recomendada'] },
      { version: '37262', url: 'https://runtime.fivem.net/mock/37262/server.7z', tags: [] },
      { version: '37175', url: 'https://runtime.fivem.net/mock/37175/server.7z', tags: ['opcional'] },
      { version: '37100', url: 'https://runtime.fivem.net/mock/37100/server.7z', tags: [] },
    ],
    backups: [
      { name: '2026-10-12_101500_37100', version: '37100' },
      { name: '2026-10-13_183000_37150', version: '37150' },
    ],
    log: [
      { ts: Math.floor(Date.now() / 1000) - 86400 * 2, from: '37100', to: '37150', ok: true, msg: 'updated', source: 'manual' },
      { ts: Math.floor(Date.now() / 1000) - 86400, from: '37150', to: '37175', ok: false, msg: 'E_DOWNLOAD|https://runtime.fivem.net/...: HTTP 404', source: 'auto' },
    ],
    settings: {
      max_backups: 5, format: 'auto', backups_dir: '', restart_after_update: true,
      keep: ['chat', 'monitor'], tx_data_path: '', task_time: '05:00', lang: P.lang || 'es', txadmin_port: 40120,
    },
    tasks: { daily: false, logon: false },
    logTimer: null,
  };

  const ESC = '\x1b';
  const consoleLines = [
    `${ESC}[38;5;83m[           resources] ${ESC}[0m${ESC}[0mStarted resource oxmysql`,
    `${ESC}[38;5;83m[           resources] ${ESC}[0mStarted resource ox_lib`,
    `${ESC}[38;5;83m[           resources] ${ESC}[0mStarted resource qbx_core`,
    `${ESC}[33m[ script:ox_inventory] ${ESC}[0mWARNING: item "burger" has no image`,
    `${ESC}[31;1m[ script:qbx_police] ${ESC}[0mERROR: failed to load config`,
    `${ESC}[36m[             txAdmin] ${ESC}[0mServer started (mock)`,
  ];
  let lineIdx = 0;

  function startLogs() {
    stopLogs();
    state.logTimer = setInterval(() => {
      emit('server-log', consoleLines[lineIdx++ % consoleLines.length]);
    }, 900);
  }
  function stopLogs() { if (state.logTimer) clearInterval(state.logTimer); state.logTimer = null; }
  if (state.running) startLogs();

  async function runUpdate(keep, version) {
    const steps = [
      ['check', 0, [version]],
      ['download', 12, ['20', '180']], ['download', 30, ['54', '180']], ['download', 50, ['99', '180']],
      ['verify', 58, []], ['keep', 78, []],
    ];
    if (state.running) steps.push(['server_stop', 82, []]);
    steps.push(['backup', 86, []], ['install', 92, []]);
    for (const [stage, percent, args] of steps) {
      emit('progress', { stage, percent, args });
      await wait(450);
      if (P.fail === 'update' && stage === 'download' && percent === 30) throw 'E_DOWNLOAD|https://runtime.fivem.net/mock: HTTP 503';
    }
    state.backups.unshift({ name: '2026-10-14_120000_' + (state.installed || 'sin-version'), version: state.installed || 'sin-version' });
    state.log.unshift({ ts: Math.floor(Date.now() / 1000), from: state.installed || 'desconocida', to: version, ok: true, msg: 'updated', source: 'manual' });
    state.installed = version;
    if (state.running) { emit('progress', { stage: 'server_start', percent: 99, args: [] }); await wait(400); }
    emit('progress', { stage: 'done', percent: 100, args: [version] });
    return version;
  }

  const handlers = {
    close_splash: () => null,
    get_info: () => ({
      base: state.base, artifacts: state.artifacts, error: P.nofolder ? 'E_NO_ARTIFACTS' : null,
      installed: state.installed, resources: state.resources, recommended: state.recommended, net_error: null,
      installed_status: state.installed === '37100' ? 'BROKEN' : (state.installed ? 'OK' : null),
      installed_reason: state.installed === '37100' ? 'State bags not replicated to clients' : null,
      fx_running: state.running,
    }),
    set_base: () => null,
    list_versions: () => state.versions,
    check_version: ({ version }) => (version === '37100' ? { status: 'BROKEN', reason: 'State bags not replicated to clients' } : { status: 'OK' }),
    list_backups: () => state.backups,
    delete_backup: ({ name }) => { state.backups = state.backups.filter((b) => b.name !== name); return null; },
    update: ({ keep, version }) => runUpdate(keep, version),
    restore: async ({ name }) => {
      emit('progress', { stage: 'restore', percent: 50, args: [] });
      await wait(700);
      if (P.fail === 'restore') throw 'E_RESTORE_FAILED|access denied';
      const b = state.backups.find((x) => x.name === name);
      state.installed = b ? b.version : state.installed;
      emit('progress', { stage: 'done_restore', percent: 100, args: [state.installed] });
      return state.installed;
    },
    server_status: () => ({ running: state.running, pids: state.running ? [4242, 4243] : [], managed: true }),
    server_start: async () => {
      if (P.fail === 'server') throw 'E_NO_FXSERVER_EXE';
      await wait(500); state.running = true; lineIdx = 0; startLogs(); return 4242;
    },
    server_stop: async () => { await wait(400); state.running = false; stopLogs(); return true; },
    server_logs: () => [],
    open_txadmin: () => { console.log('[mock] open txAdmin http://localhost:' + state.settings.txadmin_port); return null; },
    create_txadmin_shortcut: () => 'C:\\Users\\demo\\Desktop\\txAdmin.url',
    get_settings: () => ({ ...state.settings }),
    save_settings: ({ settings }) => { state.settings = { ...state.settings, ...settings }; return null; },
    get_log: () => state.log,
    task_status: () => ({ daily: state.tasks.daily, logon: state.tasks.logon, exe: 'C:\\demo\\servidor\\cfx_artifacts.exe' }),
    task_create: ({ daily, logon }) => {
      if (!daily && !logon) throw 'E_TASK_NONE';
      state.tasks = { daily, logon }; return null;
    },
    task_delete: () => { state.tasks = { daily: false, logon: false }; return null; },
    check_app_update: () => ({
      current: '1.0.0', latest: P.appupdate === '1' ? '1.1.0' : '1.0.0',
      release: 'https://github.com/DFNTK/CFX-Artifacts/releases/latest', available: P.appupdate === '1',
    }),
    open_url: ({ url }) => { console.log('[mock] open url', url); window.open(url, '_blank'); return null; },
  };

  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        await wait(120);
        if (!handlers[cmd]) throw 'E_UNKNOWN_COMMAND|' + cmd;
        return handlers[cmd](args || {});
      },
    },
    event: {
      listen: async (name, cb) => {
        (listeners[name] = listeners[name] || []).push(cb);
        return () => { listeners[name] = (listeners[name] || []).filter((x) => x !== cb); };
      },
    },
  };
  window.__MOCK_STATE = state;
})();
