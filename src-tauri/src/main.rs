#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;

const API: &str = "https://artifacts.jgscripts.com";
const APP_VERSION_URL: &str = "https://raw.githubusercontent.com/DFNTK/CFX-Artifacts/main/version.md";
const VERSION_FILE: &str = "cfx_version.xml";
const LEGACY_VERSION_FILE: &str = "df_version.xml";
const BACKUPS_DIR: &str = "artifacts_backups";
const DATA_DIR: &str = "cfx_artifacts_data";
const TASK_DAILY: &str = "CFX Artifacts - Comprobar diario";
const TASK_LOGON: &str = "CFX Artifacts - Comprobar al iniciar sesion";
const ARTIFACT_NAMES: [&str; 5] = ["artifacts", "artifact", "fxserver", "fx-server", "fivem"];

static MANUAL_BASE: Mutex<Option<PathBuf>> = Mutex::new(None);
static SERVER: Mutex<Option<Child>> = Mutex::new(None);
static LOGBUF: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

// ---------- progreso ----------

#[derive(Serialize, Clone)]
struct Progress {
    stage: String,
    percent: f64,
    args: Vec<String>,
}

/// Emite progreso al frontend; en modo CLI (sin ventana) no hace nada.
struct Rep(Option<AppHandle>);

impl Rep {
    fn emit(&self, stage: &str, percent: f64, args: Vec<String>) {
        if let Some(app) = &self.0 {
            let _ = app.emit("progress", Progress { stage: stage.into(), percent, args });
        }
    }
}

fn hide(cmd: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

// ---------- ajustes ----------

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Settings {
    max_backups: usize,
    format: String,        // auto | zip | 7z
    backups_dir: String,   // vacio = <base>/artifacts_backups
    restart_after_update: bool,
    keep: Vec<String>,
    tx_data_path: String,  // vacio = <base>/txData
    task_time: String,
    lang: String,
    txadmin_port: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_backups: 5,
            format: "auto".into(),
            backups_dir: String::new(),
            restart_after_update: true,
            keep: vec!["chat".into(), "monitor".into()],
            tx_data_path: String::new(),
            task_time: "05:00".into(),
            lang: "es".into(),
            txadmin_port: 40120,
        }
    }
}

fn data_dir(base: &Path) -> PathBuf {
    let d = base.join(DATA_DIR);
    let _ = fs::create_dir_all(&d);
    d
}

fn load_settings(base: &Path) -> Settings {
    fs::read_to_string(data_dir(base).join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_settings_file(base: &Path, s: &Settings) -> Result<(), String> {
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    fs::write(data_dir(base).join("settings.json"), json).map_err(|e| e.to_string())
}

fn backups_root(base: &Path, s: &Settings) -> PathBuf {
    if s.backups_dir.trim().is_empty() {
        base.join(BACKUPS_DIR)
    } else {
        PathBuf::from(s.backups_dir.trim())
    }
}

// ---------- historial ----------

#[derive(Serialize, Deserialize, Clone)]
struct LogEntry {
    ts: u64,
    from: String,
    to: String,
    ok: bool,
    msg: String,
    source: String,
}

fn log_event(base: &Path, from: &str, to: &str, ok: bool, msg: &str, source: &str) {
    let e = LogEntry { ts: now_secs(), from: from.into(), to: to.into(), ok, msg: msg.into(), source: source.into() };
    if let Ok(line) = serde_json::to_string(&e) {
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(data_dir(base).join("update_log.jsonl")) {
            let _ = writeln!(f, "{line}");
        }
    }
}

// ---------- ubicacion ----------

fn is_artifacts_dir(p: &Path) -> bool {
    p.join("FXServer.exe").is_file() || p.join("citizen").join("system_resources").is_dir()
}

fn artifacts_in(dir: &Path) -> Option<PathBuf> {
    let mut fallback: Option<PathBuf> = None;
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name == BACKUPS_DIR || !is_artifacts_dir(&p) {
            continue;
        }
        if ARTIFACT_NAMES.contains(&name.as_str()) {
            return Some(p);
        }
        if fallback.is_none() {
            fallback = Some(p);
        }
    }
    fallback
}

/// Escanea `dir`, luego sus subcarpetas, buscando una carpeta artifacts.
fn scan_down(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    if let Some(a) = artifacts_in(dir) {
        return Some((dir.to_path_buf(), a));
    }
    for e in fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !p.is_dir() || name == BACKUPS_DIR || name == DATA_DIR.to_lowercase() || is_artifacts_dir(&p) || name.starts_with('.') {
            continue;
        }
        if let Some(a) = artifacts_in(&p) {
            return Some((p, a));
        }
    }
    None
}

