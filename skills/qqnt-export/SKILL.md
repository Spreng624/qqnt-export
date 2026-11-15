---
name: qqnt-export
description: Install and operate QQNT Export on Windows to decrypt, search, and export locally authorized QQNT chat records. Use when the user asks to install, update, authenticate, query, search, or export QQNT history.
---

# QQNT Export

QQNT Export is a Windows x64 command-line tool that works with data belonging
to the current Windows user. It never uploads chat data.

## Install or update

Run the installer bundled with this skill:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\install.ps1
```

The installer:

- downloads the latest release from `Spreng624/qqnt-export`;
- verifies `SHA256SUMS.txt`;
- skips the download when the installed EXE already matches;
- installs to `%LOCALAPPDATA%\qqnt-export\bin\qqnt-export.exe`.

Use the returned absolute path for later commands. Add `-AddToUserPath` only
when the user wants the command available globally.

## Authenticate once

```powershell
& "$env:LOCALAPPDATA\qqnt-export\bin\qqnt-export.exe" auth -a <QQ号>
```

`auth` first validates any DPAPI-cached key. If the key is valid, it exits
without launching QQ. If no valid key exists, it asks the user to exit QQ
manually, waits for all `QQ.exe` processes to close, launches a debug-mode QQ,
and prompts the user to log in or confirm the target account.

Never close QQ on the user's behalf. Never print the database key. After a
successful capture, the debug QQ exits automatically and the key is stored
under `%LOCALAPPDATA%\qqnt-export\keys`.

## Build the local index

```powershell
$tool = "$env:LOCALAPPDATA\qqnt-export\bin\qqnt-export.exe"
& $tool open -a <QQ号>
```

`open` reads only the cached key. Run it again after QQ data changes. Run
`auth` again only when the key is missing, invalid, or the QQ database was
recreated.

## Query and export

```powershell
& $tool chats
& $tool history group:<群号>
& $tool history c2c:<QQ号> -n 100
& $tool search "<关键词>" --chat group:<群号>
& $tool export group:<群号> -f markdown -o chat.md
& $tool export group:<群号> -f html -o chat.html
```

Supported export formats: `markdown`, `html`, `json`, `jsonl`, `csv`.

## Safety

- Process only the user's own account or data they are explicitly authorized
  to access.
- Do not upload databases, exported chats, keys, or screenshots containing
  personal chat content.
- Ask before exporting chat content to a new location.
- `clean` removes the local plaintext index and cached database key.
