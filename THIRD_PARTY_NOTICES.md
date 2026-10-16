# Third-party notices

Oxide is distributed under `GPL-3.0-or-later`. The complete license text is in
`LICENSE`. This document records the release policy for third-party components;
it is not legal advice and does not replace the license text of any component.

## Build-only installer tooling

Windows installer artifacts are produced with Inno Setup 7.1.0. Inno Setup is
build tooling and is not bundled into or redistributed with Oxide. Its pinned
official download, digest, source and license locations are recorded in
`release/installer-tool.json`.

## Rust dependency graph

The exact Rust packages, versions, sources, checksums and declared license
expressions for a release are recorded in its CycloneDX JSON SBOM. Generate it
from the locked graph with:

```powershell
cargo install cargo-cyclonedx --version 0.5.9 --locked
cargo cyclonedx --all-features --target x86_64-pc-windows-msvc `
  --format json --spec-version 1.5 --override-filename oxide.cdx
```

The release archive must contain this notice, `LICENSE`, the generated SBOM and
the source corresponding to that release. `deny.toml` is the machine-readable
allowlist used by `cargo deny check licenses sources`.

Important direct components include:

| Component | Selected license path | Project |
| --- | --- | --- |
| Slint 1.17.1 | GPL-3.0-only | <https://slint.dev/> |
| zip 6.0.0 | MIT | <https://github.com/zip-rs/zip2> |
| vendored `libmpv-rs` wrapper 2.0.1 | LGPL-2.1 | <https://github.com/ParadoxSpiral/libmpv-rs> |
| libmpv / mpv | Selected upstream build uses the LGPL route with `-Dgpl=false`; combined release runtime is treated as LGPL-3.0-or-later | <https://mpv.io/> |
| FFmpeg inside libmpv | Upstream LGPL variant; statically linked for playback, with GPL features disabled | <https://ffmpeg.org/> |

The SBOM is authoritative for the complete Rust graph. The separate
`sbom/native-runtime.cdx.json` records the exact mpv, FFmpeg and libass versions
reported by the selected DLL and points to their source commits; its coverage
property explicitly notes the remaining static upstream build dependencies.
Individual components retain their own copyrights and license terms.

## Native runtime gate

The release runtime is the unmodified x86-64 LGPL libmpv artifact from upstream
mpv GitHub Actions run `38326895870`, source commit
`c3ac7f39514c7f45bc38a18e66887240539ee33c`. Its archive, DLL and import library
hashes are fixed in `release/native-runtime.json`; the upstream license is copied
into every compliance bundle. `scripts/fetch-native-runtime.ps1` fails closed on
any missing member, size change or hash mismatch. Local development DLLs with a
different hash are **not approved for redistribution**.

FFmpeg is optional for playback and is currently supplied by the user for the
converter. Oxide must not download or bundle an FFmpeg build until the same
native-runtime gate has been satisfied for that exact binary.

## No warranty

Oxide and the listed third-party software are supplied without warranty to the
extent permitted by their respective licenses. Patent rules for media codecs
vary by jurisdiction; an open-source license audit does not establish patent
clearance.