fn locate() -> Result<(PathBuf, PathBuf), String> {
    if let Some(b) = MANUAL_BASE.lock().unwrap().clone() {
        return artifacts_in(&b)
            .map(|a| (b.clone(), a))
            .ok_or_else(|| "E_BASE_NO_ARTIFACTS".to_string());
    }
    if let Ok(b) = std::env::var("CFX_BASE").or_else(|_| std::env::var("DF_BASE")) {
        let b = PathBuf::from(b);
        if let Some(a) = artifacts_in(&b) {
            return Ok((b, a));
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    for dir in exe.ancestors().skip(1).take(6) {
        if let Some(found) = scan_down(dir) {
            return Ok(found);
        }
    }
    Err("E_NO_ARTIFACTS".into())
}

// ---------- utilidades ----------

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn stamp() -> String {
    let s = now_secs() as i64;
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}_{:02}{:02}{:02}", y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let (from, to) = (e.path(), dst.join(e.file_name()));
        if e.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Mueve una carpeta; si es otro disco (rename falla) copia y borra el origen.
fn move_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_dir(src, dst).map_err(|e| format!("E_COPY|{}|{e}", dst.display()))?;
    fs::remove_dir_all(src).map_err(|e| format!("E_REMOVE_SRC|{e}"))
}

fn read_version(art: &Path) -> Option<String> {
    let t = fs::read_to_string(art.join(VERSION_FILE))
        .or_else(|_| fs::read_to_string(art.join(LEGACY_VERSION_FILE)))
        .ok()?;
    let a = t.find("<version>")? + 9;
    let b = t[a..].find("</version>")? + a;
    let v = t[a..b].trim().to_string();
    (!v.is_empty()).then_some(v)
}

fn write_version(art: &Path, version: &str) -> Result<(), String> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<artifact>\r\n  <version>{}</version>\r\n  <updated>{}</updated>\r\n</artifact>\r\n",
        version,
        stamp()
    );
    fs::write(art.join(VERSION_FILE), xml).map_err(|e| e.to_string())
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && !n.contains(['/', '\\', ':']) && n != ".." && n != "."
}

fn sorted_backups(root: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn prune_backups(root: &Path, max: usize) {
    let max = max.max(1);
    let list = sorted_backups(root);
    if list.len() > max {
        for old in &list[..list.len() - max] {
            let _ = fs::remove_dir_all(root.join(old));
        }
    }
}

fn move_to_backup(root: &Path, art: &Path, tag: &str) -> Result<String, String> {
    fs::create_dir_all(root).map_err(|e| format!("E_BACKUP_DIR|{e}"))?;
    let ver = read_version(art).unwrap_or_else(|| "sin-version".into());
    let name = format!("{}_{}{}", stamp(), ver, tag);
    move_dir(art, &root.join(&name))
        .map_err(|e| format!("E_BACKUP_MOVE|{e}"))?;
    Ok(name)
}

// ---------- servidor ----------

fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_start_matches("\\\\?\\").to_lowercase()
}

/// PIDs de FXServer.exe que salen de la carpeta artifacts indicada.
fn fx_pids(art: &Path) -> Vec<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
    );
    let root = norm(art);
    let mut out = vec![];
    for (pid, p) in sys.processes() {
        if p.name().to_string_lossy().eq_ignore_ascii_case("FXServer.exe") {
            if let Some(exe) = p.exe() {
                if norm(exe).starts_with(&root) {
                    out.push(pid.as_u32());
                }
            }
        }
    }
    out
}

fn push_log(app: &Option<AppHandle>, line: String) {
    {
        let mut b = LOGBUF.lock().unwrap();
        if b.len() >= 1000 {
            b.pop_front();
        }
        b.push_back(line.clone());
    }
    if let Some(a) = app {
        let _ = a.emit("server-log", line);
    }
}

/// Lee lineas en bruto (conserva los codigos ANSI) y tolera bytes que no son UTF-8.
fn pump_lines<R: std::io::Read>(r: R, app: &Option<AppHandle>) {
    let mut rd = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match rd.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let s = String::from_utf8_lossy(&buf);
                push_log(app, s.trim_end_matches(['\r', '\n']).to_string());
            }
        }
    }
}

fn start_server(app: Option<AppHandle>, base: &Path, art: &Path) -> Result<u32, String> {
    if !fx_pids(art).is_empty() {
        return Err("E_ALREADY_RUNNING".into());
    }
    let exe = art.join("FXServer.exe");
    if !exe.is_file() {
        return Err("E_NO_FXSERVER_EXE".into());
    }
    let s = load_settings(base);
    let data = if s.tx_data_path.trim().is_empty() {
        let d = base.join("txData");
        if d.is_dir() { d } else { base.to_path_buf() }
    } else {
        PathBuf::from(s.tx_data_path.trim())
    };
    let mut cmd = Command::new(&exe);
    cmd.current_dir(art)
        .env("TXHOST_DATA_PATH", &data)
        .env("TXHOST_TXA_PORT", s.txadmin_port.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("E_START_FAILED|{e}"))?;
    let pid = child.id();
    push_log(&app, format!("[cfx-artifacts] Iniciando {} (datos: {})", exe.display(), data.display()));
    if let Some(out) = child.stdout.take() {
        let a = app.clone();
        std::thread::spawn(move || {
            pump_lines(out, &a);
        });
    }
    if let Some(err) = child.stderr.take() {
        let a = app.clone();
        std::thread::spawn(move || {
            pump_lines(err, &a);
        });
    }
    *SERVER.lock().unwrap() = Some(child);
    Ok(pid)
}

fn taskkill(pid: u32, force: bool) {
    let mut c = Command::new("taskkill");
    c.args(["/PID", &pid.to_string(), "/T"]);
    if force {
        c.arg("/F");
    }
    hide(&mut c);
    let _ = c.output();
}

