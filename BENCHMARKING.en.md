# Measuring Oxide

**English** · [Русский](BENCHMARKING.md)

Performance measurements use the `profiling` profile: an optimized build with
symbols for the profiler. A debug build started with `cargo run` is not suitable
for CPU, memory, or latency comparisons.

## Quick scenario measurement

1. Build an unchanged commit:

   ```powershell
   cargo build --profile profiling --locked --offline
   ```

   The executable embeds its commit, package version, and tracked-source state.
   Before recording, the script calls `oxide.exe --build-info` and refuses to
   label a stale or locally modified binary with the current commit. Rebuild the
   profiling target after every benchmark commit. `-AllowUnverifiedBuild` is for
   a draft single run only; `-AllowDirty` is its series equivalent, and neither
   result is eligible for a baseline.

2. Run one scenario. This example records two minutes of audio playback:

   ```powershell
   .\scripts\measure-oxide.ps1 -Scenario audio -DurationSeconds 120
   ```

   Video measurements must name the requested decoder path and renderer so that
   incompatible output paths cannot be grouped by accident:

   ```powershell
   .\scripts\measure-oxide.ps1 `
     -Scenario video-1080p `
     -DecoderMode hardware-auto-copy `
     -RendererBackend integrated-opengl `
     -WindowMode maximized `
     -Theme dark `
     -MediaPath C:\media\sample-1080p.mp4
   ```

   The main decoder modes are `software` and `hardware-auto-copy`; renderer
   values are `integrated-opengl` and `native-hwnd-fallback`. Both fields are
   required for video. The historical `hardware-auto-copy` name denotes Oxide's
   automatic product chain: direct NVDEC on compatible NVIDIA hardware, then
   NVDEC copy-back, then a safe cross-vendor copy-back fallback. After playback
   starts, the actual values appear in Inspector under **Playback settings**.
   `OpenGL` means `integrated-opengl`; `HWND` means `native-hwnd-fallback`.
   `nvdec` is a strict NVIDIA-only CUDA/OpenGL interop mode for diagnostic A/B
   tests and does not allow fallback. The exact `nvdec-copy` and `d3d11va-copy`
   labels remain accepted for older results.

   `MediaPath` is never stored. Results contain only the extension, file size,
   and SHA-256 digest. Add local context with `-Notes` when needed.

   For the standard profiling target, the script can copy the repository-root
   `mpv-2.dll` beside the executable as `libmpv-2.dll`, the name expected by the
   loader. A custom executable must provide its own DLL.

3. Oxide opens and waits. Configure the requested scenario, let it settle, then
   press Enter in the terminal. Do not interact with unrelated applications
   during the sample; the terminal shows the remaining time.

Available scenarios are `idle`, `audio`, `video-1080p`, `video-4k`,
`large-playlist`, `large-artwork`, `artwork-release`, and `ui-panels`. Record the
hardware-decoding and theme choice in the result and do not change either during
a run.

The `ui-panels` scenario automatically repeats **Queue → Settings → close** after
measurement begins. Before pressing Enter, open the requested media and leave all
drawers closed. A 100-cycle run looks like this:

```powershell
.\scripts\measure-oxide.ps1 -Scenario ui-panels -DurationSeconds 136 -UiCycleCount 100 -MediaPath "C:\Media\sample.mp4" -CloseAfterMeasurement
```

The script validates completed cycles from Oxide's internal report instead of
trusting elapsed time. Slint timers are serviced on frame boundaries, so the
duration budget includes frame alignment and normal scheduler jitter in addition
to the nominal 380 ms step. A 1,000-cycle run needs at least 1,270 seconds. This
scenario detects sustained growth in private bytes, handles, and threads; it does
not replace separate media-switch or converter loops.

Raw samples are written to `benchmark-results/*.csv` and the summary to an
adjacent JSON file. The directory is ignored because it contains details of the
machine. JSON records the commit and tree state, Rust and Cargo versions, hashes
of `Cargo.lock`, the executable, and `mpv-2.dll`, plus OS, CPU, GPU, RAM, storage,
power plan, and AC/battery state. Once the scenario is configured, the script
also records DPI and the physical and logical client-area dimensions. Decoder,
renderer, theme, and window mode are stored separately.

CPU is normalized against all logical processors, matching Task Manager closely.
Windows `GPU Process Memory` counters are sampled in the same time series. CSV
and JSON contain p50, p95, p99, and peak dedicated and shared memory, plus local,
non-local, and total committed memory. Dedicated/shared and local/non-local are
two classifications of the same allocations and must not be added together. If
the driver does not publish per-PID counters, the report says `available: false`;
it never substitutes zero.

