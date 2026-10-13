# Oxide: local libmpv resource-ownership patch

Applied: 2026-09-12. Based on **libmpv 2.0.1**, upstream
[ParadoxSpiral/libmpv-rs](https://github.com/ParadoxSpiral/libmpv-rs/tree/3e6c389b716f52a595cc5e8e3fa1f96cb76b3de7),
commit `3e6c389b716f52a595cc5e8e3fa1f96cb76b3de7`.
This is a locally maintained patch, not a claim that upstream released a fix.
The upstream `LICENSE` and source copyright notices are retained.

The package license metadata uses the current SPDX identifier
`LGPL-2.1-or-later`, matching the existing source headers and bundled license
text. Upstream used the deprecated ambiguous identifier `LGPL-2.1`; no license
terms were changed.

## Contents and source changes

Copied `src/`, `Cargo.toml`, `build.rs`, `README.md`, and `LICENSE` from that
revision. Examples, media fixtures, upstream CI and Git metadata are not vendored.

- `Cargo.toml`: remove the upstream nested workspace and unused example-only
  `crossbeam` dev dependency. Resolve the separately vendored `libmpv-sys`
  copied from the same upstream revision. No native ABI or DLL change.
- `src/mpv.rs`: construct the owning `Mpv` immediately after `mpv_create`, before
  the initializer callback. Callback errors, panic unwinding and initialization
  errors now all use its existing destructor; no separate error-path destroy.
- `src/mpv.rs`: a private allocation guard frees string output when UTF-8
  conversion fails. `String` copies before releasing the guard; successful
  `MpvStr` takes ownership and frees on its own drop. Successful getters returning
  NULL now report `Error::Null` instead of dereferencing it.
- `src/mpv.rs`: make the existing `CString::from_raw` destruction explicit in
  `load_config`, removing a modern compiler warning without changing behavior.
- `src/mpv/events.rs`: spell out two existing elided return lifetimes (`'_`),
  removing compiler warnings without changing ownership or event handling.
- `src/mpv/ownership_tests.rs` and test-only counters at native release calls:
  regression coverage below. Counters are compiled out of normal builds.

The upstream UTF-8 conversion policy and public API are retained. An invalid
string is still an error; the patch changes resource cleanup, not decoding.
Other upstream APIs/features have not received a general safety review here.

## Verification

Run from the Oxide repository root with the native import library and DLL available:

```powershell
cargo test --locked -p libmpv --lib ownership_tests
cargo test --locked --all-targets --all-features
cargo build --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
```

The dedicated filter is intentional: upstream tests include playback and external
media and must not be run as this non-UI ownership check.

The eight ownership tests use a real headless libmpv context with config/scripts
disabled and `vo=null`, `ao=null`. They load no files or URLs. Native buffers come
from `mpv_get_property_string("mpv-version")`; invalid UTF-8 is injected into the
owned allocation, preserving its terminator and size. Assertions count completed
native frees/destroys per thread, including 32 repeated failing callbacks and 32
invalid conversions for each string type. Success, panic unwinding, delayed
`MpvStr` destruction, failed getters and NULL output are covered as well.

These checks establish cleanup of the tested allocations, not a heap/GPU profile
or a guarantee that the whole application is leak-free. Panic-abort termination
does not run Rust destructors. UI/playback regression testing remains manual.
CI invokes these tests when its existing native-runtime condition is satisfied;
unconditional native provisioning is still a separate pending task.

## Updating or removing this patch

Keep the manifest revision and this provenance record together. Before replacing
the local copy with upstream, compare all affected early exits and rerun these
regression scenarios plus the Oxide checks. Preserve both correct cleanup on
failure and single ownership on success. Do not silently update the native DLL
or the rest of the graphics dependency graph as part of this patch.