/// Para el servidor (arbol de procesos). Devuelve true si estaba en ejecucion.
fn stop_server(art: &Path) -> bool {
    let pids = fx_pids(art);
    if pids.is_empty() {
        *SERVER.lock().unwrap() = None;
        return false;
    }
    for p in &pids {
        taskkill(*p, false);
    }
    for _ in 0..20 {
        if fx_pids(art).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    for p in fx_pids(art) {
        taskkill(p, true);
    }
    for _ in 0..10 {
        if fx_pids(art).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    if let Some(mut c) = SERVER.lock().unwrap().take() {
        let _ = c.kill();
        let _ = c.wait();
    }
    true
}

// ---------- red ----------

async fn fetch_json(path: &str) -> Result<serde_json::Value, String> {
    reqwest::get(format!("{API}{path}"))
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())
}

fn version_from_link(link: &str) -> Option<String> {
    link.split('/').find_map(|s| {
        let (n, h) = s.split_once('-')?;
        (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && h.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| n.to_string())
    })
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    let mut magic = [0u8; 6];
    {
        use std::io::Read;
        fs::File::open(archive).and_then(|mut f| f.read_exact(&mut magic)).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    if magic[..2] == *b"PK" {
        let file = fs::File::open(archive).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        zip.extract(dest).map_err(|e| format!("E_CORRUPT|{e}"))
    } else if magic == [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C] {
        sevenz_rust2::decompress_file(archive, dest).map_err(|e| format!("E_CORRUPT|{e}"))
    } else {
        Err(format!("E_UNKNOWN_FORMAT|{:?}", magic))
    }
}

async fn download(rep: &Rep, link: &str, dest: &Path) -> Result<(), String> {
    let resp = reqwest::get(link).await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(dest).await.map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let (mut got, mut last) = (0u64, 0.0f64);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        got += chunk.len() as u64;
        if total > 0 {
            let pct = got as f64 / total as f64 * 100.0;
            if pct - last >= 1.0 {
                last = pct;
                rep.emit("download", pct * 0.55, vec![format!("{:.0}", got as f64 / 1e6), format!("{:.0}", total as f64 / 1e6)]);
            }
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    drop(file);
    // verificacion 1: tamano exacto
    if total > 0 && got != total {
        return Err(format!("E_INCOMPLETE|{got}|{total}"));
    }
    Ok(())
}

fn url_candidates(url: &str, fmt: &str) -> Vec<String> {
    let dir = url.rsplit_once('/').map(|(d, _)| d).unwrap_or(url);
    let zip = format!("{dir}/server.zip");
    let sz = format!("{dir}/server.7z");
    match fmt {
        "zip" => vec![zip, sz],
        "7z" => vec![sz, zip],
        _ => {
            let other = if url.ends_with(".zip") { sz } else { zip };
            vec![url.to_string(), other]
        }
    }
}

// ---------- actualizacion (compartida por GUI y CLI) ----------

async fn update_core(rep: &Rep, app: Option<AppHandle>, keep: Vec<String>, url: String, version: String, source: &str) -> Result<String, String> {
    let (base, art) = locate()?;
    let from = read_version(&art).unwrap_or_else(|| "desconocida".into());
    let was_running = !fx_pids(&art).is_empty();
    let settings = load_settings(&base);
    let res = update_inner(rep, &base, &art, &settings, keep, url, version.clone()).await;
    match &res {
        Ok(_) => log_event(&base, &from, &version, true, "updated", source),
        Err(e) => log_event(&base, &from, &version, false, e, source),
    }
    if was_running && settings.restart_after_update {
        let (b, a) = locate().unwrap_or((base.clone(), art.clone()));
        rep.emit("server_start", 99.0, vec![]);
        let _ = start_server(app, &b, &a);
    }
    if res.is_ok() {
        rep.emit("done", 100.0, vec![version.clone()]);
    }
    res
}

async fn update_inner(rep: &Rep, base: &Path, art: &Path, settings: &Settings, keep: Vec<String>, url: String, version: String) -> Result<String, String> {
    if let Some(bad) = keep.iter().find(|k| !valid_name(k)) {
        return Err(format!("E_INVALID_NAME|{bad}"));
    }
    if !url.starts_with("https://runtime.fivem.net/") || version.is_empty() || !version.chars().all(|c| c.is_ascii_digit()) {
        return Err("E_BAD_URL".into());
    }

    let tmp = std::env::temp_dir().join(format!("cfx_artifacts_{}", now_secs()));
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let archive = tmp.join("server.pkg");
    let extract = tmp.join("extract");

    rep.emit("check", 0.0, vec![version.clone()]);
    let mut last_err = String::new();
    let mut ok = false;
    for u in url_candidates(&url, &settings.format) {
        match download(rep, &u, &archive).await {
            Ok(()) => { ok = true; break; }
            Err(e) => last_err = format!("{u}: {e}"),
        }
    }
    if !ok {
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("E_DOWNLOAD|{last_err}"));
    }

    rep.emit("verify", 58.0, vec![]);
    let (a, e) = (archive.clone(), extract.clone());
    let r = tokio::task::spawn_blocking(move || extract_archive(&a, &e)).await.map_err(|e| e.to_string())?;
    if let Err(e) = r {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let _ = fs::remove_file(&archive);
    if !extract.join("FXServer.exe").is_file() {
        let _ = fs::remove_dir_all(&tmp);
        return Err("E_NO_FXSERVER_IN_PKG".into());
    }

    rep.emit("keep", 78.0, vec![]);
    let rel = Path::new("citizen").join("system_resources");
    for k in &keep {
        let src = art.join(&rel).join(k);
        if src.is_dir() {
            let dst = extract.join(&rel).join(k);
            if dst.exists() {
                fs::remove_dir_all(&dst).map_err(|e| e.to_string())?;
            }
            copy_dir(&src, &dst).map_err(|e| format!("E_COPY|{k}|{e}"))?;
        }
    }

    // a partir de aqui se toca la instalacion: parar el servidor si corre
    if !fx_pids(art).is_empty() {
        rep.emit("server_stop", 82.0, vec![]);
        stop_server(art);
        if !fx_pids(art).is_empty() {
            let _ = fs::remove_dir_all(&tmp);
            return Err("E_STOP_FAILED".into());
        }
    }

    rep.emit("backup", 86.0, vec![]);
    let root = backups_root(base, settings);
    let bname = move_to_backup(&root, art, "")?;

    rep.emit("install", 92.0, vec![]);
    if let Err(e) = copy_dir(&extract, art) {
        let _ = fs::remove_dir_all(art);
        let _ = move_dir(&root.join(&bname), art);
        let _ = fs::remove_dir_all(&tmp);
        return Err(format!("E_INSTALL_FAILED|{e}"));
    }
    write_version(art, &version)?;
    prune_backups(&root, settings.max_backups);
    let _ = fs::remove_dir_all(&tmp);
    Ok(version)
}

// ---------- comandos ----------

#[derive(Serialize)]
struct Info {
    base: Option<String>,
    artifacts: Option<String>,
    error: Option<String>,
    installed: Option<String>,
    resources: Vec<String>,
    recommended: Option<String>,
    net_error: Option<String>,
    installed_status: Option<String>,
    installed_reason: Option<String>,
    fx_running: bool,
}

#[tauri::command]
async fn get_info() -> Info {
    let mut info = Info {
        base: None, artifacts: None, error: None, installed: None, resources: vec![],
        recommended: None, net_error: None, installed_status: None, installed_reason: None,
        fx_running: false,
    };
    match locate() {
        Ok((base, art)) => {
            info.base = Some(base.to_string_lossy().into());
            info.installed = read_version(&art);
            info.fx_running = !fx_pids(&art).is_empty();
            if let Ok(rd) = fs::read_dir(art.join("citizen").join("system_resources")) {
                let mut r: Vec<String> = rd.flatten().filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().to_string()).collect();
                r.sort();
                info.resources = r;
            }
            info.artifacts = Some(art.to_string_lossy().into());
        }
        Err(e) => info.error = Some(e),
    }
    match fetch_json("/jsonv2").await {
        Ok(j) => info.recommended = j["recommendedArtifact"].as_str().map(String::from),
        Err(e) => info.net_error = Some(e),
    }
    if let Some(v) = &info.installed {
        if let Ok(j) = fetch_json(&format!("/check?artifact={v}")).await {
            info.installed_status = j["status"].as_str().map(String::from);
            info.installed_reason = j["reason"].as_str().map(String::from);
        }
    }
    info
}

#[derive(Serialize)]
struct VersionEntry {
    version: String,
    url: String,
    tags: Vec<String>,
}

async fn collect_versions() -> Vec<VersionEntry> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<u64, VersionEntry> = BTreeMap::new();
    let mut add = |version: String, url: String, tag: Option<&str>| {
        if let Ok(n) = version.parse::<u64>() {
            let e = map.entry(n).or_insert(VersionEntry { version: version.clone(), url: url.clone(), tags: vec![] });
            if let Some(t) = tag {
                if !e.tags.iter().any(|x| x == t) { e.tags.push(t.into()); }
            }
        }
    };
    if let Ok(j) = fetch_json("/jsonv2").await {
        if let Some(l) = j["windowsDownloadLink"].as_str() {
            if let Some(v) = j["recommendedArtifact"].as_str().map(String::from).or_else(|| version_from_link(l)) {
                add(v, l.to_string(), Some("recomendada"));
            }
        }
    }
    if let Ok(r) = reqwest::get("https://changelogs-live.fivem.net/api/changelog/versions/win32/server").await {
        if let Ok(j) = r.json::<serde_json::Value>().await {
            for (key, tag) in [("latest", "ultima"), ("recommended", "cfx-recomendada"), ("optional", "opcional"), ("critical", "critica")] {
                if let Some(l) = j[format!("{key}_download")].as_str() {
                    if let Some(v) = version_from_link(l) { add(v, l.to_string(), Some(tag)); }
                }
            }
        }
    }
    if let Ok(r) = reqwest::get("https://runtime.fivem.net/artifacts/fivem/build_server_windows/master/").await {
        if let Ok(html) = r.text().await {
            let marker = "build_server_windows/master/";
            let mut rest = html.as_str();
            while let Some(i) = rest.find(marker) {
                rest = &rest[i + marker.len()..];
                let seg: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
                if let Some((n, h)) = seg.split_once('-') {
                    if h.len() == 40 && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
                        add(n.to_string(), format!("https://runtime.fivem.net/artifacts/fivem/{marker}{seg}/server.7z"), None);
                    }
                }
            }
        }
    }
    let mut v: Vec<VersionEntry> = map.into_values().collect();
    v.reverse();
    v
}