Validate script mechanics without opening a window:

```powershell
.\scripts\measure-oxide.ps1 -Scenario idle -ValidateOnly
```

## Pre-release smoke test

After building a clean commit, verify that the profiling executable creates its
main window, stays responsive, and handles a normal close request:

```powershell
.\scripts\smoke-oxide.ps1
```

The script operates only on the PID it created. It never calls `Stop-Process`; if
the normal close fails, it leaves the window for inspection. For the standard
profiling build it also checks the embedded commit and clean-tree marker.

Pass a short video to exercise media startup. While the window is visible,
confirm picture and sound, automatic playback, and timeline progress:

```powershell
.\scripts\smoke-oxide.ps1 -MediaPath "C:\Media\short video.mp4" -VisibleSeconds 8
```

The script checks process lifecycle, not pixels or audio output. Validate paths
without opening the UI:

```powershell
.\scripts\smoke-oxide.ps1 -ValidateOnly
```

## Repeated-run summaries

Run a five-sample series with one command:

```powershell
.\scripts\run-benchmark-series.ps1 `
  -Scenario idle `
  -Runs 5 `
  -DurationSeconds 120
```

For `idle`, startup, warmup, capture, and normal close are automatic. Audio,
video, and playlist scenarios retain one deliberate manual step per run: open
the requested file, confirm the actual decoder and renderer, then press Enter.
The series never kills Oxide. If a normal close takes more than ten seconds, it
stops and leaves the window open.

The final run triggers aggregation automatically. To aggregate existing results:

```powershell
.\scripts\summarize-benchmark-results.ps1 -Scenario idle
```

Files are not grouped merely because their names look similar. A group requires
matching commit and build/runtime hashes, duration and interval, media, decoder
and renderer, theme, window mode and size, DPI, hardware, drivers, and power
state. Incomplete runs and dirty trees are excluded by default. `-AllowDirty`
and `-IncludeDirty` can test the pipeline, but the output is a
`pilot-aggregate`, not a `baseline-candidate`. Untracked files do not affect
build status; changes to tracked files always do.

Each `aggregate-*.json` retains the source values and reports min, p50, p95, and
max across runs for CPU, working and private memory, dedicated/shared GPU memory,
handles, and threads. Older schema-2 files without GPU counters remain readable;
their GPU aggregates are absent. Script self-tests:

```powershell
.\scripts\summarize-benchmark-results.ps1 -Scenario idle -ValidateOnly
.\scripts\run-benchmark-series.ps1 -Scenario idle -ValidateOnly
```

## Media corpus

The schema and target matrix live in
[`benchmark-corpus/README.md`](benchmark-corpus/README.md). Local samples and the
manifest remain ignored until every file has a known source and redistribution
permission. Check sizes, SHA-256 digests, unique IDs, and safe relative paths:

```powershell
.\scripts\verify-benchmark-corpus.ps1
```

Use `-RequireReleaseCoverage` before the final baseline. It also requires every
mandatory resolution, frame rate, codec, and special corpus feature.

### Large-queue fixture

Generate independent sets containing 100, 1,000, and 10,000 names for a short
silent WAV. The last set also tests the product limit of 5,000 queue entries.
Because NTFS limits the number of hard links to one file, the generator creates
several physical WAV shards and assigns no more than 800 names to each. The
default 10,000-name fixture uses about 33 MiB instead of tens of gigabytes.

```powershell
.\scripts\new-large-playlist-fixture.ps1 -FileCount 100
.\scripts\new-large-playlist-fixture.ps1 -FileCount 1000
.\scripts\new-large-playlist-fixture.ps1 -FileCount 10000

.\scripts\run-benchmark-series.ps1 `
  -Scenario large-playlist `
  -DurationSeconds 60 `
  -WorkloadManifest .\.tmp\benchmarks\large-playlist-<timestamp>\fixture.json
```

For each run, open the directory containing that `fixture.json`, open the queue,
and only then press Enter. With 10,000 names, confirm the notice that 5,000 files
were omitted. The runner compares the WAV count with the manifest. The manifest
SHA-256 is part of the grouping signature, so fixtures of different sizes cannot
be combined. Fixtures use unique directories under `.tmp/benchmarks` and are not
deleted automatically; remove them in Explorer after testing.

### Queue-loading phases

