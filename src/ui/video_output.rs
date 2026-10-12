//! Windows video-output lifecycle shared by Slint's render notifier and player events.
#![deny(clippy::undocumented_unsafe_blocks)]

use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use crossbeam_channel::Receiver;
use slint::ComponentHandle;
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{RDW_INTERNALPAINT, RedrawWindow},
};

use crate::{
    ipc::Command,
    player::commands::CommandSender,
    render::mpv_opengl::{MpvOpenGlRenderer, MpvRenderSource},
};

use super::{
    event_delivery::{self, Bridge},
    native_frame,
    ui_logic::AppWindow,
    window_interaction::native_window_handle,
};

#[derive(Clone)]
pub(super) struct VideoOutputEventHandle {
    render_source: Arc<Mutex<Option<MpvRenderSource>>>,
    renderer_requested: Arc<AtomicBool>,
}

impl VideoOutputEventHandle {
    fn set_render_source(&self, app: &AppWindow, source: MpvRenderSource) {
        if let Ok(mut target) = self.render_source.lock() {
            *target = Some(source);
        }
        app.window().request_redraw();
    }

    pub(super) fn spawn_source_bridge(
        &self,
        app: &AppWindow,
        rx: Receiver<MpvRenderSource>,
    ) -> io::Result<Bridge> {
        let weak = app.as_weak();
        let output = self.clone();
        event_delivery::spawn(
            rx,
            |_, _| true,
            move |mut sources, completion| {
                let weak = weak.clone();
                let output = output.clone();
                slint::invoke_from_event_loop(move || {
                    let _completion = completion;
                    if let Some(app) = weak.upgrade()
                        && let Some(source) = sources.pop()
                    {
                        output.set_render_source(&app, source);
                    }
                })
                .is_ok()
            },
        )
    }

    pub(super) fn request_renderer(&self, app: &AppWindow, player_tx: &CommandSender) {
        if app.get_integrated_video_renderer() {
            // The OpenGL renderer survives playlist transitions. Explicitly
            // re-enable mpv's video track instead of waiting for creation that
            // has already completed.
            let _ = player_tx.send(Command::RenderApiReady);
        } else {
            self.renderer_requested.store(true, Ordering::Release);
            app.window().request_redraw();
        }
    }
}

#[derive(Clone)]
pub(super) struct VideoOutputController {
    render_source: Arc<Mutex<Option<MpvRenderSource>>>,
    renderer_requested: Arc<AtomicBool>,
}