#[tauri::command]
async fn list_versions() -> Result<Vec<VersionEntry>, String> {
    let v = collect_versions().await;
    if v.is_empty() { return Err("E_NO_LIST".into()); }
    Ok(v)
}

#[tauri::command]
async fn check_version(version: String) -> Result<serde_json::Value, String> {
    if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit()) { return Err("E_BAD_VERSION".into()); }
    fetch_json(&format!("/check?artifact={version}")).await
}

#[tauri::command]
fn set_base(path: String) -> Result<(), String> {
    let p = PathBuf::from(path.trim());
    if artifacts_in(&p).is_none() {
        return Err("E_BASE_NO_ARTIFACTS".into());
    }
    *MANUAL_BASE.lock().unwrap() = Some(p);
    Ok(())
}

#[derive(Serialize)]
struct Backup {
    name: String,
    version: String,
}

#[tauri::command]
fn list_backups() -> Result<Vec<Backup>, String> {
    let (base, _) = locate()?;
    let root = backups_root(&base, &load_settings(&base));
    let mut v: Vec<Backup> = sorted_backups(&root)
        .into_iter()
        .map(|n| {
            let version = n.splitn(3, '_').nth(2).unwrap_or("").to_string();
            Backup { name: n, version }
        })
        .collect();
    v.reverse();
    Ok(v)
}