Total process CPU does not identify the expensive part of a large queue. A small
profiling tool therefore invokes the production module's functions and measures
directory traversal and limits, natural sort, bounded UTF-8 labels, conversion
to `SharedString`, `VecModel` creation, and total time. Model construction is
measured without a window. Assignment, layout, and painting of an open Slint
drawer must be measured in the application. IPC transfers ownership of the
finished `Vec<String>` and is profiled separately only after a queueing delay is
demonstrated.

Build the tool from a clean commit and pass the fixture manifest:

```powershell
cargo build `
  --profile profiling `
  --features benchmark-tools `
  --bin oxide-playlist-bench `
  --locked `
  --offline

.\scripts\measure-playlist-phases.ps1 `
  -Manifest .\.tmp\benchmarks\large-playlist-<timestamp>\fixture.json `
  -Iterations 25 `
  -WarmupIterations 3
```

`benchmark-results/playlist-phases-*.json` does not contain the local path. It
records build provenance, the manifest digest, kept and rejected item counts,
all raw samples, and min/p50/p95/max. A stale or dirty binary requires explicit
`-AllowDirty` and is valid only for checking the mechanism.

When reading `directory_scan`, include the cost of classifying every directory
entry. Regular files should use `DirEntry::file_type` already returned by the
scan. Calling `Path::is_file` again for every name causes another metadata query
on Windows. Link traversal and the error fallback are rare separate paths and
must not be deleted merely to improve a benchmark number.

## Comparison rules

- Compare the same file, settings, power state, window size, and renderer.
- Do not use `not-recorded` in a final video baseline. Decoder and renderer are
  mandatory there. For idle, audio, and playlist runs,
  `decoder_mode: not-recorded` means “not applicable” and is valid when the other
  fields are fixed.
- Record at least five warmed runs per variant and retain every outlier.
- Process cold start and a cold filesystem cache are different experiments.
- The working set moves with the system cache. A leak is better indicated by
  sustained growth in private bytes, handles, or threads after equal completed
  cycles.
- The final 1.0 baseline also needs frame time and GPU engine/time from an
  external profiler. Built-in sampling covers process GPU-memory volume, not the
  purpose of individual textures, and does not replace RenderDoc or WPA.

## 1.0 pre-release budgets

These local gates apply to the reference machine: Ryzen 5 5600X, RTX 4070 SUPER,
Windows 11 build 26200, 2560×1440 at 165 Hz, high-performance power plan. They
are regression gates for comparable runs, not a promise of identical absolute
numbers on every PC. A new baseline needs at least five runs. A failed gate calls
for a profile or a justified budget review, not a larger number without evidence.

| Scenario | CPU | Working Set p95 | Private Bytes p95 | GPU dedicated/shared p95 | Additional gate |
| --- | --- | ---: | ---: | ---: | --- |
| Idle | p95 ≤ 0.5%, p99 ≤ 1.0% | ≤ 100 MiB | ≤ 150 MiB | ≤ 50/20 MiB | no continuous repaint; second-half growth ≤ 1 MiB/min |
| Audio, visible window | p95 ≤ 1.5%, p99 ≤ 2.1% | ≤ 175 MiB | ≤ 235 MiB | ≤ 80/30 MiB | second-half growth ≤ 1 MiB/min; handles ≤ 1,200; threads ≤ 45 |
| 1080p/29.97, software, integrated OpenGL | p95 ≤ 2.1%, p99 ≤ 3.2% | ≤ 260 MiB | ≤ 500 MiB | ≤ 250/110 MiB | zero VO/decoder drops and delayed frames; interval p95/p99 ≤ 37/43 ms; mpv render p99 ≤ 37 ms |

The audio gate deliberately includes the cold transition from the first alpha.38
run: 161.36 MiB Working Set, 221.29 MiB Private Bytes, and 1,147 handles, followed
by release of 5.12/25.48 MiB and 325 handles. The other four processes measured
110.06–110.54 MiB Working Set p95, 168.40–171.02 MiB Private Bytes p95, and
443–445 handles. The outlier remains in the raw data. A dedicated cold-start and
file-picker scenario is required before assigning it to a particular Windows or
Oxide component.

Pause requires zero continuous frames and CPU indistinguishable from idle.
Minimized mode requires zero Slint frames and at most 0.25% total CPU capacity on
this machine. Hardware decoding is not part of the mandatory release baseline
yet. An older `d3d11va-copy` run increased CPU, RAM, and VRAM and showed visual
artifacts. A new `software` versus `hardware-auto-copy` A/B is required after the
OpenGL-state isolation fix and cross-vendor chain. Until then, hardware decoding
remains off by default. 4K/60, other codecs, energy, and end-to-end presentation
have separate manifests and must not inherit the 1080p gate silently.

