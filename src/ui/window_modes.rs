use std::sync::{Arc, Mutex};

use slint::{
    ComponentHandle,
    winit_030::{
        WinitWindowAccessor,
        winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
    },
};

use super::{native_fullscreen::NativeFullscreen, ui_logic::AppWindow};

#[derive(Clone, Copy, Default)]
struct PictureInPictureState {
    active: bool,
    saved_position: Option<PhysicalPosition<i32>>,
    saved_size: Option<PhysicalSize<u32>>,
    was_maximized: bool,
}

#[derive(Clone, Default)]
pub(super) struct WindowModeController {
    native_fullscreen: Arc<Mutex<NativeFullscreen>>,
    picture_in_picture: Arc<Mutex<PictureInPictureState>>,
}

impl WindowModeController {
    pub(super) fn register_callbacks(&self, app: &AppWindow) {
        {
            let app_weak = app.as_weak();
            let controller = self.clone();
            app.on_toggle_fullscreen(move || {
                if let Some(app) = app_weak.upgrade() {
                    if app.get_picture_in_picture() {
                        controller.toggle_picture_in_picture(&app);
                    }
                    controller.toggle_fullscreen(&app);
                }
            });
        }

        {
            let app_weak = app.as_weak();
            let controller = self.clone();
            app.on_toggle_picture_in_picture(move || {
                if let Some(app) = app_weak.upgrade()
                    && app.get_media_loaded()
                {
                    controller.toggle_picture_in_picture(&app);
                }
            });
        }
    }

    pub(super) fn toggle_fullscreen(&self, app: &AppWindow) {
        let fullscreen = !app.get_fullscreen_mode();
        let result = self
            .native_fullscreen
            .lock()
            .map_err(|_| anyhow::anyhow!("Не удалось изменить полноэкранный режим"))
            .and_then(|mut native_fullscreen| native_fullscreen.set(app.window(), fullscreen));

        if let Err(error) = result {
            show_error(app, error.to_string());
        } else {
            app.set_fullscreen_mode(fullscreen);
            app.invoke_reveal_controls();
        }
    }

    fn toggle_picture_in_picture(&self, app: &AppWindow) {
        let entering = !app.get_picture_in_picture();
        if entering && app.get_fullscreen_mode() {
            self.toggle_fullscreen(app);
            if app.get_fullscreen_mode() {
                return;
            }
        }

        if entering {
            app.set_info_drawer_visible(false);
            app.set_playlist_drawer_visible(false);
            app.set_settings_drawer_visible(false);
            app.set_inspector_audio_reserved(false);
            app.set_inspector_side_switching(false);
            app.set_command_palette_visible(false);
            app.set_picture_in_picture(true);
        }

        let result = self
            .picture_in_picture
            .lock()
            .map_err(|_| anyhow::anyhow!("Не удалось изменить режим «картинка в картинке»"))
            .map(|mut state| {
                app.window().with_winit_window(|window| {
                    if entering {
                        if state.active {
                            return;
                        }
                        state.saved_position = window.outer_position().ok();
                        state.saved_size = Some(window.inner_size());
                        state.was_maximized = window.is_maximized();

                        window.set_minimized(false);
                        window.set_maximized(false);

                        let target_logical = LogicalSize::new(480.0, 300.0);
                        let target_physical: PhysicalSize<u32> =
                            target_logical.to_physical(window.scale_factor());
                        let _ = window.request_inner_size(target_physical);

                        if let Some(monitor) = window.current_monitor() {
                            let monitor_position = monitor.position();
                            let monitor_size = monitor.size();
                            let right_offset = monitor_size
                                .width
                                .saturating_sub(target_physical.width.saturating_add(32));
                            let bottom_offset = monitor_size
                                .height
                                .saturating_sub(target_physical.height.saturating_add(72));
                            window.set_outer_position(PhysicalPosition::new(
                                monitor_position.x
                                    + i32::try_from(right_offset).unwrap_or(i32::MAX),
                                monitor_position.y
                                    + i32::try_from(bottom_offset).unwrap_or(i32::MAX),
                            ));
                        }
                        state.active = true;
                    } else if state.active {
                        window.set_maximized(false);
                        if let Some(size) = state.saved_size {
                            let _ = window.request_inner_size(size);
                        }
                        if let Some(position) = state.saved_position {
                            window.set_outer_position(position);
                        }
                        if state.was_maximized {
                            window.set_maximized(true);
                        }
                        *state = PictureInPictureState::default();
                    }
                    window.request_redraw();
                });
            });

        if let Err(error) = result {
            if entering {
                app.set_picture_in_picture(false);
            }
            show_error(app, error.to_string());
        } else if !entering {
            app.set_picture_in_picture(false);
        }
        app.invoke_reveal_controls();
    }
}

fn show_error(app: &AppWindow, message: String) {
    app.set_error_message(message.into());
    app.set_error_visible(true);
}
