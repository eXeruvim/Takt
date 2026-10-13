# Third-party notices

Oxide is distributed under `GPL-3.0-or-later`. The complete license text is in
`LICENSE`. This document records the release policy for third-party components;
it is not legal advice and does not replace the license text of any component.

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
| vendored `libmpv-rs` wrapper 2.0.1 | LGPL-2.1 | <https://github.com/ParadoxSpiral/libmpv-rs> |
| libmpv / mpv | Depends on the exact native build; normally GPL-2.0-or-later, or LGPL-2.1-or-later only when built with the documented LGPL configuration | <https://mpv.io/> |
| FFmpeg | LGPL-2.1-or-later by default; GPL applies when GPL parts are enabled, and `nonfree` builds are not redistributable | <https://ffmpeg.org/> |

The SBOM is authoritative for the complete Rust graph. Individual crates retain
their own copyrights and license terms.

## Native runtime gate

The `mpv-2.dll` and import library currently used for local development have a
known hash but an unproven source/build configuration. They are **not approved
for public redistribution**. A release must use a native manifest marked
`release_eligible`, with exact upstream source commits, build commands and flags,
license files, checksums and corresponding-source location. The compliance
script rejects a package while any of those facts are absent.

FFmpeg is optional for playback and is currently supplied by the user for the
converter. Oxide must not download or bundle an FFmpeg build until the same
native-runtime gate has been satisfied for that exact binary.

## No warranty

Oxide and the listed third-party software are supplied without warranty to the
extent permitted by their respective licenses. Patent rules for media codecs
vary by jurisdiction; an open-source license audit does not establish patent
clearance.
