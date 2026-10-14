# Security Policy

## Supported Versions

Only the latest release receives security fixes.

| Version | Supported          |
| ------- | ------------------ |
| 1.0.x   | :white_check_mark: |
| < 1.0   | :x:                |

## Reporting a Vulnerability

Please **do not open a public issue** for security problems.

Report it privately using one of these options:

1. **GitHub private reporting (preferred):** go to the repository's **Security** tab and click **Report a vulnerability**.
2. **Email:** [gus@dfnetwork.in]

Please include:
- The CFX Artifacts version and your Windows version.
- A description of the problem and its impact.
- Steps to reproduce it (a proof of concept helps).
- Whether you want to be credited in the fix.

### What to expect
- An acknowledgement within **7 days**.
- An update on the status at least every **14 days** until it is resolved.
- If the report is accepted, a fix will be published in a new release and the `version.md` notice will point users to it. You will be credited unless you prefer otherwise.
- If the report is declined, you will get an explanation of why.

Please give us a reasonable time to release a fix before disclosing the issue publicly.

## Scope

CFX Artifacts runs locally and does the following, so reports in these areas are especially welcome:
- Downloading and extracting archives from `runtime.fivem.net` (path traversal, integrity checks, redirects to other hosts).
- Moving, copying and deleting folders (artifacts, backups).
- Starting and stopping `FXServer.exe`, and creating Windows scheduled tasks.
- Opening URLs and creating shortcuts.
- Rendering of server console output in the interface (injection).
- The update notice read from `version.md`.

Out of scope: vulnerabilities in FXServer, txAdmin or the FiveM platform themselves (report those to Cfx.re), and problems that require an attacker to already have administrator access to the machine.