#[tauri::command]
fn delete_backup(name: String) -> Result<(), String> {
    let (base, _) = locate()?;
    if !valid_name(&name) {
        return Err("E_INVALID_NAME|".into());
    }
    let p = backups_root(&base, &load_settings(&base)).join(&name);
    if !p.is_dir() {
        return Err("E_BACKUP_MISSING".into());
    }
    fs::remove_dir_all(&p).map_err(|e| format!("E_DELETE_FAILED|{e}"))
}

#[tauri::command]
async fn update(app: AppHandle, keep: Vec<String>, url: String, version: String) -> Result<String, String> {
    if let Ok((base, _)) = locate() {
        let mut s = load_settings(&base);
        s.keep = keep.clone();
        let _ = save_settings_file(&base, &s);
    }
    let rep = Rep(Some(app.clone()));
    update_core(&rep, Some(app), keep, url, version, "manual").await
}

#[tauri::command]
async fn restore(app: AppHandle, name: String) -> Result<String, String> {
    let (base, art) = locate()?;
    if !valid_name(&name) {
        return Err("E_INVALID_NAME|".into());
    }
    let settings = load_settings(&base);
    let root = backups_root(&base, &settings);
    let src = root.join(&name);
    if !src.is_dir() {
        return Err("E_BACKUP_MISSING".into());
    }
    let from = read_version(&art).unwrap_or_else(|| "desconocida".into());
    let was_running = !fx_pids(&art).is_empty();
    let rep = Rep(Some(app.clone()));
    if was_running {
        rep.emit("server_stop", 20.0, vec![]);
        stop_server(&art);
    }
    rep.emit("restore", 50.0, vec![]);
    let res: Result<String, String> = (|| {
        let bname = move_to_backup(&root, &art, "_antes-de-restaurar")?;
        if let Err(e) = move_dir(&src, &art) {
            let _ = fs::remove_dir_all(&art);
            let _ = move_dir(&root.join(&bname), &art);
            return Err(format!("E_RESTORE_FAILED|{e}"));
        }
        prune_backups(&root, settings.max_backups);
        Ok(read_version(&art).unwrap_or_else(|| "sin-version".into()))
    })();
    match &res {
        Ok(v) => log_event(&base, &from, v, true, "restored", "manual"),
        Err(e) => log_event(&base, &from, "-", false, e, "manual"),
    }
    if was_running && settings.restart_after_update {
        let _ = start_server(Some(app), &base, &art);
    }
    if let Ok(v) = &res {
        rep.emit("done_restore", 100.0, vec![v.clone()]);
    }
    res
}

#[tauri::command]
fn close_splash(app: AppHandle) {
    use tauri::Manager;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.show();
        let _ = main.set_focus();
    }
    if let Some(splash) = app.get_webview_window("splash") {
        let _ = splash.close();
    }
}

// ---- servidor ----

#[derive(Serialize)]
struct ServerStatus {
    running: bool,
    pids: Vec<u32>,
    managed: bool,
}

#[tauri::command]
fn server_status() -> Result<ServerStatus, String> {
    let (_, art) = locate()?;
    let pids = fx_pids(&art);
    let mut managed = false;
    {
        let mut g = SERVER.lock().unwrap();
        if let Some(c) = g.as_mut() {
            match c.try_wait() {
                Ok(None) => managed = true,
                _ => *g = None,
            }
        }
    }
    Ok(ServerStatus { running: !pids.is_empty(), pids, managed })
}

#[tauri::command]
fn server_start(app: AppHandle) -> Result<u32, String> {
    let (base, art) = locate()?;
    start_server(Some(app), &base, &art)
}

#[tauri::command]
fn server_stop() -> Result<bool, String> {
    let (_, art) = locate()?;
    Ok(stop_server(&art))
}

#[tauri::command]
fn server_logs() -> Vec<String> {
    LOGBUF.lock().unwrap().iter().cloned().collect()
}

// ---- ajustes / historial ----

#[derive(Serialize)]
struct AppUpdate {
    current: String,
    latest: String,
    release: String,
    available: bool,
}

/// Lee `key = value` de version.md.
fn parse_kv(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let (k, v) = l.split_once('=')?;
        (k.trim().eq_ignore_ascii_case(key)).then(|| v.trim().to_string())
    })
}

