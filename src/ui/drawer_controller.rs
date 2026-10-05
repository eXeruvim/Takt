//! Owns the mutually exclusive inspector drawers and their media reservation.

use std::{cell::Cell, rc::Rc, time::Duration};

use slint::{ComponentHandle, Model};

use super::ui_logic::AppWindow;

#[cfg(target_os = "windows")]
use super::video_output::VideoOutputController;

const CLOSE_TRANSITION_DURATION: Duration = Duration::from_millis(360);
const SIDE_SWITCH_DURATION: Duration = Duration::from_millis(220);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drawer {
    Info,
    Playlist,
    Settings,
}

#[derive(Default)]
struct TransitionState {
    generation: Cell<u64>,
    pending: Cell<Option<Drawer>>,
}

#[derive(Clone)]
struct DrawerController {
    state: Rc<TransitionState>,
    #[cfg(target_os = "windows")]
    video_output: VideoOutputController,
}

pub(super) fn register_callbacks(
    app: &AppWindow,
    #[cfg(target_os = "windows")] video_output: VideoOutputController,
) {
    let controller = DrawerController {
        state: Rc::new(TransitionState::default()),
        #[cfg(target_os = "windows")]
        video_output,
    };

    {
        let app_weak = app.as_weak();
        let controller = controller.clone();
        app.on_toggle_info(move || {
            if let Some(app) = app_weak.upgrade() {
                controller.toggle(&app, Drawer::Info);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let controller = controller.clone();
        app.on_toggle_playlist(move || {
            if let Some(app) = app_weak.upgrade() {
                controller.toggle(&app, Drawer::Playlist);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let controller = controller.clone();
        app.on_toggle_settings(move || {
            if let Some(app) = app_weak.upgrade() {
                controller.toggle(&app, Drawer::Settings);
            }
        });
    }
}

impl DrawerController {
    fn toggle(&self, app: &AppWindow, drawer: Drawer) {
        if is_open(app, drawer) {
            app.set_inspector_side_switching(false);
            let generation = self.begin_transition(None);
            begin_close(app);
            self.release_media_after_close(app, generation);
            return;
        }

        if !can_open(app, drawer) {
            return;
        }

        if current_drawer(app).is_some_and(|current| shares_panel(current, drawer)) {
            app.set_inspector_side_switching(false);
            self.begin_transition(None);
            self.open(app, drawer);
            return;
        }

        if app.get_inspector_visible() || self.state.pending.get().is_some() {
            app.set_inspector_side_switching(true);
            let generation = self.begin_transition(Some(drawer));
            begin_close(app);
            self.open_after_close(app, drawer, generation);
        } else {
            app.set_inspector_side_switching(false);
            self.begin_transition(None);
            self.open(app, drawer);
        }
    }

    fn begin_transition(&self, pending: Option<Drawer>) -> u64 {
        let generation = self.state.generation.get().wrapping_add(1);
        self.state.generation.set(generation);
        self.state.pending.set(pending);
        generation
    }

    fn open_after_close(&self, app: &AppWindow, drawer: Drawer, generation: u64) {
        let app_weak = app.as_weak();
        let controller = self.clone();
        slint::Timer::single_shot(SIDE_SWITCH_DURATION, move || {
            if controller.state.generation.get() != generation
                || controller.state.pending.get() != Some(drawer)
            {
                return;
            }
            controller.state.pending.set(None);
            if let Some(app) = app_weak.upgrade() {
                if can_open(&app, drawer) {
                    controller.open(&app, drawer);
                    controller.finish_side_switch(&app, generation);
                } else {
                    app.set_inspector_side_switching(false);
                    controller.release_media(&app);
                }
            }
        });
    }

    fn finish_side_switch(&self, app: &AppWindow, generation: u64) {
        let app_weak = app.as_weak();
        let controller = self.clone();
        slint::Timer::single_shot(SIDE_SWITCH_DURATION, move || {
            if controller.state.generation.get() == generation
                && controller.state.pending.get().is_none()
                && let Some(app) = app_weak.upgrade()
            {
                app.set_inspector_side_switching(false);
            }
        });
    }

    fn open(&self, app: &AppWindow, drawer: Drawer) {
        app.set_inspector_left_aligned(!matches!(drawer, Drawer::Settings));
        app.set_inspector_audio_reserved(true);
        #[cfg(target_os = "windows")]
        app.set_inspector_reserved(true);
        app.set_info_drawer_visible(matches!(drawer, Drawer::Info));
        app.set_playlist_drawer_visible(matches!(drawer, Drawer::Playlist));
        app.set_settings_drawer_visible(matches!(drawer, Drawer::Settings));
        #[cfg(target_os = "windows")]
        self.video_output.sync_fallback(app);
    }

    fn release_media_after_close(&self, app: &AppWindow, generation: u64) {
        let delayed_app = app.as_weak();
        let controller = self.clone();
        slint::Timer::single_shot(CLOSE_TRANSITION_DURATION, move || {
            if controller.state.generation.get() == generation
                && controller.state.pending.get().is_none()
                && let Some(app) = delayed_app.upgrade()
                && !app.get_inspector_visible()
            {
                controller.release_media(&app);
            }
        });
    }

    #[cfg(target_os = "windows")]
    fn release_media(&self, app: &AppWindow) {
        app.set_inspector_reserved(false);
    }

    #[cfg(not(target_os = "windows"))]
    fn release_media(&self, _app: &AppWindow) {}
}

fn is_open(app: &AppWindow, drawer: Drawer) -> bool {
    match drawer {
        Drawer::Info => app.get_info_drawer_visible(),
        Drawer::Playlist => app.get_playlist_drawer_visible(),
        Drawer::Settings => app.get_settings_drawer_visible(),
    }
}

fn current_drawer(app: &AppWindow) -> Option<Drawer> {
    if app.get_info_drawer_visible() {
        Some(Drawer::Info)
    } else if app.get_playlist_drawer_visible() {
        Some(Drawer::Playlist)
    } else if app.get_settings_drawer_visible() {
        Some(Drawer::Settings)
    } else {
        None
    }
}

fn shares_panel(current: Drawer, target: Drawer) -> bool {
    matches!(current, Drawer::Info | Drawer::Playlist)
        && matches!(target, Drawer::Info | Drawer::Playlist)
}

fn begin_close(app: &AppWindow) {
    app.set_inspector_audio_reserved(false);
    app.set_info_drawer_visible(false);
    app.set_playlist_drawer_visible(false);
    app.set_settings_drawer_visible(false);
}

fn can_open(app: &AppWindow, drawer: Drawer) -> bool {
    match drawer {
        Drawer::Info => !app.get_current_media_path().is_empty(),
        Drawer::Playlist => app.get_playlist_items().row_count() > 0,
        Drawer::Settings => true,
    }
}
