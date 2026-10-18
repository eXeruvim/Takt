# Oxide 1.0 Support

**English** · [Русский](SUPPORT.md)

This guide explains the known limitations of Oxide 1.0 and the information we
need for a reproducible bug report. Do not attach the media file, its full path,
your playback history, or a memory dump unless a maintainer asks for it and you
have reviewed the contents.

## Known limitations in Windows 1.0

- Oxide supports 64-bit Windows 10 and 11. ARM64, Linux, and macOS are planned
  for later releases and are not supported yet.
- DWM does not guarantee live window transparency in maximized or fullscreen
  mode. Oxide uses an opaque fallback with matching colors instead of capturing
  the desktop. This avoids extra load and latency and prevents accidental capture
  of private or DRM-protected content.
- Hardware decoding depends on the GPU, driver, and media format. If it produces
  artifacts, turn it off under **Video**. Software decoding is the supported
  fallback. The codecs available in the bundled libmpv/FFmpeg build are listed
  in `CODEC_MATRIX.md`.
- A slow or worn hard drive may pause briefly on first open, seeking, or a
  transition to another large file. Oxide uses bounded read-ahead; it does not
  load an entire local file into memory or hide a real I/O stall.
- The converter requires a user-provided `ffmpeg.exe`. Oxide 1.0 neither bundles
  nor downloads it, and playback does not depend on it.
- Packages are not Authenticode-signed yet, so SmartScreen may warn about an
  early, low-reputation release. Verify the SHA-256 value published with the
  release. The built-in updater separately verifies the project Ed25519
  signature and the SHA-256 digest before launching a package.
- A portable update needs write access to the directory where Oxide was
  extracted. If permissions or disk space are insufficient, the current version
  remains intact and restarts; the reason is shown on the next launch.

## Before filing a report

1. Reproduce the issue on the latest available release and record the exact steps.
2. Check whether it still occurs after restarting Oxide.
3. For video artifacts, compare hardware decoding on and off.
4. For slow media, mention the storage type (SSD, HDD, or network), but omit the
   full path.
5. For converter failures, include the last log line and `ffmpeg -version`, but
   not the source media file.

## Safe diagnostics

Run these commands in PowerShell from the directory containing `oxide.exe`:

```powershell
(& .\oxide.exe --build-info 2>&1 | Out-String).Trim()
(& .\oxide.exe --native-info 2>&1 | Out-String).Trim()
```

The first command prints the package version, commit, and source-tree state. The
second prints the versions of mpv, FFmpeg, and libass loaded from the actual DLL.
Neither command opens the UI or a media file, and neither output contains playback
history or user paths.

For a development build, also run:

```powershell
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
```

For installation or update issues, state whether you used the installer or the
portable package, identify the failing stage (check, download, apply, first
launch, or rollback), and include the exact message. The updater helper keeps its
last local error in the Windows temporary directory only until the next Oxide
launch, then deletes it after display.

## Report template

```text
Oxide: <output of --build-info>
Native runtime: <output of --native-info>
Windows: <10/11 and version>, display scaling: <%>
GPU and driver: <if relevant to video>
Package: <installer/portable>
Storage: <SSD/HDD/network share>

Steps:
1. …
2. …
3. …

Expected: …
Actual: …
Frequency: <always/sometimes, approximately N out of M>
Hardware decoding: <on/off/both>
```

A screenshot or short screen recording helps with visual issues. Before posting
it, hide personal file names, notifications, and any other private information.