/// Compara versiones tipo "1.0" / "1.0.2" rellenando con ceros.
fn version_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> { s.trim().trim_start_matches(['v', 'V']).split('.').map(|p| p.trim().parse().unwrap_or(0)).collect() };
    let (a, b) = (parse(latest), parse(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

#[tauri::command]
async fn check_app_update() -> Result<AppUpdate, String> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let text = reqwest::get(APP_VERSION_URL)
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let latest = parse_kv(&text, "version").ok_or("E_BAD_VERSION")?;
    let release = parse_kv(&text, "release").unwrap_or_default();
    let available = version_newer(&latest, &current);
    Ok(AppUpdate { current, latest, release, available })
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") || url.contains(['"', ' ', '<', '>', '^', '&', '|']) {
        return Err("E_BAD_URL".into());
    }
    let mut c = Command::new("rundll32");
    c.args(["url.dll,FileProtocolHandler", &url]);
    hide(&mut c);
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_txadmin() -> Result<(), String> {
    let (base, _) = locate()?;
    let url = format!("http://localhost:{}", load_settings(&base).txadmin_port);
    let mut c = Command::new("rundll32");
    c.args(["url.dll,FileProtocolHandler", &url]);
    hide(&mut c);
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn create_txadmin_shortcut() -> Result<String, String> {
    let (base, _) = locate()?;
    let url = format!("http://localhost:{}", load_settings(&base).txadmin_port);
    let mut c = Command::new("powershell.exe");
    c.args(["-NoProfile", "-Command", "[Environment]::GetFolderPath('Desktop')"]);
    hide(&mut c);
    let out = c.output().map_err(|e| e.to_string())?;
    let desktop = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if desktop.is_empty() {
        return Err("E_NO_DESKTOP".into());
    }
    let icon = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let path = PathBuf::from(desktop).join("txAdmin.url");
    let body = format!("[InternetShortcut]\r\nURL={url}\r\nIconFile={icon}\r\nIconIndex=0\r\n");
    fs::write(&path, body).map_err(|e| format!("E_SHORTCUT|{e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_settings() -> Result<Settings, String> {
    let (base, _) = locate()?;
    Ok(load_settings(&base))
}

#[tauri::command]
fn save_settings(settings: Settings) -> Result<(), String> {
    let (base, _) = locate()?;
    let mut s = settings;
    s.max_backups = s.max_backups.clamp(1, 50);
    if !["auto", "zip", "7z"].contains(&s.format.as_str()) {
        s.format = "auto".into();
    }
    if s.txadmin_port == 0 {
        s.txadmin_port = 40120;
    }
    if !["es", "en", "pt", "de", "fr", "ru", "uk", "ar", "ro", "it", "tr", "pl", "zh", "ja"].contains(&s.lang.as_str()) {
        s.lang = "es".into();
    }
    if !s.backups_dir.trim().is_empty() {
        fs::create_dir_all(s.backups_dir.trim()).map_err(|e| format!("E_BACKUP_DIR|{e}"))?;
    }
    save_settings_file(&base, &s)
}

#[tauri::command]
fn get_log() -> Result<Vec<LogEntry>, String> {
    let (base, _) = locate()?;
    let text = fs::read_to_string(data_dir(&base).join("update_log.jsonl")).unwrap_or_default();
    let mut v: Vec<LogEntry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v.truncate(100);
    Ok(v)
}

// ---- tareas programadas ----

#[derive(Serialize)]
struct TaskStatus {
    daily: bool,
    logon: bool,
    exe: String,
}

fn task_exists(name: &str) -> bool {
    let mut c = Command::new("schtasks");
    c.args(["/Query", "/TN", name]);
    hide(&mut c);
    c.output().map(|o| o.status.success()).unwrap_or(false)
}

#[tauri::command]
fn task_status() -> TaskStatus {
    TaskStatus {
        daily: task_exists(TASK_DAILY),
        logon: task_exists(TASK_LOGON),
        exe: std::env::current_exe().map(|p| p.to_string_lossy().into()).unwrap_or_default(),
    }
}

fn schtasks(args: &[&str]) -> Result<(), String> {
    let mut c = Command::new("schtasks");
    c.args(args);
    hide(&mut c);
    let o = c.output().map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&[o.stderr, o.stdout].concat()).trim().to_string())
    }
}

#[tauri::command]
fn task_create(time: String, daily: bool, logon: bool) -> Result<(), String> {
    if !daily && !logon {
        return Err("E_TASK_NONE".into());
    }
    let ok_time = time.len() == 5 && time.as_bytes()[2] == b':' && time.chars().enumerate().all(|(i, c)| i == 2 || c.is_ascii_digit());
    if daily && !ok_time {
        return Err("E_TIME_INVALID".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let tr = format!("\"{}\" --check", exe.display());
    if daily {
        schtasks(&["/Create", "/TN", TASK_DAILY, "/TR", &tr, "/SC", "DAILY", "/ST", &time, "/F"])?;
        if let Ok((base, _)) = locate() {
            let mut s = load_settings(&base);
            s.task_time = time.clone();
            let _ = save_settings_file(&base, &s);
        }
    } else if task_exists(TASK_DAILY) {
        let _ = schtasks(&["/Delete", "/TN", TASK_DAILY, "/F"]);
    }
    if logon {
        schtasks(&["/Create", "/TN", TASK_LOGON, "/TR", &tr, "/SC", "ONLOGON", "/F"])?;
    } else if task_exists(TASK_LOGON) {
        let _ = schtasks(&["/Delete", "/TN", TASK_LOGON, "/F"]);
    }
    Ok(())
}

#[tauri::command]
fn task_delete() -> Result<(), String> {
    for t in [TASK_DAILY, TASK_LOGON] {
        if task_exists(t) {
            schtasks(&["/Delete", "/TN", t, "/F"])?;
        }
    }
    Ok(())
}

// ---------- modo linea de comandos ----------

fn msgbox(title: &str, text: &str, yes_no: bool) -> bool {
    let script = if yes_no {
        "Add-Type -AssemblyName PresentationFramework; $r=[System.Windows.MessageBox]::Show($env:CFX_MSG,$env:CFX_TITLE,'YesNo','Question'); if($r -eq 'Yes'){exit 0}else{exit 1}"
    } else {
        "Add-Type -AssemblyName PresentationFramework; [void][System.Windows.MessageBox]::Show($env:CFX_MSG,$env:CFX_TITLE,'OK','Information')"
    };
    let mut c = Command::new("powershell.exe");
    c.args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script])
        .env("CFX_MSG", text)
        .env("CFX_TITLE", title);
    hide(&mut c);
    c.status().map(|s| s.success()).unwrap_or(false)
}

/// Textos de las ventanas del modo --check, por idioma.
/// Orden: 0 titulo, 1 disponible ({v},{i},{b}), 2 con fallos, 3 conservara ({k}), 4 backup,
/// 5 servidor en marcha, 6 pregunta, 7 ok ({v}), 8 fallo, 9 desconocida.
fn mb(lang: &str) -> [&'static str; 10] {
    match lang {
        "en" => ["CFX Artifacts - Update available", "A recommended artifacts build is available: {v}.\nInstalled: {i}{b}.", " (reported issues)", "Will keep: {k}.", "A full backup will be created.", "The server is running: it will be stopped and restarted.", "Update now?", "Updated to build {v}.", "The update failed:", "unknown"],
        "pt" => ["CFX Artifacts - Atualização disponível", "Há uma versão recomendada dos artifacts: {v}.\nInstalada: {i}{b}.", " (com problemas reportados)", "Será mantido: {k}.", "Será criado um backup completo.", "O servidor está em execução: será parado e reiniciado.", "Atualizar agora?", "Atualizado para a versão {v}.", "A atualização falhou:", "desconhecida"],
        "de" => ["CFX Artifacts - Update verfügbar", "Eine empfohlene Artifacts-Version ist verfügbar: {v}.\nInstalliert: {i}{b}.", " (gemeldete Probleme)", "Beibehalten wird: {k}.", "Es wird ein vollständiges Backup erstellt.", "Der Server läuft: er wird gestoppt und neu gestartet.", "Jetzt aktualisieren?", "Auf Version {v} aktualisiert.", "Das Update ist fehlgeschlagen:", "unbekannt"],
        "fr" => ["CFX Artifacts - Mise à jour disponible", "Une version recommandée des artifacts est disponible : {v}.\nInstallée : {i}{b}.", " (problèmes signalés)", "Sera conservé : {k}.", "Une sauvegarde complète sera créée.", "Le serveur est en cours d'exécution : il sera arrêté puis redémarré.", "Mettre à jour maintenant ?", "Mis à jour vers la version {v}.", "La mise à jour a échoué :", "inconnue"],
        "ru" => ["CFX Artifacts - Доступно обновление", "Доступна рекомендуемая версия artifacts: {v}.\nУстановлена: {i}{b}.", " (есть сообщения о проблемах)", "Будет сохранено: {k}.", "Будет создана полная резервная копия.", "Сервер запущен: он будет остановлен и перезапущен.", "Обновить сейчас?", "Обновлено до версии {v}.", "Не удалось обновить:", "неизвестно"],
        "uk" => ["CFX Artifacts - Доступне оновлення", "Доступна рекомендована версія artifacts: {v}.\nВстановлена: {i}{b}.", " (є повідомлення про проблеми)", "Буде збережено: {k}.", "Буде створено повну резервну копію.", "Сервер запущено: його буде зупинено та перезапущено.", "Оновити зараз?", "Оновлено до версії {v}.", "Не вдалося оновити:", "невідомо"],
        "ar" => ["CFX Artifacts - تحديث متاح", "يتوفر إصدار موصى به من artifacts: {v}.\nالمثبّت: {i}{b}.", " (توجد مشكلات مُبلَّغ عنها)", "سيتم الاحتفاظ بـ: {k}.", "سيتم إنشاء نسخة احتياطية كاملة.", "الخادم قيد التشغيل: سيتم إيقافه ثم إعادة تشغيله.", "هل تريد التحديث الآن؟", "تم التحديث إلى الإصدار {v}.", "فشل التحديث:", "غير معروف"],
        "ro" => ["CFX Artifacts - Actualizare disponibilă", "Este disponibilă o versiune recomandată de artifacts: {v}.\nInstalată: {i}{b}.", " (probleme raportate)", "Se vor păstra: {k}.", "Se va crea o copie de siguranță completă.", "Serverul rulează: va fi oprit și repornit.", "Actualizezi acum?", "Actualizat la versiunea {v}.", "Actualizarea a eșuat:", "necunoscută"],
        "it" => ["CFX Artifacts - Aggiornamento disponibile", "È disponibile una versione consigliata degli artifacts: {v}.\nInstallata: {i}{b}.", " (problemi segnalati)", "Verrà mantenuto: {k}.", "Verrà creato un backup completo.", "Il server è in esecuzione: verrà arrestato e riavviato.", "Aggiornare ora?", "Aggiornato alla versione {v}.", "Aggiornamento non riuscito:", "sconosciuta"],
        "tr" => ["CFX Artifacts - Güncelleme mevcut", "Önerilen bir artifacts sürümü mevcut: {v}.\nYüklü: {i}{b}.", " (bildirilen sorunlar var)", "Korunacak: {k}.", "Tam bir yedek oluşturulacak.", "Sunucu çalışıyor: durdurulacak ve yeniden başlatılacak.", "Şimdi güncellensin mi?", "{v} sürümüne güncellendi.", "Güncelleme başarısız oldu:", "bilinmiyor"],
        "pl" => ["CFX Artifacts - Dostępna aktualizacja", "Dostępna jest zalecana wersja artifacts: {v}.\nZainstalowana: {i}{b}.", " (zgłoszone problemy)", "Zachowane zostanie: {k}.", "Zostanie utworzona pełna kopia zapasowa.", "Serwer działa: zostanie zatrzymany i uruchomiony ponownie.", "Zaktualizować teraz?", "Zaktualizowano do wersji {v}.", "Aktualizacja nie powiodła się:", "nieznana"],
        "zh" => ["CFX Artifacts - 有可用更新", "有推荐的 artifacts 版本：{v}。\n已安装：{i}{b}。", "（存在已报告的问题）", "将保留：{k}。", "将创建完整备份。", "服务器正在运行：将先停止再重新启动。", "现在更新吗？", "已更新到版本 {v}。", "更新失败：", "未知"],
        "ja" => ["CFX Artifacts - アップデートがあります", "推奨の artifacts バージョンがあります：{v}。\nインストール済み：{i}{b}。", "（問題が報告されています）", "保持するもの：{k}。", "完全なバックアップを作成します。", "サーバーが実行中です。停止してから再起動します。", "今すぐ更新しますか？", "バージョン {v} に更新しました。", "更新に失敗しました：", "不明"],
        _ => ["CFX Artifacts - Actualización disponible", "Hay una versión recomendada de artifacts: {v}.\nInstalada: {i}{b}.", " (con fallos reportados)", "Se conservará: {k}.", "Se creará un backup completo.", "El servidor está en ejecución: se parará y se reiniciará.", "¿Actualizar ahora?", "Actualizado a la versión {v}.", "La actualización falló:", "desconocida"],
    }
}

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

async fn pick_version(args: &[String]) -> Result<(String, String), String> {
    let list = collect_versions().await;
    let want = arg_value(args, "--version");
    let found = match &want {
        Some(v) => list.into_iter().find(|e| &e.version == v),
        None => list.into_iter().find(|e| e.tags.iter().any(|t| t == "recomendada")),
    };
    found.map(|e| (e.version, e.url)).ok_or_else(|| "E_VERSION_NOT_FOUND".to_string())
}

async fn cli_main(args: Vec<String>) -> i32 {
    let (base, art) = match locate() {
        Ok(x) => x,
        Err(e) => {
            if args.iter().any(|a| a == "--check") {
                msgbox("CFX Artifacts", &e, false);
            }
            return 2;
        }
    };
    let settings = load_settings(&base);
    let keep = arg_value(&args, "--keep")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>())
        .unwrap_or_else(|| settings.keep.clone());

    if args.iter().any(|a| a == "--update") {
        let (version, url) = match pick_version(&args).await {
            Ok(x) => x,
            Err(e) => { log_event(&base, "-", "-", false, &e, "auto"); return 1; }
        };
        return match update_core(&Rep(None), None, keep, url, version, "auto").await {
            Ok(_) => 0,
            Err(_) => 1,
        };
    }

    // --check: avisa al usuario y deja que decida
    let installed = read_version(&art);
    let (version, url) = match pick_version(&args).await {
        Ok(x) => x,
        Err(_) => return 1,
    };
    let broken = match &installed {
        Some(v) => fetch_json(&format!("/check?artifact={v}")).await.ok().map(|j| j["status"] == "BROKEN").unwrap_or(false),
        None => false,
    };
    if installed.as_deref() == Some(version.as_str()) && !broken {
        return 0;
    }
    let m = mb(&settings.lang);
    let mut msg = m[1]
        .replace("{v}", &version)
        .replace("{i}", installed.as_deref().unwrap_or(m[9]))
        .replace("{b}", if broken { m[2] } else { "" });
    msg += "\n\n";
    if !keep.is_empty() {
        msg += &m[3].replace("{k}", &keep.join(", "));
        msg += "\n";
    }
    msg += m[4];
    msg += "\n";
    if !fx_pids(&art).is_empty() {
        msg += m[5];
        msg += "\n";
    }
    msg += "\n";
    msg += m[6];
    if !msgbox(m[0], &msg, true) {
        return 0;
    }
    match update_core(&Rep(None), None, keep, url, version.clone(), "auto").await {
        Ok(_) => { msgbox("CFX Artifacts", &m[7].replace("{v}", &version), false); 0 }
        Err(e) => { msgbox("CFX Artifacts", &format!("{}\n{e}", m[8]), false); 1 }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--check" || a == "--update") {
        let code = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map(|rt| rt.block_on(cli_main(args)))
            .unwrap_or(1);
        std::process::exit(code);
    }
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            close_splash, get_info, list_versions, check_version, delete_backup, set_base,
            list_backups, update, restore, server_status, server_start, server_stop, server_logs,
            check_app_update, open_url, open_txadmin, create_txadmin_shortcut, get_settings, save_settings, get_log, task_status, task_create, task_delete
        ])
        .run(tauri::generate_context!())
        .expect("error al iniciar la app");
}
