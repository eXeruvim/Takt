# Oxide Architecture

**English** · [Русский](ARCHITECTURE.md)

## System boundaries

Oxide separates the UI thread from the player thread. All libmpv control calls
are confined to the player thread, with commands and events crossing bounded
channels. The render API is the only deliberate exception: Slint calls it from
the rendering notifier while the window's OpenGL context is current.

```text
Slint UI -> ipc::Command -> player::mpv -> libmpv
Slint UI <- ipc::Event   <- player::mpv <- libmpv events
player::mpv -> one-shot MpvRenderSource -> ui::video_output

player::mpv -> bounded session snapshot -> storage writer
player::mpv -> latest metadata request -> cancellable metadata worker
player::mpv -> latest playlist request -> cancellable directory scanner

Slint UI -> converter::ConverterCommand -> converter -> FFmpeg process
Slint UI <- converter::ConverterEvent   <- converter <- FFmpeg progress
```

This split keeps decoding and filesystem work out of the UI event loop.

## Modules

- `src/ipc.rs` defines product-level commands and events shared by the UI and
  player.
- `src/player/mpv.rs` owns the libmpv lifecycle, commands, and events.
- `src/player/mpv_adapter.rs` creates and configures libmpv, registers observed
  properties, and supplies platform-specific option values.
- `src/player/playback_diagnostics.rs` owns the optional environment-controlled
  libmpv counters and diagnostic report.
- `src/player/playback_state.rs` implements the explicit state transitions used
  while waiting for a resume decision and while suppressing a rewrite of cleared
  history.
- `src/player/background_jobs.rs` is the single owner of background player work.
  It publishes results only after higher-priority commands have been processed
  and rejects stale generations and paths before changing state or notifying the
  UI.
- `src/player/latest_worker.rs` is the shared bounded worker primitive: one active
  request, one replaceable pending request, cooperative cancellation, a single
  completed result, and a mandatory thread join at shutdown.
- `src/player/playlist.rs` discovers and sorts media in a directory.
- `src/player/playlist_worker.rs` runs one cancellable latest-request worker that
  builds a bounded queue without blocking libmpv commands.
- `src/player/media_info.rs` reads media properties.
- `src/player/metadata_worker.rs` runs one bounded audio-tag and artwork worker;
  active work can be cancelled and the pending request can be replaced.
- `src/player/session_store.rs` contains the history and resume-position model
  plus one bounded writer. Clear and shutdown operations use an acknowledged
  flush.
- `src/player/shortcuts.rs` defines typed actions, physical key combinations,
  conflicts, the editing draft, and persistence of user bindings.
- `src/converter.rs` and `src/converter/` coordinate conversion. FFmpeg discovery,
  profiles and arguments, the job queue, child process, result publication, and
  bounded parsing of progress and diagnostics each have a clear owner.
- `src/render/` integrates the video surface with the application window.
- `src/ui/ui_logic.rs` connects Slint callbacks to IPC.
- `src/ui/conversion_controller.rs` owns converter presentation state: input and
  output selection, presets, job submission, history, and FFmpeg events.
- `src/ui/settings_controller.rs` applies and persists theme, accent, volume and
  mute, speed, playlist order, and hardware-decoding settings.
- `src/ui/video_output.rs` owns the Windows lifecycle of integrated OpenGL output,
  the render notifier, the explicit error state, and the video-ready handshake
  with the player thread.
- `src/ui/player_event_controller.rs` applies bounded batches of player events to
  the Slint model, including media transitions, timeline updates, and metadata.
- `src/ui/player_command_controller.rs` maps file, transport, audio, and playlist
  actions from Slint to typed player commands.
- `src/ui/shortcut_controller.rs` owns the Command Palette and shortcut editor:
  explicit editing and saved-feedback states, physical-key capture, dispatch,
  search results, persistence, and a consistent reset path.
- `src/ui/window_event_controller.rs` routes native window events: focus and
  system theme, move and resize, drag and drop, Inspector gestures, modal keys,
  and physical shortcuts.
- `src/ui/drawer_controller.rs` handles Inspector side changes, in-place tab
  switches within a shared drawer, and synchronized audio-layout motion. A side
  change uses a shorter motion profile without speeding up a normal close.
- `src/ui/command_catalog.rs` contains immutable command descriptions and search
  over localized names, descriptions, and aliases without a Slint dependency.
- `src/ui/presentation.rs` formats time, sizes, and media properties without a
  Slint window dependency.
- `src/ui/window_placement.rs` restores normal bounds on an available monitor and
  combines native move/resize events before saving, so stale geometry is never
  written.
- `src/ui/window_interaction.rs` provides HWND access and the manual drag
  lifecycle with Win32 mouse capture, including restore-on-drag from maximized.
- `src/ui/window_modes.rs` is the single owner of fullscreen and picture-in-
  picture state, including prior geometry.
- `src/ui/windows_theme.rs` isolates Windows system-theme detection.
- `ui/` contains the declarative Slint components.

## Development rules

1. Failure of one command must not terminate the player thread.
2. The UI receives state through events; it does not poll libmpv.
3. Playlist and metadata code must be testable without opening a window.
4. UI colors, dimensions, and states come from semantic tokens.
5. Win32 code stays under `cfg(windows)` in `render` or `ui`; it does not leak
   into the player model.
6. FFmpeg runs as a child process and must never block the UI or mpv.

## Video output selected for Windows 1.0

