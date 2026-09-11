use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use slint::{ComponentHandle, Image, Rgba8Pixel, SharedPixelBuffer};

use super::ui_logic::AppWindow;

const CLOSE_DELAY: Duration = Duration::from_secs(3);

/// Own the countdown on the event loop, independent of window activation and animation ticks.
pub(super) fn install(app: &AppWindow) {
    let weak = app.as_weak();
    app.on_prepare_shortcut_close(move |x, y, width, height| {
        let Some(app) = weak.upgrade() else { return };
        if app.get_shortcut_close_pending() {
            return;
        }

        // Capture before hiding the browser. Only the modal crop is retained, never written to disk.
        match blurred_modal_snapshot(app.window(), (x, y, width, height)) {
            Ok(image) => {
                app.set_shortcut_close_background(image);
                app.set_shortcut_close_background_ready(true);
            }
            Err(error) => {
                eprintln!("Shortcut dialog blur unavailable: {error:#}");
                app.set_shortcut_close_background(Image::default());
                app.set_shortcut_close_background_ready(false);
            }
        }
        app.set_shortcut_close_remaining(CLOSE_DELAY.as_secs_f32());
        app.set_shortcut_close_pending(true);
    });

    let timer = Rc::new(slint::Timer::default());
    let weak = app.as_weak();
    app.on_shortcut_close_state_changed(move |pending| {
        timer.stop();
        let Some(app) = weak.upgrade() else { return };
        if !pending {
            app.set_shortcut_close_background(Image::default());
            app.set_shortcut_close_background_ready(false);
            return;
        }

        let started = Instant::now();
        let weak = app.as_weak();
        timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let Some(app) = weak.upgrade() else { return };
                if !app.get_shortcut_close_pending() {
                    return;
                }
                let remaining = CLOSE_DELAY.saturating_sub(started.elapsed());
                app.set_shortcut_close_remaining(remaining.as_secs_f32());
                if remaining.is_zero() {
                    app.invoke_finish_shortcut_close();
                }
            },
        );
    });
}

fn blurred_modal_snapshot(window: &slint::Window, bounds: (f32, f32, f32, f32)) -> Result<Image> {
    let snapshot = window
        .take_snapshot()
        .context("reading the Slint framebuffer")?;
    let (x, y, width, height) = bounds;
    ensure!(
        [x, y, width, height].iter().all(|value| value.is_finite()),
        "invalid modal bounds"
    );
    let scale = window.scale_factor();
    let left = (x * scale).round().max(0.0) as u32;
    let top = (y * scale).round().max(0.0) as u32;
    let width =
        ((width * scale).round().max(0.0) as u32).min(snapshot.width().saturating_sub(left));
    let height =
        ((height * scale).round().max(0.0) as u32).min(snapshot.height().saturating_sub(top));
    ensure!(
        width > 0 && height > 0,
        "modal lies outside the framebuffer"
    );

    // Borrow the full buffer and downsample the crop before blurring, bounding CPU and retained RAM.
    let frame = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
        snapshot.width(),
        snapshot.height(),
        snapshot.as_bytes(),
    )
    .context("invalid RGBA framebuffer")?;
    let crop = image::imageops::crop_imm(&frame, left, top, width, height);
    let small = image::imageops::thumbnail(&*crop, 340, 280);
    let blurred = image::imageops::fast_blur(&small, 3.0);
    Ok(Image::from_rgba8(
        SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
            blurred.as_raw(),
            blurred.width(),
            blurred.height(),
        ),
    ))
}
