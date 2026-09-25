use anyhow::Result;
use slint::winit_030::{WinitWindowAccessor, winit::window::Fullscreen};

/// Keeps the UI's fullscreen state in sync with winit's native borderless
/// fullscreen implementation.
///
/// Winit owns the Windows transition and restores the previous maximized or
/// windowed placement on exit. Keeping that transition in one layer avoids the
/// conflicting restore/resize/topmost sequence that can briefly detach the
/// OpenGL surface or expose a black frame.
#[derive(Default)]
pub struct NativeFullscreen {
    active: bool,
}

impl NativeFullscreen {
    pub fn set(&mut self, window: &slint::Window, fullscreen: bool) -> Result<()> {
        if self.active == fullscreen && window.is_fullscreen() == fullscreen {
            return Ok(());
        }

        // Keep custom non-client geometry/painting in force throughout the
        // transition and all later deferred Slint property updates.
        super::native_frame::prepare(window, fullscreen)?;

        // Run on the UI thread, before Slint's deferred property update.
        // Slint 1.17.1 enables decorations while native fullscreen is active.
        // On exit its update_window_properties() sets decorations using that
        // OLD native state, then leaves fullscreen, exposing a system caption.
        // Disable decorations while still fullscreen and let winit restore the
        // placement only afterwards. On entry the same ordering keeps the
        // window frameless before winit changes its styles and monitor bounds.
        window
            .with_winit_window(|native| {
                native.set_decorations(false);
                native.set_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
            })
            .ok_or_else(|| anyhow::anyhow!("Недоступно окно для полноэкранного режима"))?;

        // Synchronize Slint too: on its next property update the native state
        // already matches the request. In particular, exit now evaluates the
        // no-frame property with native fullscreen OFF. native_frame filters
        // caption/edge styles in WM_STYLECHANGING; winit owns placement and
        // the operational flags (including WS_THICKFRAME for resize).
        window.set_fullscreen(fullscreen);
        self.active = fullscreen;
        window.request_redraw();
        Ok(())
    }
}
