<div align="center">

# TikTok Lite

**A small, native Windows shell for TikTok Web.**

Built with Tauri 2 and Microsoft Edge WebView2. No bundled Chromium, no custom account handling, and no background updater.

[![Windows build](https://github.com/officialputuid/TikTokLite/actions/workflows/windows-build.yml/badge.svg)](https://github.com/officialputuid/TikTokLite/actions/workflows/windows-build.yml)
[![Latest release](https://img.shields.io/github/v/release/officialputuid/TikTokLite?display_name=tag&sort=semver)](https://github.com/officialputuid/TikTokLite/releases/latest)
[![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-0078D4?logo=windows11&logoColor=white)](#requirements)
[![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)](https://v2.tauri.app/)

[Download latest release](https://github.com/officialputuid/TikTokLite/releases/latest) · [Report a problem](https://github.com/officialputuid/TikTokLite/issues)

</div>

> [!NOTE]
> TikTok Lite is an independent, unofficial wrapper for [TikTok Web](https://www.tiktok.com/). It is not affiliated with or endorsed by TikTok or ByteDance.

## Why TikTok Lite?

TikTok Lite uses the WebView2 runtime already available on most current Windows systems instead of shipping another browser engine. The result is a compact executable with native window, tray, and startup controls around the standard TikTok website.

## Features

- **Portable and installed modes** — run the standalone EXE or use the per-user NSIS installer.
- **Persistent login** — keep the same TikTok Web session between launches.
- **Native system tray** — open, reload, pause media, change settings, clear site data, or quit.
- **Lower idle activity** — pause video and audio while the window is hidden or minimized.
- **Startup controls** — optionally start with Windows and launch minimized.
- **Single instance** — another launch focuses the existing window.
- **Safer navigation** — TikTok HTTPS pages stay in-app; external HTTPS links open in the default browser; unsafe schemes are blocked.
- **Managed site permissions** — control notifications, camera, and microphone; location remains blocked.
- **Built-in update check** — view the installed version and check GitHub Releases from **Tray → About**.
- **No privileged installer** — default installation targets the current Windows user.

## Download

Get both builds from [GitHub Releases](https://github.com/officialputuid/TikTokLite/releases/latest):

| File | Use case |
| --- | --- |
| `tiktok-lite.exe` | Portable. Run directly without installing. |
| `TikTok Lite_<version>_x64-setup.exe` | Standard installation with Windows integration and uninstall support. |

Windows may show a SmartScreen warning because release binaries are not code-signed. Verify that the file came from this repository before running it.

## Requirements

- Windows 10 or Windows 11, x64
- [Microsoft Edge WebView2 Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)

WebView2 is normally present on Windows 11 and updated Windows 10 systems. The installer can download its bootstrapper when the runtime is missing. Portable mode expects WebView2 to be available already.

## Tray controls

| Command | Behavior |
| --- | --- |
| **Open TikTok Lite** | Restores and focuses the main window. |
| **Pause Media** | Pauses active video and audio without forcing playback when resumed. |
| **Settings** | Controls close-to-tray, autostart, minimized launch, pause-while-hidden behavior, and site permissions. |
| **Reload** | Reloads TikTok Web. |
| **Clear Site Data** | Clears the WebView2 profile after confirmation and signs out. |
| **About** | Shows app version, developer, update check, and repository link. |
| **Quit** | Ends the app and releases its WebView2 processes. |

Default settings:

- Hide to tray when closed: **enabled**
- Pause media while hidden or minimized: **enabled**
- Start with Windows: **disabled**
- Start minimized: **disabled**
- Notifications: **blocked**
- Camera: **blocked**
- Microphone: **blocked**
- Location: **always blocked**

Enable managed permissions under **Tray → Settings → Permissions**. Changes persist in the WebView2 profile and reload TikTok after the update completes. Camera and microphone also require access for desktop apps in Windows privacy settings.

## Privacy and security

TikTok Lite displays the official TikTok website. Login credentials, cookies, messages, account data, and video content are handled by TikTok inside WebView2; the app does not add code that reads them.

Remote content receives no Tauri capabilities, IPC commands, shell access, or native filesystem access. Only HTTPS `tiktok.com` hosts and subdomains can navigate in the main window. Required OAuth providers may open in a separate webview, while unrelated HTTPS destinations open in the default browser. Release builds disable DevTools.

Settings are stored as JSON in the Windows app-config directory. **Clear Site Data** removes the WebView2 browsing profile only after explicit confirmation.

## Resource behavior

Hiding or minimizing the window pauses media to reduce video decoding, audio, and GPU activity. It does not unload TikTok or discard WebView2 renderer state, so RAM use remains driven by TikTok Web and WebView2. Use **Quit** when the app must release all memory.

## Build from source

### Prerequisites

- [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with **Desktop development with C++** and a Windows SDK
- [Rust stable](https://rustup.rs/) 1.88 or newer with the `x86_64-pc-windows-msvc` target
- Tauri CLI 2

Run in Developer PowerShell for Visual Studio:

```powershell
rustup toolchain install stable --profile minimal --target x86_64-pc-windows-msvc
rustup default stable
rustup component add rustfmt clippy
cargo install tauri-cli --version '^2' --locked

cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo tauri build --bundles nsis
```

Outputs:

- Portable executable: `target/release/tiktok-lite.exe`
- NSIS installer: `target/release/bundle/nsis/*-setup.exe`

## Troubleshooting

### The portable EXE does not open

Install the [WebView2 Evergreen Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) and try again.

### Windows SmartScreen blocks the file

Choose **More info → Run anyway** only after confirming the download came from this repository.

### TikTok stays active after closing the window

Closing hides the app to tray by default. Use **Tray → Quit** to stop it completely, or change the close behavior under **Settings**.

### Login or site state is broken

Use **Tray → Clear Site Data**, then sign in again. This intentionally removes the saved TikTok Web session.

---

<div align="center">

Maintained by [officialputuid](https://github.com/officialputuid)

</div>