## System traces for CPU, GPU, and wakeups

A 250 ms sample cannot reveal short functions, context switches, or a thread's
wakeup source. Use Windows Performance Recorder for that. Build the profiling
target from a clean commit, then record one ETL trace per question:

```powershell
.\scripts\capture-oxide-wpr.ps1 -Scenario idle -Profile CPU -DurationSeconds 30
.\scripts\capture-oxide-wpr.ps1 -Scenario video-1080p -Profile GPU -DurationSeconds 30 -MediaPath C:\media\sample.mp4 -DecoderMode hardware-auto-copy -RendererBackend integrated-opengl -WindowMode windowed -Theme dark
```

For media scenarios the script opens Oxide and waits for confirmation after you
configure playback. `idle` is automatic. Available WPR profiles are `CPU`, `GPU`,
`Power`, `GeneralProfile`, `Heap`, `Video`, `ResidentSet`, `ReferenceSet`, and
`VirtualAllocation`. Do not combine heavy profiles or compare runs with different
modes.

Video scenarios require decoder and renderer fields, preventing accidental
comparison of software and hardware decode or of integrated OpenGL and the
diagnostic HWND fallback. `-WindowMode`, `-Theme`, and optional `-Notes` are also
written to the JSON manifest. Record the state actually selected in the UI.

The script rejects dirty or stale builds and does not interfere with an existing
WPR session. On failure it cancels only the session it started and requests a
normal close for its Oxide window; it does not kill the process. ETL and manifest
files are stored under `benchmark-results/wpr`.

ETL is a system trace. It includes process names and may include local paths. The
directory is ignored; review every trace before sharing it. Stack, CPU Usage,
Context Switches, and GPU-engine analysis requires Windows Performance Analyzer
from the Windows Performance Toolkit. WPR alone can record data but cannot
honestly identify hot functions. `measure-oxide.ps1` collects dedicated/shared
GPU memory directly. The standard WPA Exporter may not expose the VidMm table
even when it exists in a GPU trace, so a missing CSV is not zero VRAM usage.

System profiles normally require an elevated PowerShell session or the Windows
“Profile system performance” right. On `0xc5585011`, the script creates no trace,
closes the Oxide instance it started, and leaves no WPR session running.

Check paths and WPR profile availability without starting Oxide or recording:

```powershell
.\scripts\capture-oxide-wpr.ps1 -Scenario idle -Profile CPU -ValidateOnly
```

After a CPU trace, export Oxide module, function, sampled-thread, and exact
context-switch slices without configuring WPA tables by hand:

```powershell
.\scripts\export-oxide-wpa.ps1
```

The exporter selects the newest manifest by default, reads the exact PID, and
writes derived CSV files to the ignored `analysis` directory beside the trace.
It also creates `oxide-cpu-summary.json` with module, function, thread, and wakeup
aggregates for the busiest thread. `-Symbols` resolves Oxide names only through
the PDB under `target/profiling`; the script never enables public symbol servers.
Without it, only existing symbol caches are used. The source ETL is unchanged.

### GPU engines and paired decoder comparison

GPU traces use a separate exporter. It reads the exact PID and modes from the
manifest, creates Oxide-only and system-wide tables, and groups tasks by engine:

```powershell
.\scripts\export-oxide-gpu.ps1
```

`one_engine_timeline_percent` is the sum of GPU time divided by trace duration
for one engine. It is not whole-GPU utilization; parallel engines cannot be
added. A missing `Video Decode` row in one trace does not prove software fallback,
because drivers may attribute work differently.

To validate hardware decode, record `software` and `hardware-auto-copy` using the
same commit, binary, and media; export both and compare them:

```powershell
.\scripts\compare-oxide-gpu.ps1 -SoftwareSummaryPath C:\path\software\oxide-gpu-summary.json -HardwareSummaryPath C:\path\hardware\oxide-gpu-summary.json
```

The tool rejects mismatched executable, mpv, or media hashes, commit, renderer,
window mode, theme, or scenario. GPU comparison does not replace separate CPU,
frame-pacing, energy, and dedicated/shared-memory measurements.

Compare lightweight CPU and memory results separately:

```powershell
.\scripts\compare-oxide-decoder.ps1 -SoftwareResultPath .\benchmark-results\software.json -HardwareResultPath .\benchmark-results\hardware.json
```

