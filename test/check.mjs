// Comprobaciones estáticas de la interfaz (sin dependencias): node test/check.mjs
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (p) => readFileSync(join(root, p), 'utf8');
const html = read('ui/index.html');
const app = read('ui/app.js');
const i18n = read('ui/i18n.js');
const rust = read('src-tauri/src/main.rs');

let failures = 0;
const fail = (msg) => { failures++; console.error('  ✗ ' + msg); };
const section = (name) => console.log('\n' + name);

const { T, LANGS } = new Function(i18n + '\n;return { T, LANGS };')();
const en = T.en;
const placeholders = (s) => [...String(s).matchAll(/\{(\d+)\}/g)].map((m) => m[1]).sort().join(',');

section('Translations: same keys and placeholders in every language');
for (const [code] of LANGS) {
  const tr = T[code];
  if (!tr) { fail(`language "${code}" is listed in LANGS but has no translations`); continue; }
  for (const k of Object.keys(en)) {
    if (!(k in tr)) fail(`${code}: missing key ${k}`);
    else if (placeholders(tr[k]) !== placeholders(en[k])) fail(`${code}: placeholders of ${k} differ from English`);
    else if (!String(tr[k]).trim()) fail(`${code}: empty value for ${k}`);
  }
  for (const k of Object.keys(tr)) if (!(k in en)) fail(`${code}: extra key ${k} (not in English)`);
}
console.log(`  ${LANGS.length} languages, ${Object.keys(en).length} keys each`);

section('HTML: ids used by app.js exist in index.html');
const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
for (const m of app.matchAll(/\$\('([A-Za-z0-9_-]+)'\)/g)) if (!ids.has(m[1])) fail(`app.js uses #${m[1]} but index.html has no such id`);

section('HTML / JS: translation keys exist');
for (const m of html.matchAll(/data-i18n(?:-ph)?="([^"]+)"/g)) if (!(m[1] in en)) fail(`index.html uses unknown key ${m[1]}`);
for (const m of app.matchAll(/\bt\('([a-z0-9_]+)'[,)]/g)) if (!(m[1] in en)) fail(`app.js uses unknown key ${m[1]}`);

section('Backend: progress stages and error codes have translations');
const stages = new Set([...rust.matchAll(/\.emit\("([a-z_]+)",\s*[\d.*\s\w]+,\s*vec!/g)].map((m) => m[1]));
for (const s of stages) if (!(`p_${s}` in en)) fail(`stage "${s}" has no p_${s} key`);
const codes = new Set([...rust.matchAll(/"(E_[A-Z_]+)[|"]/g)].map((m) => m[1]));
for (const c of codes) if (!(c.toLowerCase() in en)) fail(`error code ${c} has no ${c.toLowerCase()} key`);
console.log(`  ${stages.size} progress stages, ${codes.size} error codes`);

section('Backend: language list in save_settings matches LANGS');
const allowed = (rust.match(/\[("[a-z]{2}"(?:, "[a-z]{2}")+)\]\.contains\(&s\.lang/) || [])[1];
if (!allowed) fail('could not find the language allow-list in main.rs');
else {
  const list = [...allowed.matchAll(/"([a-z]{2})"/g)].map((m) => m[1]);
  for (const [code] of LANGS) if (!list.includes(code)) fail(`main.rs does not allow language "${code}"`);
  for (const code of list) if (!LANGS.some(([c]) => c === code)) fail(`main.rs allows "${code}" but LANGS does not list it`);
}

section('Mock backend covers every command used by the UI');
const mock = read('test/mock-tauri.js');
const mockCmds = new Set([...mock.matchAll(/^\s{4}([a-z_]+):/gm)].map((m) => m[1]));
for (const m of app.matchAll(/invoke\('([a-z_]+)'/g)) if (!mockCmds.has(m[1])) fail(`mock-tauri.js has no handler for "${m[1]}"`);

console.log(failures ? `\n${failures} problem(s) found.` : '\nAll UI checks passed.');
process.exit(failures ? 1 : 0);
