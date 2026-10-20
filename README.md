<p align="center">
  <img src="assets/icons/takt-256.png" width="112" alt="Takt icon">
</p>

<h1 align="center">Takt</h1>

<p align="center">A local video and audio player built with Rust, Slint, and libmpv.</p>

<p align="center"><strong>English</strong> · <a href="README.ru.md">Русский</a></p>

Takt is currently available for 64-bit Windows 10 and 11. Linux support is
planned after version 1.0.

Current version: `1.0.0`.

## Features

- Video and audio playback with mixed folder queues.
- Audio metadata, embedded artwork, lossless formats, and gapless transitions.
- Audio and subtitle track selection, external subtitles, and chapters.
- Playback speed, equalizer, aspect-ratio controls, and optional hardware decoding.
- Fullscreen and picture-in-picture modes.
- Configurable physical-key shortcuts that work across keyboard layouts.
- Dark and light themes with selectable accent colors.
- Optional FFmpeg conversion and remuxing.
- Signed in-app updates with automatic rollback.

## Install

Download either the installer or the portable archive from the GitHub Releases
page.

The installer adds Takt for the current user and can register supported media
types in **Open with**. The portable package can be extracted anywhere writable;
keep `takt.exe` and `libmpv-2.dll` in the same directory.

Playback works without a system-wide mpv or FFmpeg installation. The converter
is optional and requires a user-provided `ffmpeg.exe` in `PATH`, beside
`takt.exe`, under `tools`, or at the path specified by `TAKT_FFMPEG`.

## Build from source

Requirements:

- 64-bit Windows 10 or 11;
- stable Rust with the `x86_64-pc-windows-msvc` target;
- Visual Studio Build Tools with the C++ toolchain;
- PowerShell 7.

Prepare the pinned native runtime and start Takt:

```powershell
./scripts/fetch-native-runtime.ps1
cargo run --locked --features windows-gui
```

Open a file or directory from the command line:

```powershell
cargo run --locked --features windows-gui -- "C:\Media\video.mp4"
target\debug\takt.exe "C:\Music\Album"
```

Build distributable packages:

```powershell
./scripts/build-release.ps1
./scripts/build-installer.ps1
```

Artifacts are written to `artifacts/release`.

## Checks

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo test --locked -p libmpv --lib ownership_tests
```

## Documentation

- [Architecture](ARCHITECTURE.en.md) · [Русский](ARCHITECTURE.md)
- [Benchmarking](BENCHMARKING.en.md) · [Русский](BENCHMARKING.md)
- [Support and known limitations](SUPPORT.en.md) · [Русский](SUPPORT.md)
- [Security policy](SECURITY.md)
- [Codec matrix](CODEC_MATRIX.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)

## License

Takt is distributed under the [GNU General Public License v3.0 or later](LICENSE).