The primary backend uses the libmpv render API. It draws into FemtoVG's current
OpenGL framebuffer before the Slint scene. This shared composition lets the title
chrome, control deck, Inspector, and modal surfaces sit directly above video
without resizing the video target when controls appear or auto-hide.

`libmpv-rs` leaves its `render` module unimplemented, so the small required
`libmpv-sys` wrapper is isolated in `src/render/mpv_opengl.rs`. It creates and
destroys the render context on the UI/render thread, resolves OpenGL symbols
through FemtoVG, and turns mpv's update callback into a Slint redraw request. The
UI confirms that the render context is ready before playback begins. After
context creation, every frame, and teardown, Oxide restores framebuffer,
viewport, scissor, blending, clear color, and dithering state—the state libmpv
explicitly excludes from its reset guarantee.

## FFI and ownership contracts

- `MpvOpenGlRenderer` deliberately implements neither `Send` nor `Sync`.
  Creation, rendering, `report_swap`, and destruction are valid only in Slint's
  rendering notifier while the original OpenGL context is current. Its `unsafe`
  API makes that external invariant explicit.
- The render source holds `Arc<Mpv>` longer than the render context. The update
  callback receives the stable address of a regular `Box<UpdateTarget>`. `Drop`
  first removes the callback, then destroys the context, and only then frees the
  box.
- No Rust panic may cross the C ABI from the GL symbol loader or update callback.
  The callback only coalesces redraw requests and never calls mpv.
- Fullscreen uses Slint/winit's single borderless-fullscreen path. That path owns
  the Windows transition, taskbar visibility, and restoration of the previous
  maximized or windowed placement. The UI updates its fullscreen state only after
  passing the request to the window backend; there is no parallel manual chain of
  placement or topmost calls. On Windows, decorations are disabled before winit's
  native transition and the Slint fullscreen property is synchronized afterward.

  `native_frame` installs a static HWND subclass from `RenderingSetup` before the
  window becomes visible. Slint 1.17.1/winit draws the first frame of a hidden
  window and only then calls `set_visible(true)`; an event-loop callback is too
  late and leaves a visible system title bar between those operations. Calling
  `app.show()` before the loop only queues creation, so the HWND is not available
  immediately afterward. Installation failure terminates the loop and is
  reported after normal cleanup.

  The subclass supplies the client-area calculation for `WM_NCCALCSIZE`, ignores
  `WM_NCPAINT` and the two legacy UAH drawing messages, and forwards
  `WM_NCACTIVATE` to winit with `lParam = -1`. Focus accounting remains intact
  without repainting the standard title bar. Maximized bounds use the work area
  of the target monitor; fullscreen uses the entire monitor. On exit from
  fullscreen, winit restores its saved `WINDOWPLACEMENT`. For a borderless
  window, the proposed maximized geometry is normalized in
  `WM_WINDOWPOSCHANGING` before Windows applies it. This is the intended Win32
  extension point and needs neither a timer nor a corrective `SetWindowPos`.

  Subclass installation removes `WS_CAPTION` (including `BORDER` and
  `DLGFRAME`) and decorative extended-edge styles.
  `WM_STYLECHANGING` filters the same bits from later winit writes before Windows
  applies them. The backend retains control of all other styles, including
  `WS_THICKFRAME`, visibility, minimize/maximize, taskbar presence, and
  transparency. Title and icon use the normal API. Hidden-HWND tests cover
  repeated style writes and activation messages; visual effects and Snap still
  require manual acceptance.

  `WS_EX_WINDOWEDGE` is intentionally not filtered because user32 keeps it
  automatically with `WS_THICKFRAME`. Independent
  `CLIENTEDGE`, `STATICEDGE`, and `DLGMODALFRAME` bits are removed, while winit
  remains responsible for window size. During windowed live resize,
  `WM_NCCALCSIZE` returns `WVR_VALIDRECTS`: the exact intersection of the old and
  new client areas is copied at the same screen coordinates. Moving the top or
  left edge therefore does not shift the visible frame while Slint prepares a
  surface of the new size. Region preservation is disabled in maximized and
  fullscreen modes; periodic redraws during the modal resize loop remain
  independent.

  The registration stores only the target fullscreen flag, never a Rust pointer,
  and is removed on `WM_NCDESTROY`. It may only be installed by the window's
  owning thread. On Windows/MSVC, `build.rs` embeds an activation manifest for
  `Microsoft.Windows.Common-Controls` v6. `GetWindowSubclass` is not a named
  export of the legacy `System32/comctl32.dll`; without the manifest the process
  exits before `main` with `STATUS_ENTRYPOINT_NOT_FOUND`. A packaged application
  must therefore be smoke-tested with `target/debug/oxide.exe --build-info`—a
  successful link is not enough. Caption filtering is centralized in
  `native_frame`; placement stays with winit. `WS_THICKFRAME` remains available
  for resize and Snap, and the guard does not alter position, size, taskbar state,
  or z-order. Oxide chrome is the only visible title bar.

The remaining external assumptions are tested against the release native runtime.
Slint must keep the original GL context current through `RenderingTeardown`, and
the selected libmpv build and driver must honor the documented render API. Oxide
does not preserve the OpenGL debug callback, another state excluded by mpv's
guarantee: FemtoVG does not install one, and a general implementation would
require OpenGL 4.3. These assumptions are covered by a smoke test whenever the
renderer or backend changes.

Failure of the target OpenGL render API disables video and shows an explicit
error state; the alternative child-HWND backend is not enabled.