impl Default for VideoOutputController {
    fn default() -> Self {
        Self {
            render_source: Arc::new(Mutex::new(None)),
            renderer_requested: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl VideoOutputController {
    pub(super) fn event_handle(&self) -> VideoOutputEventHandle {
        VideoOutputEventHandle {
            render_source: Arc::clone(&self.render_source),
            renderer_requested: Arc::clone(&self.renderer_requested),
        }
    }

    pub(super) fn register_rendering(
        &self,
        app: &AppWindow,
        player_tx: CommandSender,
    ) -> Result<Receiver<anyhow::Error>> {
        let (native_init_error_tx, native_init_error_rx) = crossbeam_channel::bounded(1);

        let app_weak = app.as_weak();
        let notifier_render_source = Arc::clone(&self.render_source);
        let notifier_renderer_requested = Arc::clone(&self.renderer_requested);
        let mut gl_renderer = None::<MpvOpenGlRenderer>;
        let mut render_api_failed = false;

        app.window()
            .set_rendering_notifier(move |state, graphics_api| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                if matches!(state, slint::RenderingState::RenderingSetup) {
                    // Slint/winit pre-renders before set_visible(true). The HWND
                    // exists here but is still hidden, which is the only point
                    // where custom chrome can be installed without a flash.
                    if let Err(error) =
                        native_frame::prepare(app.window(), app.window().is_fullscreen())
                    {
                        let _ = native_init_error_tx.try_send(error);
                        let _ = slint::quit_event_loop();
                    }
                    return;
                }
                if matches!(state, slint::RenderingState::BeforeRendering) {
                    crate::benchmark_diagnostics::begin_ui_latency_render();
                    let surface = if !app.get_media_presented() {
                        crate::benchmark_diagnostics::RenderSurface::Empty
                    } else if app.get_audio_media() {
                        crate::benchmark_diagnostics::RenderSurface::Audio
                    } else {
                        crate::benchmark_diagnostics::RenderSurface::Video
                    };
                    crate::benchmark_diagnostics::begin_scene_frame(surface);
                }
                match state {
                    slint::RenderingState::BeforeRendering => {
                        if gl_renderer.is_none()
                            && !render_api_failed
                            && notifier_renderer_requested.load(Ordering::Acquire)
                        {
                            let source = notifier_render_source
                                .lock()
                                .ok()
                                .and_then(|source| source.clone());

                            if let Some(source) = source {
                                let renderer = if crate::benchmark_diagnostics::fault_requested(
                                    "renderer-init",
                                ) {
                                    Err(anyhow::anyhow!(
                                        "OXIDE_DIAGNOSTIC_FAULT:renderer-init"
                                    ))
                                } else {
                                    match graphics_api {
                                    slint::GraphicsAPI::NativeOpenGL { get_proc_address } => {
                                        let wake_app = app.as_weak();
                                        let native_redraw_hwnd = native_window_handle(app.window())
                                            .map(|hwnd| hwnd as usize);
                                        let redraw_pending = Arc::new(AtomicBool::new(false));
                                        let wake_pending = Arc::clone(&redraw_pending);
                                        let wake_renderer = move || {
                                            if let Some(target) = native_redraw_hwnd {
                                                request_native_redraw(target);
                                            }
                                            if wake_pending.swap(true, Ordering::AcqRel) {
                                                return;
                                            }
                                            let event_pending = Arc::clone(&wake_pending);
                                            if wake_app
                                                .upgrade_in_event_loop(move |app| {
                                                    event_pending.store(false, Ordering::Release);
                                                    app.window().request_redraw();
                                                })
                                                .is_err()
                                            {
                                                wake_pending.store(false, Ordering::Release);
                                            }
                                        };
                                        // SAFETY: BeforeRendering makes this
                                        // window's OpenGL context current. The
                                        // renderer remains in this notifier and
                                        // is dropped during RenderingTeardown.
                                        unsafe {
                                            MpvOpenGlRenderer::create(
                                                source,
                                                *get_proc_address,
                                                wake_renderer,
                                            )
                                        }
                                    }
                                    _ => Err(anyhow::anyhow!(
                                        "FemtoVG did not expose a native OpenGL context"
                                    )),
                                    }
                                };

                                match renderer {
                                    Ok(renderer) => {
                                        notifier_renderer_requested
                                            .store(false, Ordering::Release);
                                        gl_renderer = Some(renderer);
                                        app.set_integrated_video_renderer(true);
                                        let _ = player_tx.send(Command::RenderApiReady);
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "OpenGL video renderer unavailable; video remains disabled: {error:#}"
                                        );
                                        render_api_failed = true;
                                        notifier_renderer_requested
                                            .store(false, Ordering::Release);
                                        app.set_integrated_video_renderer(false);
                                        app.set_error_message(
                                            "Не удалось запустить безопасный вывод видео. Обновите драйвер видеокарты или отключите аппаратное декодирование в настройках."
                                                .into(),
                                        );
                                        app.set_error_visible(true);
                                        eprintln!("OXIDE_DIAGNOSTIC_STATE:renderer-error");
                                    }
                                }
                            }
                        }

                        if let Some(renderer) = gl_renderer.as_mut()
                            && video_surface_should_render(
                                app.get_media_presented(),
                                app.get_audio_media(),
                            )
                        {
                            let size = app.window().size();
                            let render_started = crate::benchmark_diagnostics::begin_render();
                            // SAFETY: BeforeRendering guarantees the same
                            // current OpenGL context used to create renderer.
                            let render_result =
                                unsafe { renderer.render((size.width, size.height)) };
                            crate::benchmark_diagnostics::finish_render(render_started);
                            if let Err(error) = render_result {
                                eprintln!("OpenGL video render failed: {error:#}");
                                gl_renderer = None;
                                render_api_failed = true;
                                app.set_integrated_video_renderer(false);
                                app.set_error_message(
                                    "Не удалось продолжить встроенный вывод видео".into(),
                                );
                                app.set_error_visible(true);
                            }
                        }
                    }
                    slint::RenderingState::AfterRendering => {
                        crate::benchmark_diagnostics::finish_scene_frame();
                        crate::benchmark_diagnostics::finish_ui_latency_frame();
                        if let Some(renderer) = gl_renderer.as_ref() {
                            // SAFETY: AfterRendering still has the original
                            // window OpenGL context current.
                            unsafe { renderer.report_swap() };
                        }
                    }
                    slint::RenderingState::RenderingTeardown => {
                        gl_renderer = None;
                    }
                    _ => {}
                }
            })
            .map_err(|error| anyhow::anyhow!("Не удалось подключить подготовку окна: {error}"))?;

        Ok(native_init_error_rx)
    }
}

fn video_surface_should_render(media_presented: bool, audio_media: bool) -> bool {
    media_presented && !audio_media
}

fn request_native_redraw(target: usize) {
    // SAFETY: target comes from the live Slint Win32 window handle. RedrawWindow
    // does not retain either null region pointer after the call.
    unsafe {
        RedrawWindow(
            target as HWND,
            std::ptr::null(),
            std::ptr::null_mut(),
            RDW_INTERNALPAINT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::video_surface_should_render;

    #[test]
    fn integrated_video_surface_never_paints_under_audio() {
        assert!(video_surface_should_render(true, false));
        assert!(!video_surface_should_render(true, true));
        assert!(!video_surface_should_render(false, false));
        assert!(!video_surface_should_render(false, true));
    }
}
