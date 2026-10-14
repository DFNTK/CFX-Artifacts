# Contributing to CFX Artifacts

Thanks for your interest in improving CFX Artifacts! This guide explains how to report problems, propose changes and send code.

## Reporting bugs

Open an issue and include:
- App version (shown next to the title, e.g. `v1.0.0`) and Windows version.
- What you did, what you expected and what happened.
- The error message. Backend errors appear as codes like `E_DOWNLOAD|...`; copy the whole text.
- The relevant entry of `cfx_artifacts_data/update_log.jsonl` if an update failed.
- Whether the server was running and where the app and the artifacts folder are located.

Please do **not** paste license keys, tokens or the contents of `server-private.cfg`.

## Suggesting features

Open an issue describing the problem you want to solve before writing code. Small, focused features that fit the scope (updating artifacts, backups, server control, automation) are the most likely to be accepted.

## Development setup

Requirements: Rust (stable), Node.js and WebView2 (included in Windows 10/11).

```bash
npm install
npx tauri build --no-bundle