The analyzer checks commit, hashes, media, duration, window, theme, hardware,
and power. `oxide-decoder-comparison.json` contains absolute and relative deltas
for CPU, Working Set, Private Bytes, dedicated/shared GPU memory, memory trends,
handles, and threads. GPU fields are marked unavailable for old pairs that lack
counters. If both inputs contain `playback_diagnostics`, the output also includes
the actual decoder, VO and decoder drops, delayed/mistimed frames, rates, and A/V
sync. One pair is useful for direction; a release gate requires at least five
clean runs plus frame-pacing, GPU, and energy evidence.

Both the recorder and analyzer validate the decoder reported by mpv. A run
requested as hardware but reporting `no` is retained for diagnosis, fails the
command, and is excluded from A/B. A software run that actually used hardware is
rejected in the same way.

With a media scenario and `-CloseAfterMeasurement`, the lightweight recorder asks
the embedded mpv instance for final counters and adds `playback_diagnostics`:
frames, VO and decoder drops, delayed/mistimed counts, rates, vsync, and A/V sync.
Benchmark mode also measures intervals between integrated OpenGL renderer calls
and the duration of `mpv_render_context_render` at p50/p95/p99.

`render_timing.scene_by_surface` separates `empty`, `video`, and `audio`: frame
interval, Slint `BeforeRendering` to `AfterRendering`, and the remainder after
subtracting nested `mpv_render`. The remainder includes Slint drawing and notifier
work but is not pure layout time. This measures pacing inside Oxide/Slint, not DWM,
GPU, or display presentation latency; PresentMon or WPA is still required for
end-to-end results. The temporary diagnostic file is deleted after merging and
is never created by a normal Oxide launch. Counters cover the current file from
load, including manual setup and warmup, so comparisons require identical steps.
`vo-delayed-frame-count` is an mpv heuristic rather than proof of a dropped frame;
it cannot produce frame-time percentiles by itself.

### Physically resident memory

The quick recorder's Working Set shows a trend, not page composition. To capture
resident memory at the end of a scenario, run an elevated PowerShell session:

```powershell
.\scripts\capture-oxide-wpr.ps1 -Scenario audio -Profile ResidentSet -DurationSeconds 30 -WarmupSeconds 5
.\scripts\export-oxide-memory.ps1
```

The exporter reads the exact PID and creates four independent views: process
total, memory lists, page categories, and file/image descriptions.
`oxide-resident-memory-summary.json` never adds WPA hierarchy levels together.
Resident Set is a snapshot of pages in physical memory when recording stops. It
is not Private Bytes or commit and does not prove a leak. If warmed Private Bytes
continue to grow, use `VirtualAllocation` as a separate experiment; collect a
heap trace only for confirmed growth in a chosen process.

For `VirtualAllocation`, `ReferenceSet`, and `Heap`, the collector starts WPR
before Oxide so startup allocations are not lost. Their ETL therefore includes
manual setup and warmup; the manifest records both trace start and the start of
the fixed measurement window. CPU/GPU and Resident Set still begin after setup:
the former need a clean steady-state interval, and the latter is an end snapshot.

The manifest separates the stop request from potentially slow ETL serialization.
`actual_trace_duration_seconds` ends before `wpr -stop`, while
`export_flush_seconds` records finalization time. Export time must not be counted
as extra scenario duration.

### Virtual allocations and private commit

Capture a separate trace to attribute startup private memory to stacks:

```powershell
.\scripts\capture-oxide-wpr.ps1 -Scenario audio -Profile VirtualAllocation -DurationSeconds 30 -WarmupSeconds 5
.\scripts\export-oxide-virtual-allocation.ps1 -Symbols
```

The exporter creates `oxide-virtual-allocation-summary.json`, four source CSV
files, and a derived WPA profile. `Outstanding Committed` is virtual memory for
which Windows promises RAM or pagefile backing; it is not the physically resident
Working Set. `Outstanding Reserved` only occupies virtual address ranges. Tens of
gigabytes reserved must not be reported as RAM consumption.

WPA stack rows are inclusive: a child allocation is already counted in its
parent. The summary stores non-overlapping branches at depth three and separate
upper-bound hints for functions and modules. Those hints must not be added; they
locate a large allocation path for a focused A/B experiment. The full tree stays
in CSV. `-Symbols` resolves Oxide through the local PDB; missing system-DLL PDBs
do not affect byte totals.
