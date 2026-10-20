# Native runtime provenance

Takt 1.0 uses the x86-64 LGPL `libmpv` artifact produced by the upstream mpv
repository. This is an unmodified upstream binary; Takt only renames its GNU
import library from `libmpv.dll.a` to `mpv.lib`, which LLVM/lld accepts as a
COFF import archive.

## Pinned inputs

- mpv commit: `c3ac7f39514c7f45bc38a18e66887240539ee33c`;
- upstream GitHub Actions run: `38326895870`, successful `mingw64-full
  (win64, lgpl)` job `115067544505`;
- workflow: `.github/workflows/build.yml` at the pinned commit;
- build entry point: `ci/build-mingw64-full.sh` at the pinned commit;
- upstream archive and source hashes: `release/native-runtime.json`.

The loaded runtime identifies embedded FFmpeg as commit
`0eeb79fcc5b2761e8fdd3d0cfc5e5ba0e786645a` and libass as 0.17.5 (tag commit
`4a05d8127f525943ebf45fdc6497c9e665947f0d`). Their source archive hashes are
also part of the manifest.

The upstream job runs the following entry point in its `win64-lgpl` FFmpeg
build container:

```sh
./ci/build-mingw64-full.sh
```

That script configures FFmpeg as static and shared libraries as disabled, then
configures mpv with `-Dgpl=false`, `--default-library=shared`, static FFmpeg
linkage and `libmpv` enabled. The exact script and workflow are retained by the
pinned mpv source commit; the resulting archive and each file used by Takt are
independently SHA-256 checked.

## Recreating the Takt runtime directory

From the Takt repository:

```powershell
./scripts/fetch-native-runtime.ps1
./scripts/verify-native-runtime.ps1
```

The script downloads only the URL fixed in `release/native-runtime.json`, checks
the archive before extraction, checks every copied member, and writes:

- `libmpv-2.dll` — the runtime loaded by Takt;
- `mpv.lib` — the unchanged COFF archive bytes from `libmpv.dll.a`;
- `LICENSE.LGPL` — the license file shipped in the upstream archive;
- `native-runtime.json` — the manifest that selected the artifact.

The source archive is intentionally recorded separately from the binary archive.
Changing either URL, commit, build run or hash requires a new reviewed manifest;
the fetch script never follows a moving release name after such a change.
