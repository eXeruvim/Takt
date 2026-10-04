#![deny(clippy::undocumented_unsafe_blocks)]

#[cfg(target_os = "windows")]
use std::cell::RefCell;
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};

use crate::converter::{ConverterCommand, ConverterEvent};
use crate::ipc::{AspectRatio, Command, EqualizerPreset, Event, FrameStepDirection};
use crate::player::commands::{CommandSender, SendError as PlayerCommandError};
use crate::player::playlist::{
    AUDIO_EXTENSIONS, MEDIA_EXTENSIONS, VIDEO_EXTENSIONS, is_media_file,
};
use crate::player::shortcuts::{
    ShortcutAction, ShortcutBinding, ShortcutDraftStatus, ShortcutEdit, ShortcutRegistry,
    is_modifier_key,
};
#[cfg(target_os = "windows")]
use crate::render::mpv_opengl::{MpvOpenGlRenderer, MpvRenderSource};
#[cfg(target_os = "windows")]
use crate::render::mpv_render::VideoHost;
use crate::settings::{AppSettings, ThemePreference, WindowPlacementPreference};
use crate::ui::command_catalog;
#[cfg(test)]
use crate::ui::command_catalog::COMMANDS as PALETTE_COMMANDS;
#[cfg(target_os = "windows")]
use crate::ui::native_fullscreen::NativeFullscreen;
use crate::ui::presentation::{
    format_bitrate, format_codec, format_container, format_file_size, format_precise_file_size,
    format_time, format_track_summary, is_audio_only_media, media_name_without_extension,
};
#[cfg(target_os = "windows")]
use crate::ui::system_notification;
#[cfg(target_os = "windows")]
use crate::ui::window_interaction::{
    WindowDragState, begin_manual_window_drag, finish_manual_window_drag, native_window_handle,
    update_manual_window_drag,
};
#[cfg(target_os = "windows")]
use crate::ui::window_placement::{merge_window_placement, restore_window_placement};
use slint::winit_030::{
    EventResult, WinitWindowAccessor,
    winit::{
        dpi::{LogicalSize, PhysicalPosition, PhysicalSize},
        event::{MouseButton, WindowEvent},
        keyboard::{KeyCode, ModifiersState, PhysicalKey},
        window::Theme as WinitTheme,
    },
};
use slint::{Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, HWND},
    Graphics::Gdi::{RDW_INTERNALPAINT, RedrawWindow},
    System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW},
};

slint::include_modules!();

#[cfg(target_os = "windows")]
// Release the media composition while the drawer is still fading out. Keeping
// this shorter than the visual dismissal avoids a noticeable pause before the
// artwork returns to its resting position.
const DRAWER_MEDIA_RELEASE_DELAY: Duration = Duration::from_millis(180);

#[cfg(target_os = "windows")]
fn windows_light_theme_flag(value_name: &str) -> Option<bool> {
    let subkey = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let value_name = format!("{value_name}\0").encode_utf16().collect::<Vec<_>>();
    let mut value = 1_u32;
    let mut value_size = std::mem::size_of::<u32>() as u32;

    // SAFETY: Both strings are NUL-terminated for the duration of the call, and
    // the output buffer points to a writable u32 with its exact byte length.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            value_name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            std::ptr::addr_of_mut!(value).cast(),
            std::ptr::addr_of_mut!(value_size),
        )
    };

    (status == ERROR_SUCCESS).then_some(value != 0)
}

#[cfg(target_os = "windows")]
fn windows_prefers_dark_apps() -> Option<bool> {
    windows_light_theme_flag("AppsUseLightTheme")
        .or_else(|| windows_light_theme_flag("SystemUsesLightTheme"))
        .map(|uses_light_theme| !uses_light_theme)
}

const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "idx", "sup"];

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, Default)]
struct PictureInPictureState {
    active: bool,
    saved_position: Option<PhysicalPosition<i32>>,
    saved_size: Option<PhysicalSize<u32>>,
    was_maximized: bool,
}

#[derive(Clone, Copy)]
enum DrawerSwipeTarget {
    Info,
    Playlist,
    Settings,
}

fn preview_shortcut_edit(
    app: &AppWindow,
    edit: &mut ShortcutEdit,
    registry: &ShortcutRegistry,
    action: ShortcutAction,
    bindings: Vec<ShortcutBinding>,
) {
    let transfer_notice = registry.transfer_notice(action, &bindings);
    let draft = edit.stage_state(registry, action, bindings);
    app.set_shortcut_capture_preview(draft.label.into());
    app.set_shortcut_capture_dirty(draft.dirty);
    match draft.status {
        ShortcutDraftStatus::Valid => {
            app.set_shortcut_capture_phase(ShortcutCapturePhase::Valid);
            app.set_shortcut_capture_error("".into());
        }
        ShortcutDraftStatus::Conflict(conflict) => {
            app.set_shortcut_capture_phase(ShortcutCapturePhase::Conflict);
            app.set_shortcut_capture_error(
                format!(
                    "{} занято командой «{}».\n{transfer_notice}",
                    conflict.binding.display(),
                    conflict.action.title()
                )
                .into(),
            );
        }
    }
}

fn clear_shortcut_candidate(app: &AppWindow) {
    app.set_shortcut_capture_phase(ShortcutCapturePhase::Idle);
    app.set_shortcut_capture_dirty(false);
    app.set_shortcut_capture_preview("".into());
    app.set_shortcut_capture_error("".into());
}

fn cancel_shortcut_capture(app: &AppWindow) {
    app.set_shortcut_capture_action(-1);
    app.set_shortcut_capture_focused(false);
    clear_shortcut_candidate(app);
}

fn dispatch_shortcut(app: &AppWindow, action: ShortcutAction) {
    app.invoke_reveal_controls();
    match action {
        ShortcutAction::PlayPause => app.invoke_play_pause(),
        ShortcutAction::Stop => app.invoke_stop(),
        ShortcutAction::Previous => app.invoke_previous_video(),
        ShortcutAction::Next => app.invoke_next_video(),
        ShortcutAction::SeekBackwardShort => app.invoke_seek_relative(-5.0),
        ShortcutAction::SeekForwardShort => app.invoke_seek_relative(5.0),
        ShortcutAction::SeekBackwardLong => app.invoke_seek_relative(-10.0),
        ShortcutAction::SeekForwardLong => app.invoke_seek_relative(10.0),
        ShortcutAction::FrameBackward => app.invoke_step_frame(-1),
        ShortcutAction::FrameForward => app.invoke_step_frame(1),
        ShortcutAction::VolumeDown => app.invoke_adjust_volume(-5.0),
        ShortcutAction::VolumeUp => app.invoke_adjust_volume(5.0),
        ShortcutAction::ToggleMute => app.invoke_toggle_mute(),
        ShortcutAction::ToggleLoop => app.invoke_toggle_loop(),
        ShortcutAction::ToggleFullscreen => app.invoke_toggle_fullscreen(),
        ShortcutAction::TogglePictureInPicture => app.invoke_toggle_picture_in_picture(),
        ShortcutAction::ToggleInfo => app.invoke_toggle_info(),
        ShortcutAction::TogglePlaylist => app.invoke_toggle_playlist(),
        ShortcutAction::ToggleCommandPalette => app.invoke_toggle_command_palette(),
        ShortcutAction::ToggleSettings => app.invoke_toggle_settings(),
        ShortcutAction::OpenConverter => app.invoke_open_converter(),
        ShortcutAction::OpenFile => app.invoke_open_file(),
        ShortcutAction::OpenFolder => app.invoke_open_folder(),
    }
}

fn command_palette_items_with_registry(
    query: &str,
    shortcuts: &ShortcutRegistry,
) -> Vec<CommandEntry> {
    command_catalog::search(query)
        .into_iter()
        .map(|command| CommandEntry {
            action: command.action,
            title: command.title.into(),
            description: command.description.into(),
            shortcut: ShortcutAction::from_palette_action(command.action)
                .map(|action| shortcuts.display_for(action))
                .unwrap_or_else(|| command.shortcut.to_string())
                .into(),
            requires_media: command.requires_media,
            shortcut_editable: ShortcutAction::from_palette_action(command.action).is_some(),
            default_shortcut: ShortcutAction::from_palette_action(command.action)
                .map(|action| {
                    let defaults = action.default_bindings();
                    if defaults.is_empty() {
                        "не назначено".to_string()
                    } else {
                        defaults
                            .iter()
                            .map(|binding| binding.display())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    }
                })
                .unwrap_or_default()
                .into(),
        })
        .collect()
}

#[cfg(test)]
fn command_palette_items(query: &str) -> Vec<CommandEntry> {
    command_palette_items_with_registry(query, &ShortcutRegistry::default())
}

fn set_command_palette_items(
    model: &VecModel<CommandEntry>,
    query: &str,
    shortcuts: &ShortcutRegistry,
) {
    let items = command_palette_items_with_registry(query, shortcuts);
    let can_update_in_place = model.row_count() == items.len()
        && items.iter().enumerate().all(|(index, item)| {
            model
                .row_data(index)
                .is_some_and(|current| current.action == item.action)
        });

    if can_update_in_place {
        // Preserve repeater instances so an expanded shortcut row can finish
        // its close animation after the saved label changes.
        for (index, item) in items.into_iter().enumerate() {
            model.set_row_data(index, item);
        }
    } else {
        model.set_vec(items);
    }
}

fn persist_shortcuts(
    app: &AppWindow,
    settings: &Arc<Mutex<AppSettings>>,
    shortcuts: &ShortcutRegistry,
) {
    if let Ok(mut settings) = settings.lock() {
        settings.set_shortcut_overrides(shortcuts.preferences());
        if let Err(error) = settings.save_if_dirty() {
            app.set_error_message(format!("Не удалось сохранить горячие клавиши: {error}").into());
            app.set_error_visible(true);
        }
    }
}

fn command_for_dropped_path(path: &Path) -> Result<Command, String> {
    if path.is_dir() {
        return Ok(Command::OpenFolder(path.to_owned()));
    }
    if path.is_file() && is_media_file(path) {
        return Ok(Command::OpenFile(path.to_owned()));
    }

    Err(format!(
        "Перетащенный объект не является поддерживаемым медиафайлом или папкой: {}",
        path.display()
    ))
}

#[cfg(target_os = "windows")]
fn sync_video_host(host: &mut VideoHost, app: &AppWindow) -> Result<()> {
    let size = app.window().size();
    sync_video_host_at_size(
        host,
        app,
        (size.width, size.height),
        app.window().scale_factor(),
    )
}

#[cfg(target_os = "windows")]
fn sync_video_host_at_size(
    host: &mut VideoHost,
    app: &AppWindow,
    physical_size: (u32, u32),
    scale: f32,
) -> Result<()> {
    let video_visible =
        video_surface_should_render(app.get_media_presented(), app.get_audio_media())
            && !app.get_modal_overlay_active()
            && !app.get_error_visible()
            && !app.get_drop_target_visible();
    host.set_visible(video_visible);

    host.resize(
        physical_size,
        scale,
        (
            app.get_video_left_panel_inset(),
            app.get_video_right_panel_inset(),
            app.get_video_top_inset(),
            app.get_video_bottom_inset(),
        ),
    )
}

fn video_surface_should_render(media_presented: bool, audio_media: bool) -> bool {
    media_presented && !audio_media
}

#[cfg(target_os = "windows")]
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

#[cfg(target_os = "windows")]
fn current_window_placement(
    app: &AppWindow,
    previous: Option<WindowPlacementPreference>,
    event_position: Option<PhysicalPosition<i32>>,
    event_size: Option<PhysicalSize<u32>>,
) -> Option<WindowPlacementPreference> {
    if app.get_fullscreen_mode() || app.get_picture_in_picture() || app.get_window_maximized() {
        return None;
    }

    app.window()
        .with_winit_window(|window| {
            if window.fullscreen().is_some()
                || window.is_maximized()
                || window.is_minimized().unwrap_or(false)
            {
                return None;
            }
            // During a winit callback, querying the window can still return
            // geometry from the preceding native event. Prefer the event's
            // authoritative value for the dimension that changed.
            let fallback_position = window.outer_position().ok()?;
            let fallback_size = window.inner_size().to_logical::<f64>(window.scale_factor());
            let event_size = event_size
                .map(|size| size.to_logical::<f64>(window.scale_factor()))
                .map(|size| (size.width, size.height));
            merge_window_placement(
                previous,
                event_position.map(|position| (position.x, position.y)),
                event_size,
                (fallback_position.x, fallback_position.y),
                (fallback_size.width, fallback_size.height),
            )
        })
        .flatten()
}

#[cfg(target_os = "windows")]
fn remember_window_placement(
    app: &AppWindow,
    settings: &Arc<Mutex<AppSettings>>,
    event_position: Option<PhysicalPosition<i32>>,
    event_size: Option<PhysicalSize<u32>>,
) {
    let previous = settings
        .lock()
        .ok()
        .and_then(|settings| settings.window_placement());
    if let Some(placement) = current_window_placement(app, previous, event_position, event_size)
        && let Ok(mut settings) = settings.lock()
    {
        settings.set_window_placement(placement);
    }
}

#[cfg(target_os = "windows")]
fn activate_hwnd_video_fallback(
    slot: &mut Option<VideoHost>,
    app: &AppWindow,
    tx: &CommandSender,
) -> Result<()> {
    let mut host = VideoHost::create(app.window())?;
    sync_video_host(&mut host, app)?;
    let _ = tx.send(Command::SetVideoWindow(host.handle()));
    *slot = Some(host);
    app.set_integrated_video_renderer(false);
    Ok(())
}

#[cfg(target_os = "windows")]
fn toggle_native_fullscreen(app: &AppWindow, native_fullscreen: &Arc<Mutex<NativeFullscreen>>) {
    let fullscreen = !app.get_fullscreen_mode();

    let result = native_fullscreen
        .lock()
        .map_err(|_| anyhow::anyhow!("Не удалось изменить полноэкранный режим"))
        .and_then(|mut native_fullscreen| native_fullscreen.set(app.window(), fullscreen));

    if let Err(error) = result {
        app.set_error_message(error.to_string().into());
        app.set_error_visible(true);
    } else {
        app.set_fullscreen_mode(fullscreen);
        app.invoke_reveal_controls();
    }
}

#[cfg(target_os = "windows")]
fn toggle_picture_in_picture(
    app: &AppWindow,
    state: &Arc<Mutex<PictureInPictureState>>,
    native_fullscreen: &Arc<Mutex<NativeFullscreen>>,
) {
    let entering = !app.get_picture_in_picture();
    if entering && app.get_fullscreen_mode() {
        toggle_native_fullscreen(app, native_fullscreen);
        if app.get_fullscreen_mode() {
            return;
        }
    }

    if entering {
        app.set_info_drawer_visible(false);
        app.set_playlist_drawer_visible(false);
        app.set_settings_drawer_visible(false);
        app.set_inspector_reserved(false);
        app.set_command_palette_visible(false);
        app.set_picture_in_picture(true);
    }

    let result = state
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
                            monitor_position.x + i32::try_from(right_offset).unwrap_or(i32::MAX),
                            monitor_position.y + i32::try_from(bottom_offset).unwrap_or(i32::MAX),
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
        app.set_error_message(error.to_string().into());
        app.set_error_visible(true);
    } else if !entering {
        app.set_picture_in_picture(false);
    }
    app.invoke_reveal_controls();
}

pub fn run(
    tx: CommandSender,
    rx: Receiver<Event>,
    command_errors: Receiver<PlayerCommandError>,
    converter_tx: Sender<ConverterCommand>,
    converter_rx: Receiver<ConverterEvent>,
    converter_control: Arc<crate::converter::ConverterControl>,
) -> Result<()> {
    let app = AppWindow::new()?;
    let error_weak = app.as_weak();
    let command_error_bridge = super::event_delivery::spawn(
        command_errors,
        |_, _| true,
        move |errors, completion| {
            let weak = error_weak.clone();
            slint::invoke_from_event_loop(move || {
                let _completion = completion;
                if let Some(app) = weak.upgrade()
                    && let Some(error) = errors.last()
                {
                    app.set_error_message(error.message().into());
                    app.set_error_visible(true);
                }
            })
            .is_ok()
        },
    )?;
    crate::ui::shortcut_close::install(&app);
    app.set_app_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    app.set_window_scale_factor(app.window().scale_factor());
    let settings = Arc::new(Mutex::new(AppSettings::load_default()));
    #[cfg(target_os = "windows")]
    app.set_system_dark_theme(windows_prefers_dark_apps().unwrap_or(false));
    if let Ok(mut settings) = settings.lock() {
        if let Some(message) = settings.take_storage_notice() {
            eprintln!("{message}");
            app.set_error_message(message.into());
            app.set_error_visible(true);
        }
        app.set_theme_mode(settings.theme().ui_index());
        app.set_accent_mode(settings.accent().ui_index());
        app.set_volume(settings.default_volume());
        app.set_speed(settings.default_speed());
        app.set_folder_loop_enabled(settings.completion_behavior().repeats_playlist());
        app.set_hardware_decoding_enabled(settings.hardware_decoding_enabled());
        let _ = tx.send(Command::SetVolume(
            f64::from(settings.default_volume()) * 100.0,
        ));
        let _ = tx.send(Command::SetSpeed(f64::from(settings.default_speed())));
        let _ = tx.send(Command::SetFolderLoop(
            settings.completion_behavior().repeats_playlist(),
        ));
        let _ = tx.send(Command::SetHardwareDecoding(
            settings.hardware_decoding_enabled(),
        ));
    }
    super::conversion_controller::apply_presets(&app, false, "");
    let shortcut_registry = Arc::new(Mutex::new(
        settings
            .lock()
            .map(|settings| ShortcutRegistry::from_preferences(settings.shortcut_overrides()))
            .unwrap_or_default(),
    ));
    let shortcut_edit = Arc::new(Mutex::new(ShortcutEdit::default()));
    let command_items = Rc::new(VecModel::default());
    app.set_command_items(ModelRc::from(command_items.clone()));
    if let Ok(shortcuts) = shortcut_registry.lock() {
        set_command_palette_items(&command_items, "", &shortcuts);
    }
    let current_duration = Arc::new(Mutex::new(0.0));
    let dialog_active = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "windows")]
    let video_host = Rc::new(RefCell::new(None::<VideoHost>));
    #[cfg(target_os = "windows")]
    let render_source = Arc::new(Mutex::new(None::<MpvRenderSource>));
    #[cfg(target_os = "windows")]
    let video_renderer_requested = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "windows")]
    let native_fullscreen = Arc::new(Mutex::new(NativeFullscreen::default()));
    #[cfg(target_os = "windows")]
    let picture_in_picture_state = Arc::new(Mutex::new(PictureInPictureState::default()));
    #[cfg(target_os = "windows")]
    let video_resize_timer = Rc::new(slint::Timer::default());
    #[cfg(target_os = "windows")]
    let window_drag_state = Arc::new(Mutex::new(WindowDragState::default()));

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let window_drag_state = window_drag_state.clone();
        app.on_drag_window(move || {
            if let Some(app) = app_weak.upgrade() {
                let window = app.window();
                if app.get_fullscreen_mode() {
                    return;
                }
                if !begin_manual_window_drag(window, &window_drag_state) {
                    window.with_winit_window(|window| {
                        let _ = window.drag_window();
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();
        app.on_drag_window(move || {
            if let Some(app) = app_weak.upgrade() {
                app.window().with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            }
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let shortcut_edit = shortcut_edit.clone();
        #[cfg(target_os = "windows")]
        let event_window_drag_state = window_drag_state.clone();
        #[cfg(target_os = "windows")]
        let event_settings = settings.clone();
        let mut modifiers = ModifiersState::empty();
        let mut last_cursor_position = None::<(f64, f64)>;
        let mut drawer_swipe = None::<(DrawerSwipeTarget, f64)>;
        let mut theme_initialized = false;
        #[cfg(target_os = "windows")]
        let mut fullscreen_escape_blocked_until = None::<Instant>;

        app.window().on_winit_window_event(move |window, event| {
            let Some(app) = app_weak.upgrade() else {
                return EventResult::Propagate;
            };

            if !theme_initialized {
                let system_dark_theme = window
                    .with_winit_window(|native_window| {
                        native_window.theme().map(|theme| theme == WinitTheme::Dark)
                    })
                    .flatten()
                    .or_else(windows_prefers_dark_apps)
                    .unwrap_or(app.get_system_dark_theme());
                app.set_system_dark_theme(system_dark_theme);
                let _ = tx.send(Command::SetDarkTheme(app.get_dark_theme()));
                theme_initialized = true;
            }

            match event {
                WindowEvent::CursorMoved { position, .. } => {
                    #[cfg(target_os = "windows")]
                    update_manual_window_drag(window, &event_window_drag_state);

                    let scale = f64::from(app.window().scale_factor());
                    let window_height = f64::from(app.window().size().height);
                    let top_zone = f64::from(app.get_controls_top_zone()) * scale;
                    let bottom_zone = f64::from(app.get_controls_bottom_zone()) * scale;
                    app.set_controls_pointer_over(
                        position.y <= top_zone || position.y >= window_height - bottom_zone,
                    );
                    let current_position = (position.x, position.y);
                    if last_cursor_position != Some(current_position) {
                        last_cursor_position = Some(current_position);
                        app.invoke_reveal_controls();
                    }
                    if let Some((target, start_x)) = drawer_swipe {
                        let delta = position.x - start_x;
                        let threshold = 48.0 * scale;
                        let should_close = match target {
                            DrawerSwipeTarget::Info | DrawerSwipeTarget::Playlist => {
                                delta < -threshold
                            }
                            DrawerSwipeTarget::Settings => delta > threshold,
                        };
                        if should_close {
                            drawer_swipe = None;
                            match target {
                                DrawerSwipeTarget::Info => app.invoke_toggle_info(),
                                DrawerSwipeTarget::Playlist => app.invoke_toggle_playlist(),
                                DrawerSwipeTarget::Settings => app.invoke_toggle_settings(),
                            }
                        }
                    }
                }
                WindowEvent::CursorLeft { .. } => {
                    app.set_controls_pointer_over(false);
                }
                WindowEvent::Focused(focused) => {
                    #[cfg(target_os = "windows")]
                    if *focused && !app.get_window_active() && app.get_fullscreen_mode() {
                        // Windows overlays (for example Snipping Tool) may return
                        // focus and forward the Escape that dismissed them to the
                        // underlying window. Do not treat that same key press as a
                        // request to leave fullscreen.
                        fullscreen_escape_blocked_until =
                            Some(Instant::now() + Duration::from_millis(450));
                    }
                    app.set_window_active(*focused);
                    #[cfg(target_os = "windows")]
                    if !focused {
                        finish_manual_window_drag(&event_window_drag_state);
                    }
                }
                WindowEvent::ThemeChanged(theme) => {
                    app.set_system_dark_theme(matches!(theme, WinitTheme::Dark));
                    if app.get_theme_mode() == ThemePreference::System.ui_index() {
                        let _ = tx.send(Command::SetDarkTheme(app.get_dark_theme()));
                        window.request_redraw();
                    }
                }
                WindowEvent::Resized(size) => {
                    // Keep Slint and the native mpv child on the same frame. Resizing the child
                    // here makes video geometry lead the FemtoVG control deck by one paint.
                    // The AfterRendering notifier commits both dimensions together.
                    #[cfg(target_os = "windows")]
                    {
                        remember_window_placement(&app, &event_settings, None, Some(*size));
                    }
                    // This filter runs before Slint's winit backend applies the
                    // authoritative size. Requesting a redraw here queues a frame
                    // with the previous layout. Slint schedules the correct frame
                    // after resize_event updates the window and renderer surface.
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    app.set_window_scale_factor(*scale_factor as f32);
                    window.request_redraw();
                }
                WindowEvent::Moved(position) => {
                    #[cfg(target_os = "windows")]
                    remember_window_placement(&app, &event_settings, Some(*position), None);
                    // Moving a window does not change its contents. In particular,
                    // resizing from the left/top produces Moved immediately before
                    // Resized; requesting a frame here presents the old surface
                    // geometry between two authoritative resize frames. The native
                    // size/move timer already keeps video advancing while Windows is
                    // running its modal interaction loop.
                }
                WindowEvent::CloseRequested => {
                    #[cfg(target_os = "windows")]
                    {
                        if let Ok(mut settings) = event_settings.lock()
                            && let Err(error) = settings.save_if_dirty()
                        {
                            eprintln!("Не удалось сохранить положение окна: {error}");
                        }
                    }
                }
                WindowEvent::MouseInput {
                    state,
                    button: MouseButton::Left,
                    ..
                } => {
                    if state.is_pressed() {
                        let Some((pointer_x, pointer_y)) = last_cursor_position else {
                            return EventResult::Propagate;
                        };
                        let scale = f64::from(app.window().scale_factor());
                        let window_size = app.window().size();
                        let window_width = f64::from(window_size.width);
                        let window_height = f64::from(window_size.height);
                        let logical_width = window_width / scale;
                        let panel_top = f64::from(app.get_controls_top_zone()) * scale;
                        let panel_bottom = window_height
                            - (f64::from(app.get_controls_bottom_zone()) + 4.0) * scale;

                        if pointer_y >= panel_top && pointer_y <= panel_bottom {
                            let inspector_width = logical_width.min(392.0 + 32.0) - 32.0;
                            let (inspector_start, inspector_end) =
                                if app.get_inspector_left_aligned() {
                                    (16.0 * scale, (16.0 + inspector_width) * scale)
                                } else {
                                    (
                                        (logical_width - inspector_width - 16.0) * scale,
                                        (logical_width - 16.0) * scale,
                                    )
                                };

                            let target =
                                if pointer_x >= inspector_start && pointer_x <= inspector_end {
                                    if app.get_playlist_drawer_visible() {
                                        Some(DrawerSwipeTarget::Playlist)
                                    } else if app.get_info_drawer_visible() {
                                        Some(DrawerSwipeTarget::Info)
                                    } else if app.get_settings_drawer_visible() {
                                        Some(DrawerSwipeTarget::Settings)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };
                            drawer_swipe = target.map(|target| (target, pointer_x));
                        }
                    } else {
                        drawer_swipe = None;
                        #[cfg(target_os = "windows")]
                        finish_manual_window_drag(&event_window_drag_state);
                    }
                }
                WindowEvent::ModifiersChanged(new_modifiers) => {
                    modifiers = new_modifiers.state();
                }
                WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                    let PhysicalKey::Code(code) = event.physical_key else {
                        return EventResult::Propagate;
                    };

                    #[cfg(target_os = "windows")]
                    if code == KeyCode::Escape
                        && finish_manual_window_drag(&event_window_drag_state)
                    {
                        return EventResult::PreventDefault;
                    }

                    if app.get_resume_prompt_visible() {
                        if code == KeyCode::Enter {
                            app.invoke_resume_saved_playback();
                        } else if code == KeyCode::Escape {
                            app.invoke_restart_saved_playback();
                        }
                        return EventResult::PreventDefault;
                    }

                    if app.get_clear_playback_data_prompt_visible() {
                        if code == KeyCode::Escape {
                            app.set_clear_playback_data_prompt_visible(false);
                            return EventResult::PreventDefault;
                        }
                        return EventResult::Propagate;
                    }

                    if app.get_reset_settings_prompt_visible() {
                        if code == KeyCode::Escape {
                            app.set_reset_settings_prompt_visible(false);
                            return EventResult::PreventDefault;
                        }
                        return EventResult::Propagate;
                    }

                    if app.get_converter_visible() {
                        if code == KeyCode::Escape {
                            app.set_converter_visible(false);
                        }
                        return EventResult::PreventDefault;
                    }

                    // The close countdown owns keyboard focus; never record its buttons' keys.
                    if app.get_command_palette_visible() && app.get_shortcut_close_pending() {
                        return EventResult::Propagate;
                    }

                    let capture_action = app.get_shortcut_capture_action();
                    if capture_action >= 0 && app.get_command_palette_visible() {
                        if code == KeyCode::Escape {
                            cancel_shortcut_capture(&app);
                            return EventResult::PreventDefault;
                        }

                        if event.repeat {
                            return EventResult::PreventDefault;
                        }
                        // Tab explicitly leaves capture so focused actions can receive Enter/Space.
                        if code == KeyCode::Tab {
                            if app.get_shortcut_capture_phase() == ShortcutCapturePhase::Recording {
                                app.set_shortcut_capture_phase(ShortcutCapturePhase::Idle);
                                app.set_shortcut_capture_preview("Запись приостановлена".into());
                            }
                            return EventResult::Propagate;
                        }
                        if app.get_shortcut_capture_phase() != ShortcutCapturePhase::Recording
                            && !app.get_shortcut_capture_focused()
                        {
                            return EventResult::Propagate;
                        }
                        if is_modifier_key(code) {
                            return EventResult::PreventDefault;
                        }
                        let Some(action) = ShortcutAction::from_id(capture_action) else {
                            cancel_shortcut_capture(&app);
                            return EventResult::PreventDefault;
                        };
                        let binding = ShortcutBinding::new(code, modifiers);
                        if let Ok(shortcuts) = shortcut_registry.lock()
                            && let Ok(mut edit) = shortcut_edit.lock()
                        {
                            preview_shortcut_edit(
                                &app,
                                &mut edit,
                                &shortcuts,
                                action,
                                vec![binding],
                            );
                        }
                        return EventResult::PreventDefault;
                    }

                    if app.get_command_palette_visible() && code == KeyCode::Escape {
                        app.set_command_palette_visible(false);
                        app.set_command_query("".into());
                        app.invoke_command_query_changed("".into());
                        return EventResult::PreventDefault;
                    }

                    if code == KeyCode::Escape && app.get_inspector_visible() {
                        if app.get_playlist_drawer_visible() {
                            app.invoke_toggle_playlist();
                        } else if app.get_info_drawer_visible() {
                            app.invoke_toggle_info();
                        } else if app.get_settings_drawer_visible() {
                            app.invoke_toggle_settings();
                        }
                        return EventResult::PreventDefault;
                    }

                    if code == KeyCode::Escape && app.get_fullscreen_mode() {
                        #[cfg(target_os = "windows")]
                        if fullscreen_escape_blocked_until
                            .is_some_and(|deadline| Instant::now() < deadline)
                        {
                            return EventResult::PreventDefault;
                        }
                        app.invoke_toggle_fullscreen();
                        return EventResult::PreventDefault;
                    }

                    let binding = ShortcutBinding::new(code, modifiers);
                    let Some(action) = shortcut_registry
                        .lock()
                        .ok()
                        .and_then(|shortcuts| shortcuts.action_for(binding))
                    else {
                        return EventResult::Propagate;
                    };

                    if app.get_command_palette_visible()
                        && action != ShortcutAction::ToggleCommandPalette
                    {
                        return EventResult::Propagate;
                    }
                    if !event.repeat || action.allows_repeat() {
                        dispatch_shortcut(&app, action);
                    }
                    return EventResult::PreventDefault;
                }
                WindowEvent::HoveredFile(path) => {
                    app.set_drop_target_visible(path.is_dir() || is_media_file(path));
                }
                WindowEvent::HoveredFileCancelled => {
                    app.set_drop_target_visible(false);
                }
                WindowEvent::DroppedFile(path) => {
                    app.set_drop_target_visible(false);
                    match command_for_dropped_path(path) {
                        Ok(command) => {
                            let _ = tx.send(command);
                        }
                        Err(message) => {
                            app.set_error_message(message.into());
                            app.set_error_visible(true);
                        }
                    }
                }
                _ => {}
            }

            EventResult::Propagate
        });
    }

    super::settings_controller::register_preference_callbacks(&app, &tx, &settings);

    {
        let tx = tx.clone();
        let dialog_active = dialog_active.clone();

        app.on_open_file(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = dialog_active.clone();
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new()
                    .add_filter("Медиа", MEDIA_EXTENSIONS)
                    .add_filter("Видео", VIDEO_EXTENSIONS)
                    .add_filter("Аудио", AUDIO_EXTENSIONS)
                    .pick_file();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::OpenFile(path));
                }
            });
        });
    }

    {
        let tx = tx.clone();
        app.on_open_recent(move |path| {
            let _ = tx.send(Command::OpenFile(PathBuf::from(path.to_string())));
        });
    }

    {
        let tx = tx.clone();
        app.on_clear_playback_data(move || {
            let _ = tx.send(Command::ClearPlaybackData);
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        app.on_resume_saved_playback(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_resume_prompt_visible(false);
            }
            let _ = tx.send(Command::ResolveResume(true));
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        app.on_restart_saved_playback(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_resume_prompt_visible(false);
            }
            let _ = tx.send(Command::ResolveResume(false));
        });
    }

    {
        let tx = tx.clone();
        let dialog_active = dialog_active.clone();

        app.on_load_subtitle(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = dialog_active.clone();
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new()
                    .add_filter("Субтитры", SUBTITLE_EXTENSIONS)
                    .pick_file();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::LoadSubtitle(path));
                }
            });
        });
    }

    {
        let tx = tx.clone();
        let dialog_active = dialog_active.clone();

        app.on_open_folder(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let tx = tx.clone();
            let dialog_active = dialog_active.clone();
            std::thread::spawn(move || {
                let path = rfd::FileDialog::new().pick_folder();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let _ = tx.send(Command::OpenFolder(path));
                }
            });
        });
    }

    {
        let tx = tx.clone();

        app.on_play_pause(move || {
            let _ = tx.send(Command::TogglePlay);
        });
    }

    {
        let tx = tx.clone();

        app.on_stop(move || {
            let _ = tx.send(Command::Stop);
        });
    }

    {
        let tx = tx.clone();
        let current_duration = current_duration.clone();

        app.on_seek(move |value| {
            let duration = current_duration
                .lock()
                .map(|duration| *duration)
                .unwrap_or_default();
            let seek_to = value as f64 * duration;
            let _ = tx.send(Command::Seek(seek_to));
        });
    }

    {
        let tx = tx.clone();

        app.on_set_volume(move |value| {
            let _ = tx.send(Command::SetVolume(value as f64 * 100.0));
        });
    }

    {
        let tx = tx.clone();

        app.on_set_speed(move |value| {
            let _ = tx.send(Command::SetSpeed(value as f64));
        });
    }

    {
        let tx = tx.clone();

        app.on_select_audio_track(move |id| {
            let _ = tx.send(Command::SelectAudioTrack(i64::from(id)));
        });
    }

    {
        let tx = tx.clone();

        app.on_select_subtitle_track(move |id| {
            let id = (id >= 0).then(|| i64::from(id));
            let _ = tx.send(Command::SelectSubtitleTrack(id));
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_audio_delay(move |delta| {
            let _ = tx.send(Command::AdjustAudioDelay(f64::from(delta)));
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_subtitle_delay(move |delta| {
            let _ = tx.send(Command::AdjustSubtitleDelay(f64::from(delta)));
        });
    }

    {
        let tx = tx.clone();
        app.on_reset_audio_delay(move || {
            let _ = tx.send(Command::ResetAudioDelay);
        });
    }

    {
        let tx = tx.clone();
        app.on_reset_subtitle_delay(move || {
            let _ = tx.send(Command::ResetSubtitleDelay);
        });
    }

    {
        let tx = tx.clone();
        app.on_select_chapter(move |index| {
            let _ = tx.send(Command::SelectChapter(i64::from(index)));
        });
    }

    {
        let tx = tx.clone();
        app.on_set_aspect_ratio(move |index| {
            if let Some(aspect_ratio) = AspectRatio::from_ui_index(index) {
                let _ = tx.send(Command::SetAspectRatio(aspect_ratio));
            }
        });
    }

    {
        let tx = tx.clone();
        app.on_set_equalizer_enabled(move |enabled| {
            let _ = tx.send(Command::SetEqualizerEnabled(enabled));
        });
    }

    {
        let tx = tx.clone();
        app.on_set_equalizer_preset(move |index| {
            if let Some(preset) = EqualizerPreset::from_ui_index(index) {
                let _ = tx.send(Command::SetEqualizerPreset(preset));
            }
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_equalizer_preamp(move |delta| {
            let _ = tx.send(Command::AdjustEqualizerPreamp(f64::from(delta)));
        });
    }

    {
        let tx = tx.clone();
        app.on_adjust_equalizer_band(move |index, delta| {
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            let _ = tx.send(Command::AdjustEqualizerBand {
                index,
                delta_db: f64::from(delta),
            });
        });
    }

    {
        let tx = tx.clone();
        app.on_reset_equalizer(move || {
            let _ = tx.send(Command::ResetEqualizer);
        });
    }

    {
        let tx = tx.clone();

        app.on_next_video(move || {
            let _ = tx.send(Command::NextVideo);
        });
    }

    {
        let tx = tx.clone();

        app.on_previous_video(move || {
            let _ = tx.send(Command::PreviousVideo);
        });
    }

    {
        let tx = tx.clone();

        app.on_toggle_loop(move || {
            let _ = tx.send(Command::ToggleLoop);
        });
    }

    {
        let tx = tx.clone();

        app.on_toggle_shuffle(move || {
            let _ = tx.send(Command::ToggleShuffle);
        });
    }

    {
        let tx = tx.clone();

        app.on_request_media_info(move || {
            let _ = tx.send(Command::RequestMediaInfo);
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let native_fullscreen = native_fullscreen.clone();
        let picture_in_picture_state = picture_in_picture_state.clone();

        app.on_toggle_fullscreen(move || {
            if let Some(app) = app_weak.upgrade() {
                if app.get_picture_in_picture() {
                    toggle_picture_in_picture(&app, &picture_in_picture_state, &native_fullscreen);
                }
                toggle_native_fullscreen(&app, &native_fullscreen);
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let native_fullscreen = native_fullscreen.clone();
        let picture_in_picture_state = picture_in_picture_state.clone();

        app.on_toggle_picture_in_picture(move || {
            if let Some(app) = app_weak.upgrade()
                && app.get_media_loaded()
            {
                toggle_picture_in_picture(&app, &picture_in_picture_state, &native_fullscreen);
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();

        app.on_toggle_fullscreen(move || {
            if let Some(app) = app_weak.upgrade() {
                let fullscreen = !app.window().is_fullscreen();
                app.set_fullscreen_mode(fullscreen);
                app.window().set_fullscreen(fullscreen);
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();

        app.on_toggle_picture_in_picture(move || {
            if let Some(app) = app_weak.upgrade()
                && app.get_media_loaded()
            {
                app.set_picture_in_picture(!app.get_picture_in_picture());
                app.invoke_reveal_controls();
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let panel_video_host = video_host.clone();

        app.on_toggle_info(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_info_drawer_visible();
                if opening && app.get_current_media_path().is_empty() {
                    return;
                }
                if opening {
                    app.set_inspector_left_aligned(true);
                    app.set_inspector_reserved(true);
                    app.set_info_drawer_visible(true);
                    app.set_playlist_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                    if let Ok(mut video_host) = panel_video_host.try_borrow_mut()
                        && let Some(host) = video_host.as_mut()
                    {
                        let _ = sync_video_host(host, &app);
                    }
                } else {
                    app.set_info_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(DRAWER_MEDIA_RELEASE_DELAY, move || {
                        if let Some(app) = delayed_app.upgrade()
                            && !app.get_inspector_visible()
                        {
                            app.set_inspector_reserved(false);
                        }
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();
        app.on_toggle_info(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_info_drawer_visible();
                if opening && app.get_current_media_path().is_empty() {
                    return;
                }
                if opening {
                    app.set_inspector_left_aligned(true);
                    app.set_info_drawer_visible(true);
                    app.set_playlist_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                } else {
                    app.set_info_drawer_visible(false);
                }
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let panel_video_host = video_host.clone();

        app.on_toggle_playlist(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_playlist_drawer_visible();
                if opening && app.get_playlist_items().row_count() == 0 {
                    return;
                }
                if opening {
                    app.set_inspector_left_aligned(true);
                    app.set_inspector_reserved(true);
                    app.set_playlist_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                    if let Ok(mut video_host) = panel_video_host.try_borrow_mut()
                        && let Some(host) = video_host.as_mut()
                    {
                        let _ = sync_video_host(host, &app);
                    }
                } else {
                    app.set_playlist_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(DRAWER_MEDIA_RELEASE_DELAY, move || {
                        if let Some(app) = delayed_app.upgrade()
                            && !app.get_inspector_visible()
                        {
                            app.set_inspector_reserved(false);
                        }
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();
        app.on_toggle_playlist(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_playlist_drawer_visible();
                if opening && app.get_playlist_items().row_count() == 0 {
                    return;
                }
                if opening {
                    app.set_inspector_left_aligned(true);
                    app.set_playlist_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_settings_drawer_visible(false);
                } else {
                    app.set_playlist_drawer_visible(false);
                }
            }
        });
    }

    #[cfg(target_os = "windows")]
    {
        let app_weak = app.as_weak();
        let panel_video_host = video_host.clone();

        app.on_toggle_settings(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_settings_drawer_visible();
                if opening {
                    app.set_inspector_left_aligned(false);
                    app.set_inspector_reserved(true);
                    app.set_settings_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_playlist_drawer_visible(false);
                    if let Ok(mut video_host) = panel_video_host.try_borrow_mut()
                        && let Some(host) = video_host.as_mut()
                    {
                        let _ = sync_video_host(host, &app);
                    }
                } else {
                    app.set_settings_drawer_visible(false);
                    let delayed_app = app.as_weak();
                    slint::Timer::single_shot(DRAWER_MEDIA_RELEASE_DELAY, move || {
                        if let Some(app) = delayed_app.upgrade()
                            && !app.get_inspector_visible()
                        {
                            app.set_inspector_reserved(false);
                        }
                    });
                }
            }
        });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let app_weak = app.as_weak();
        app.on_toggle_settings(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_settings_drawer_visible();
                if opening {
                    app.set_inspector_left_aligned(false);
                    app.set_settings_drawer_visible(true);
                    app.set_info_drawer_visible(false);
                    app.set_playlist_drawer_visible(false);
                } else {
                    app.set_settings_drawer_visible(false);
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();

        app.on_toggle_command_palette(move || {
            if let Some(app) = app_weak.upgrade() {
                let opening = !app.get_command_palette_visible();
                app.set_converter_visible(false);
                cancel_shortcut_capture(&app);
                app.set_command_query("".into());
                if !opening {
                    app.invoke_command_query_changed("".into());
                }
                app.set_command_palette_visible(opening);
            }
        });
    }

    super::conversion_controller::register_callbacks(
        &app,
        converter_tx,
        converter_control,
        dialog_active.clone(),
    );

    {
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let command_items = command_items.clone();

        app.on_command_query_changed(move |query| {
            if app_weak.upgrade().is_some()
                && let Ok(shortcuts) = shortcut_registry.lock()
            {
                set_command_palette_items(&command_items, query.as_str(), &shortcuts);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let settings = settings.clone();
        let command_items = command_items.clone();

        app.on_reset_shortcuts(move || {
            if let Some(app) = app_weak.upgrade() {
                if let Ok(mut shortcuts) = shortcut_registry.lock() {
                    shortcuts.reset();
                    set_command_palette_items(
                        &command_items,
                        app.get_command_query().as_str(),
                        &shortcuts,
                    );
                    persist_shortcuts(&app, &settings, &shortcuts);
                }
                cancel_shortcut_capture(&app);
            }
        });
    }

    {
        let tx = tx.clone();
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let shortcut_edit = shortcut_edit.clone();
        let settings = settings.clone();
        let command_items = command_items.clone();

        app.on_reset_settings(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let defaults = AppSettings::default();

            let Ok(mut shortcuts) = shortcut_registry.lock() else {
                app.set_error_message("Не удалось получить доступ к горячим клавишам".into());
                app.set_error_visible(true);
                return;
            };
            let Ok(mut stored_settings) = settings.lock() else {
                app.set_error_message("Не удалось получить доступ к настройкам".into());
                app.set_error_visible(true);
                return;
            };
            stored_settings.reset_to_defaults();
            if let Err(error) = stored_settings.save_if_dirty() {
                app.set_error_message(format!("Не удалось сбросить настройки: {error}").into());
                app.set_error_visible(true);
                return;
            }
            shortcuts.reset();
            set_command_palette_items(&command_items, app.get_command_query().as_str(), &shortcuts);
            drop(shortcuts);
            drop(stored_settings);

            if let Ok(mut edit) = shortcut_edit.lock() {
                edit.discard();
            }
            cancel_shortcut_capture(&app);

            app.set_theme_mode(defaults.theme().ui_index());
            app.set_accent_mode(defaults.accent().ui_index());
            app.set_volume(defaults.default_volume());
            app.set_speed(defaults.default_speed());
            app.set_folder_loop_enabled(defaults.completion_behavior().repeats_playlist());
            app.set_hardware_decoding_enabled(defaults.hardware_decoding_enabled());
            app.set_settings_reset_done(true);
            app.set_reset_settings_prompt_visible(false);
            app.set_notice_message("Настройки восстановлены".into());
            app.set_notice_visible(true);

            let _ = tx.send(Command::SetDarkTheme(app.get_dark_theme()));
            let _ = tx.send(Command::SetVolume(
                f64::from(defaults.default_volume()) * 100.0,
            ));
            let _ = tx.send(Command::SetSpeed(f64::from(defaults.default_speed())));
            let _ = tx.send(Command::SetFolderLoop(
                defaults.completion_behavior().repeats_playlist(),
            ));
            let _ = tx.send(Command::SetHardwareDecoding(
                defaults.hardware_decoding_enabled(),
            ));
            app.window().request_redraw();

            let notice_app = app.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_millis(2600), move || {
                if let Some(app) = notice_app.upgrade() {
                    app.set_notice_visible(false);
                }
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let shortcut_edit = shortcut_edit.clone();
        app.on_begin_shortcut_edit(move |action| {
            if let Some(app) = app_weak.upgrade()
                && ShortcutAction::from_id(action).is_some()
                && let Ok(mut edit) = shortcut_edit.lock()
            {
                edit.discard();
                app.set_shortcut_saved_action(-1);
                clear_shortcut_candidate(&app);
                app.set_shortcut_capture_phase(ShortcutCapturePhase::Recording);
                app.set_shortcut_capture_action(action);
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let shortcut_edit = shortcut_edit.clone();
        app.on_preview_default_shortcut(move || {
            if let Some(app) = app_weak.upgrade()
                && let Some(action) = ShortcutAction::from_id(app.get_shortcut_capture_action())
                && let Ok(shortcuts) = shortcut_registry.lock()
                && let Ok(mut edit) = shortcut_edit.lock()
            {
                preview_shortcut_edit(
                    &app,
                    &mut edit,
                    &shortcuts,
                    action,
                    action.default_bindings(),
                );
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let shortcut_registry = shortcut_registry.clone();
        let shortcut_edit = shortcut_edit.clone();
        let settings = settings.clone();
        let command_items = command_items.clone();
        app.on_save_shortcut_edit(move |overwrite| {
            if let Some(app) = app_weak.upgrade()
                && app.get_command_palette_visible()
                && (app.get_shortcut_capture_phase() == ShortcutCapturePhase::Valid
                    || (overwrite
                        && app.get_shortcut_capture_phase() == ShortcutCapturePhase::Conflict))
                && let Some(action) = ShortcutAction::from_id(app.get_shortcut_capture_action())
                && let Ok(mut shortcuts) = shortcut_registry.lock()
                && let Ok(mut edit) = shortcut_edit.lock()
                && if overwrite {
                    edit.overwrite(&mut shortcuts, action)
                } else {
                    edit.save(&mut shortcuts, action)
                }
            {
                set_command_palette_items(
                    &command_items,
                    app.get_command_query().as_str(),
                    &shortcuts,
                );
                persist_shortcuts(&app, &settings, &shortcuts);
                cancel_shortcut_capture(&app);
                app.set_shortcut_saved_action(action as i32);

                let feedback_app = app.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_millis(1600), move || {
                    if let Some(app) = feedback_app.upgrade()
                        && app.get_shortcut_saved_action() == action as i32
                    {
                        app.set_shortcut_saved_action(-1);
                    }
                });
            }
        });
    }

    {
        let tx = tx.clone();

        app.on_seek_relative(move |value| {
            let _ = tx.send(Command::SeekRelative(value as f64));
        });
    }

    {
        let tx = tx.clone();

        app.on_step_frame(move |direction| {
            let direction = if direction < 0 {
                FrameStepDirection::Backward
            } else {
                FrameStepDirection::Forward
            };
            let _ = tx.send(Command::StepFrame(direction));
        });
    }

    {
        let tx = tx.clone();

        app.on_adjust_volume(move |value| {
            let _ = tx.send(Command::AdjustVolume(value as f64));
        });
    }

    {
        let tx = tx.clone();

        app.on_toggle_mute(move || {
            let _ = tx.send(Command::ToggleMute);
        });
    }

    {
        let tx = tx.clone();

        app.on_play_playlist_index(move |index| {
            if let Ok(index) = usize::try_from(index) {
                let _ = tx.send(Command::PlayIndex(index));
            }
        });
    }

    {
        let tx = tx.clone();

        app.on_remove_playlist_index(move |index| {
            if let Ok(index) = usize::try_from(index) {
                let _ = tx.send(Command::RemoveIndex(index));
            }
        });
    }

    #[cfg(target_os = "windows")]
    let (native_init_error_tx, native_init_error_rx) = crossbeam_channel::bounded(1);
    #[cfg(target_os = "windows")]
    {
        // The native child HWND is only used when integrated OpenGL rendering
        // is unavailable. Register its event bridge once, but keep it dormant
        // for the normal integrated path so idle windows do not wake every 50ms.
        let timer_app = app.as_weak();
        let timer_native_fullscreen = native_fullscreen.clone();
        video_resize_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(50),
            move || {
                let Some(app) = timer_app.upgrade() else {
                    return;
                };
                if VideoHost::take_fullscreen_request() {
                    toggle_native_fullscreen(&app, &timer_native_fullscreen);
                }
                if VideoHost::take_pointer_activity() {
                    app.invoke_reveal_controls();
                }
                if VideoHost::take_primary_click() {
                    if app.get_info_drawer_visible() {
                        app.invoke_toggle_info();
                    }
                    if app.get_playlist_drawer_visible() {
                        app.invoke_toggle_playlist();
                    }
                    if app.get_settings_drawer_visible() {
                        app.invoke_toggle_settings();
                    }
                }
            },
        );
        video_resize_timer.stop();

        let app_weak = app.as_weak();
        let tx_handle = tx.clone();
        let notifier_video_host = video_host.clone();
        let notifier_render_source = render_source.clone();
        let notifier_renderer_requested = Arc::clone(&video_renderer_requested);
        let notifier_video_timer = Rc::clone(&video_resize_timer);
        let mut gl_renderer = None::<MpvOpenGlRenderer>;
        let mut render_api_failed = false;

        app.window()
            .set_rendering_notifier(move |state, graphics_api| {
                let Some(app) = app_weak.upgrade() else {
                    return;
                };
                if matches!(state, slint::RenderingState::RenderingSetup) {
                    // Slint 1.17/winit pre-renders the first frame before
                    // set_visible(true). The HWND exists here, but has not yet
                    // been shown. A queued event-loop callback is too late.
                    // Run independently of video initialization and repeat if
                    // Slint recreates the rendering surface/window.
                    if let Err(error) =
                        super::native_frame::prepare(app.window(), app.window().is_fullscreen())
                    {
                        // Preserve the first failure without blocking if setup
                        // is repeated before the event loop finishes exiting.
                        let _ = native_init_error_tx.try_send(error);
                        let _ = slint::quit_event_loop();
                    }
                    return;
                }
                let Ok(mut video_host) = notifier_video_host.try_borrow_mut() else {
                    return;
                };

                match state {
                    slint::RenderingState::BeforeRendering => {
                        if gl_renderer.is_none()
                            && video_host.is_none()
                            && !render_api_failed
                            && notifier_renderer_requested.load(Ordering::Acquire)
                        {
                            let source = notifier_render_source
                                .lock()
                                .ok()
                                .and_then(|source| source.clone());

                            if let Some(source) = source {
                                let renderer = match graphics_api {
                                    slint::GraphicsAPI::NativeOpenGL { get_proc_address } => {
                                        let wake_app = app.as_weak();
                                        let native_redraw_hwnd =
                                            native_window_handle(app.window()).map(|hwnd| hwnd as usize);
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
                                        // SAFETY: Slint invokes BeforeRendering
                                        // with this window's OpenGL context
                                        // current. The renderer stays inside
                                        // this notifier and is destroyed during
                                        // RenderingTeardown with that same
                                        // context current.
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
                                };

                                match renderer {
                                    Ok(renderer) => {
                                        notifier_renderer_requested.store(false, Ordering::Release);
                                        notifier_video_timer.stop();
                                        gl_renderer = Some(renderer);
                                        app.set_integrated_video_renderer(true);
                                        let _ = tx_handle.send(Command::RenderApiReady);
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "OpenGL video renderer unavailable, using HWND fallback: {error:#}"
                                        );
                                        render_api_failed = true;
                                        notifier_renderer_requested.store(false, Ordering::Release);
                                        if let Err(error) = activate_hwnd_video_fallback(
                                            &mut video_host,
                                            &app,
                                            &tx_handle,
                                        ) {
                                            app.set_error_message(error.to_string().into());
                                            app.set_error_visible(true);
                                        } else {
                                            notifier_video_timer.restart();
                                        }
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
                            let render_result = unsafe {
                                renderer.render((size.width, size.height))
                            };
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
                        if let Some(renderer) = gl_renderer.as_ref() {
                            // SAFETY: AfterRendering still has the window's
                            // original OpenGL context current.
                            unsafe { renderer.report_swap() };
                        } else if let Some(host) = video_host.as_mut()
                            && let Err(error) = sync_video_host(host, &app)
                        {
                            app.set_error_message(error.to_string().into());
                            app.set_error_visible(true);
                        }
                    }
                    slint::RenderingState::RenderingTeardown => {
                        notifier_video_timer.stop();
                        gl_renderer = None;
                        *video_host = None;
                    }
                    _ => {}
                }
            })
            .map_err(|error| anyhow::anyhow!("Не удалось подключить подготовку окна: {error}"))?;
    }

    let weak = app.as_weak();
    #[cfg(target_os = "windows")]
    let event_render_source = render_source.clone();
    #[cfg(target_os = "windows")]
    let event_renderer_requested = Arc::clone(&video_renderer_requested);
    #[cfg(target_os = "windows")]
    let event_player_tx = tx.clone();

    let player_bridge = super::event_delivery::spawn(
        rx,
        |previous, next| matches!((previous, next), (Event::TimePos(_), Event::TimePos(_))),
        move |batch, completion| {
            let weak = weak.clone();
            let current_duration = current_duration.clone();
            #[cfg(target_os = "windows")]
            let render_source = event_render_source.clone();
            #[cfg(target_os = "windows")]
            let renderer_requested = Arc::clone(&event_renderer_requested);
            #[cfg(target_os = "windows")]
            let player_tx = event_player_tx.clone();

            slint::invoke_from_event_loop(move || {
                let _completion = completion;
                let Some(app) = weak.upgrade() else {
                    return;
                };

                for event in batch {
                    match event {
                        #[cfg(target_os = "windows")]
                        Event::RenderSource(source) => {
                            if let Ok(mut target) = render_source.lock() {
                                *target = Some(source);
                            }
                            app.window().request_redraw();
                        }

                        #[cfg(not(target_os = "windows"))]
                        Event::RenderSource(_) => {}

                        Event::VideoRendererRequired => {
                            #[cfg(target_os = "windows")]
                            {
                                if app.get_integrated_video_renderer() {
                                    // The OpenGL renderer is kept between
                                    // playlist items. Re-enable mpv's video
                                    // track explicitly instead of waiting for
                                    // renderer creation that already happened.
                                    let _ = player_tx.send(Command::RenderApiReady);
                                } else {
                                    renderer_requested.store(true, Ordering::Release);
                                    app.window().request_redraw();
                                }
                            }
                        }

                        Event::LoadStarted(name) => {
                            app.set_resume_prompt_visible(false);
                            let preserve_media_surface = app.get_media_presented();
                            app.set_audio_display_name(media_name_without_extension(&name).into());
                            app.set_file_name(name.into());
                            app.set_track_title("".into());
                            app.set_track_artist("".into());
                            app.set_album_title("".into());
                            app.set_album_art(Image::default());
                            app.set_album_art_available(false);
                            app.set_audio_tracks(ModelRc::new(VecModel::from(Vec::<
                                AudioTrackEntry,
                            >::new(
                            ))));
                            app.set_subtitle_tracks(ModelRc::new(VecModel::from(Vec::<
                                SubtitleTrackEntry,
                            >::new(
                            ))));
                            app.set_subtitles_enabled(false);
                            app.set_audio_delay(0.0);
                            app.set_subtitle_delay(0.0);
                            app.set_chapters(ModelRc::new(VecModel::from(
                                Vec::<ChapterEntry>::new(),
                            )));
                            app.set_current_chapter_index(-1);
                            app.set_aspect_ratio_index(AspectRatio::Original.ui_index());
                            app.set_waiting_for_video_surface(false);
                            app.set_media_transitioning(preserve_media_surface);
                            app.set_is_playing(false);
                            if !preserve_media_surface {
                                app.set_audio_media(false);
                                app.set_media_loaded(false);
                                app.set_player_phase(PlayerPhase::Loading);
                            }
                            app.set_error_visible(false);
                            app.set_error_message("".into());
                        }

                        Event::FileLoaded { name, path } => {
                            app.set_audio_display_name(media_name_without_extension(&name).into());
                            app.set_file_name(name.clone().into());
                            let path = path.to_string_lossy().into_owned();
                            app.set_current_media_path(path.clone().into());
                            if !app.get_conversion_running() && !app.get_converter_visible() {
                                app.set_conversion_source_path(path.into());
                                app.set_conversion_source_name(name.into());
                                app.set_conversion_source_duration("—".into());
                                app.set_conversion_source_duration_seconds(0.0);
                                app.set_conversion_source_format("—".into());
                                app.set_conversion_source_size("—".into());
                            }
                            if !app.get_conversion_running() {
                                app.set_conversion_output_path("".into());
                                app.set_conversion_progress(0.0);
                                app.set_conversion_status("".into());
                                app.set_conversion_size_status("".into());
                            }
                            app.set_current_time("00:00".into());
                            app.set_total_time("00:00".into());
                            app.set_duration_seconds(0.0);
                            app.set_progress(0.0);
                            app.set_container_format("—".into());
                            app.set_file_size_label("—".into());
                            app.set_media_duration_label("—".into());
                            app.set_media_bitrate_label("—".into());
                            app.set_track_summary("—".into());
                            app.set_error_visible(false);
                            if let Ok(mut duration) = current_duration.lock() {
                                *duration = 0.0;
                            }
                        }

                        Event::MediaAvailable(available) => {
                            app.set_media_loaded(available);
                            app.set_media_transitioning(
                                available && app.get_waiting_for_video_surface(),
                            );
                            if !available {
                                app.set_waiting_for_video_surface(false);
                                app.set_is_playing(false);
                            }
                        }

                        Event::PlaybackState(state) => {
                            app.set_is_playing(state && app.get_media_loaded());
                            if app.get_media_loaded() {
                                app.set_player_phase(if state {
                                    PlayerPhase::Playing
                                } else {
                                    PlayerPhase::Paused
                                });
                            }
                        }

                        Event::PlaybackStopped => {
                            app.set_resume_prompt_visible(false);
                            app.set_media_transitioning(false);
                            app.set_player_phase(PlayerPhase::Empty);
                        }

                        Event::PlaybackFinished => {
                            app.set_resume_prompt_visible(false);
                            app.set_media_transitioning(false);
                            app.set_player_phase(PlayerPhase::Finished);
                        }

                        Event::PlaybackFailed(message) => {
                            app.set_resume_prompt_visible(false);
                            app.set_media_transitioning(false);
                            app.set_waiting_for_video_surface(false);
                            app.set_media_loaded(false);
                            app.set_is_playing(false);
                            app.set_player_phase(PlayerPhase::Error);
                            app.set_error_message(message.into());
                            app.set_error_visible(false);
                        }

                        Event::Volume(v) => {
                            app.set_volume((v / 100.0) as f32);
                            app.invoke_schedule_playback_defaults_save();
                        }

                        Event::Muted(muted) => {
                            app.set_muted(muted);
                        }

                        Event::Speed(v) => {
                            app.set_speed(v as f32);
                            app.invoke_schedule_playback_defaults_save();
                        }

                        Event::AudioDelay(value) => {
                            app.set_audio_delay(value as f32);
                        }

                        Event::SubtitleDelay(value) => {
                            app.set_subtitle_delay(value as f32);
                        }

                        Event::TimePos(position) => {
                            // Playback and the session store keep advancing while
                            // minimized, but invisible timeline updates would still
                            // invalidate Slint and wake the OpenGL renderer. The next
                            // mpv sample refreshes the UI after the window is restored.
                            if !app.get_window_minimized() {
                                app.set_current_time(format_time(position).into());
                                let duration = current_duration
                                    .lock()
                                    .map(|duration| *duration)
                                    .unwrap_or_default();
                                if duration > 0.0 {
                                    app.set_progress((position / duration) as f32);
                                }
                            }
                        }

                        Event::Duration(duration) => {
                            if let Ok(mut current_duration) = current_duration.lock() {
                                *current_duration = duration;
                            }
                            app.set_total_time(format_time(duration).into());
                            app.set_duration_seconds(duration as f32);
                            app.set_media_duration_label(
                                if duration.is_finite() && duration > 0.0 {
                                    format_time(duration).into()
                                } else {
                                    "—".into()
                                },
                            );
                        }

                        Event::Playlist {
                            items,
                            current_index,
                        } => {
                            let items = items
                                .into_iter()
                                .map(SharedString::from)
                                .collect::<Vec<_>>();
                            let next_file = items.get(current_index + 1).cloned();
                            app.set_playlist_items(ModelRc::new(VecModel::from(items)));
                            app.set_playlist_current_index(
                                i32::try_from(current_index).unwrap_or(i32::MAX),
                            );
                            app.set_playlist_focus_revision(
                                app.get_playlist_focus_revision().wrapping_add(1),
                            );
                            if let Some(next_file) = next_file {
                                app.set_next_file(next_file);
                                app.set_next_card_visible(true);
                            } else {
                                app.set_next_card_visible(false);
                            }
                        }

                        Event::RecentFiles(files) => {
                            if !files.is_empty() {
                                app.set_playback_data_cleared(false);
                            }
                            let files = files
                                .into_iter()
                                .map(|file| RecentFileEntry {
                                    name: file.name.into(),
                                    path: file.path.to_string_lossy().into_owned().into(),
                                    location: file.location.into(),
                                })
                                .collect::<Vec<_>>();
                            app.set_recent_files(ModelRc::new(VecModel::from(files)));
                        }

                        Event::ResumeAvailable(position) => {
                            app.set_resume_position_label(format_time(position).into());
                            app.set_resume_prompt_visible(true);
                        }

                        Event::PlaybackDataCleared => {
                            app.set_resume_prompt_visible(false);
                            app.set_clear_playback_data_prompt_visible(false);
                            app.set_playback_data_cleared(true);
                            app.set_notice_message("История очищена".into());
                            app.set_notice_visible(true);
                            let notice_app = app.as_weak();
                            slint::Timer::single_shot(
                                std::time::Duration::from_millis(2600),
                                move || {
                                    if let Some(app) = notice_app.upgrade() {
                                        app.set_notice_visible(false);
                                    }
                                },
                            );
                        }

                        Event::MediaInfo {
                            width,
                            height,
                            fps,
                            codec,
                            audio_codec,
                            container,
                            file_size_bytes,
                            duration_seconds,
                            average_bitrate_bps,
                            video_track_count,
                            audio_track_count,
                            subtitle_track_count,
                        } => {
                            let audio_media =
                                is_audio_only_media(video_track_count, &codec, &audio_codec);
                            let wait_for_video = app.get_media_transitioning()
                                && app.get_audio_media()
                                && !audio_media;
                            app.set_waiting_for_video_surface(wait_for_video);
                            if !wait_for_video {
                                app.set_audio_media(audio_media);
                            }
                            app.set_resolution(if width > 0 && height > 0 {
                                format!("{width}×{height}").into()
                            } else {
                                "—".into()
                            });
                            app.set_fps(if fps.is_finite() && fps > 0.0 {
                                format!("{fps:.2} FPS").into()
                            } else {
                                "—".into()
                            });
                            app.set_codec(format_codec(&codec).into());
                            app.set_audio_codec(format_codec(&audio_codec).into());
                            if !app.get_conversion_running() && !app.get_converter_visible() {
                                super::conversion_controller::apply_presets(
                                    &app,
                                    audio_media,
                                    &audio_codec,
                                );
                                app.set_conversion_audio_media(audio_media);
                                app.set_conversion_source_duration_seconds(
                                    duration_seconds.max(0.0) as f32,
                                );
                                app.set_conversion_source_duration(
                                    if duration_seconds.is_finite() && duration_seconds > 0.0 {
                                        format_time(duration_seconds).into()
                                    } else {
                                        "—".into()
                                    },
                                );
                                app.set_conversion_source_format(if audio_media {
                                    format_codec(&audio_codec).into()
                                } else {
                                    format_container(&container).into()
                                });
                                app.set_conversion_source_size(
                                    format_precise_file_size(file_size_bytes).into(),
                                );
                            }
                            app.set_container_format(format_container(&container).into());
                            app.set_file_size_label(format_file_size(file_size_bytes).into());
                            app.set_media_duration_label(
                                if duration_seconds.is_finite() && duration_seconds > 0.0 {
                                    format_time(duration_seconds).into()
                                } else {
                                    "—".into()
                                },
                            );
                            app.set_media_bitrate_label(format_bitrate(average_bitrate_bps).into());
                            app.set_track_summary(
                                format_track_summary(
                                    video_track_count,
                                    audio_track_count,
                                    subtitle_track_count,
                                )
                                .into(),
                            );
                        }

                        Event::AudioMetadata {
                            title,
                            artist,
                            album,
                            artwork_rgba,
                            artwork_width,
                            artwork_height,
                        } => {
                            app.set_track_title(title.into());
                            app.set_track_artist(artist.into());
                            app.set_album_title(album.into());

                            let expected_len = usize::try_from(artwork_width)
                                .ok()
                                .and_then(|width| {
                                    usize::try_from(artwork_height)
                                        .ok()
                                        .and_then(|height| width.checked_mul(height))
                                })
                                .and_then(|pixels| pixels.checked_mul(4));
                            if expected_len == Some(artwork_rgba.len()) && !artwork_rgba.is_empty()
                            {
                                let buffer = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
                                    &artwork_rgba,
                                    artwork_width,
                                    artwork_height,
                                );
                                app.set_album_art(Image::from_rgba8(buffer));
                                app.set_album_art_available(true);
                            } else {
                                app.set_album_art(Image::default());
                                app.set_album_art_available(false);
                            }
                        }

                        Event::AudioTracks(tracks) => {
                            let tracks = tracks
                                .into_iter()
                                .filter_map(|track| {
                                    Some(AudioTrackEntry {
                                        id: i32::try_from(track.id).ok()?,
                                        label: track.label.into(),
                                        selected: track.selected,
                                    })
                                })
                                .collect::<Vec<_>>();
                            app.set_audio_tracks(ModelRc::new(VecModel::from(tracks)));
                        }

                        Event::SubtitleTracks(tracks) => {
                            let tracks = tracks
                                .into_iter()
                                .filter_map(|track| {
                                    Some(SubtitleTrackEntry {
                                        id: i32::try_from(track.id).ok()?,
                                        label: track.label.into(),
                                        selected: track.selected,
                                    })
                                })
                                .collect::<Vec<_>>();
                            app.set_subtitles_enabled(tracks.iter().any(|track| track.selected));
                            app.set_subtitle_tracks(ModelRc::new(VecModel::from(tracks)));
                        }

                        Event::Chapters {
                            items,
                            current_index,
                        } => {
                            let chapters = items
                                .into_iter()
                                .filter_map(|chapter| {
                                    Some(ChapterEntry {
                                        index: i32::try_from(chapter.index).ok()?,
                                        label: chapter.label.into(),
                                    })
                                })
                                .collect::<Vec<_>>();
                            app.set_chapters(ModelRc::new(VecModel::from(chapters)));
                            app.set_current_chapter_index(
                                i32::try_from(current_index).unwrap_or(-1),
                            );
                        }

                        Event::ChapterChanged(index) => {
                            app.set_current_chapter_index(i32::try_from(index).unwrap_or(-1));
                        }

                        Event::AspectRatioChanged(aspect_ratio) => {
                            app.set_aspect_ratio_index(aspect_ratio.ui_index());
                        }

                        Event::HardwareDecoding { enabled, decoder } => {
                            app.set_hardware_decoding_enabled(enabled);
                            app.set_hardware_decoder(decoder.into());
                        }

                        Event::EqualizerChanged {
                            enabled,
                            preset,
                            preamp_db,
                            band_gains_db,
                        } => {
                            app.set_equalizer_enabled(enabled);
                            app.set_equalizer_preset_index(preset.ui_index());
                            app.set_equalizer_preamp_db(preamp_db as f32);
                            let bands = ["60 Гц", "230 Гц", "910 Гц", "3,6 кГц", "14 кГц"]
                                .into_iter()
                                .zip(band_gains_db)
                                .map(|(label, gain_db)| EqualizerBandEntry {
                                    label: label.into(),
                                    gain_db: gain_db as f32,
                                })
                                .collect::<Vec<_>>();
                            app.set_equalizer_bands(ModelRc::new(VecModel::from(bands)));
                        }

                        Event::VideoOutputReady => {
                            if app.get_waiting_for_video_surface() {
                                app.set_audio_media(false);
                                app.set_waiting_for_video_surface(false);
                                app.set_media_transitioning(false);
                            }
                        }

                        Event::LoopEnabled(enabled) => {
                            app.set_loop_enabled(enabled);
                        }

                        Event::FolderLoopEnabled(enabled) => {
                            app.set_folder_loop_enabled(enabled);
                        }

                        Event::ShuffleEnabled(enabled) => {
                            app.set_shuffle_enabled(enabled);
                        }

                        Event::Error(message) => {
                            app.set_error_message(message.into());
                            app.set_error_visible(true);
                        }
                    }
                }
            })
            .is_ok()
        },
    )?;

    let converter_bridge = super::conversion_controller::spawn_event_bridge(&app, converter_rx)?;

    // `AppWindow::new()` does not guarantee that the winit window already
    // exists. Show it before restoring native coordinates; the event loop has
    // not started yet, so the default bounds are not painted as an extra frame.
    #[cfg(target_os = "windows")]
    let initial_window_placement = settings
        .lock()
        .ok()
        .and_then(|settings| settings.window_placement());
    #[cfg(target_os = "windows")]
    if let Some(placement) = initial_window_placement {
        app.window().set_size(slint::LogicalSize::new(
            placement.width as f32,
            placement.height as f32,
        ));
        // Before the event loop starts, Slint stores this in winit's future
        // WindowAttributes. Calling through winit here is too early because no
        // native window exists yet.
        app.window()
            .set_position(slint::PhysicalPosition::new(placement.x, placement.y));
    }
    app.show()?;
    #[cfg(target_os = "windows")]
    if let Some(placement) = initial_window_placement {
        let app = app.as_weak();
        slint::invoke_from_event_loop(move || {
            if let Some(app) = app.upgrade() {
                // Clamp the saved position after the native frame is ready.
                restore_window_placement(app.window(), placement);
            }
        })?;
    }
    let run_result = slint::run_event_loop();
    let hide_result = app.hide();
    drop(player_bridge);
    drop(converter_bridge);
    drop(command_error_bridge);
    #[cfg(target_os = "windows")]
    if let Some(hwnd) = native_window_handle(app.window()) {
        system_notification::clear(hwnd as usize);
    }
    if let Ok(mut settings) = settings.lock()
        && let Err(error) = settings.save_if_dirty()
    {
        eprintln!("Не удалось сохранить настройки при выходе: {error}");
    }
    run_result?;
    hide_result?;
    #[cfg(target_os = "windows")]
    if let Ok(error) = native_init_error_rx.try_recv() {
        return Err(error.context("Не удалось настроить рамку созданного окна"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        PALETTE_COMMANDS, ShortcutAction, ShortcutBinding, ShortcutEdit, ShortcutRegistry,
        command_for_dropped_path, command_palette_items, command_palette_items_with_registry,
        video_surface_should_render,
    };
    use crate::ipc::Command;
    use crate::settings::ShortcutPreference;
    use slint::winit_030::winit::keyboard::{KeyCode, ModifiersState};

    #[test]
    fn integrated_video_surface_never_paints_under_audio() {
        assert!(video_surface_should_render(true, false));
        assert!(!video_surface_should_render(true, true));
        assert!(!video_surface_should_render(false, false));
        assert!(!video_surface_should_render(false, true));
    }

    #[test]
    fn rejects_unsupported_dropped_paths() {
        assert!(command_for_dropped_path(Path::new("notes.txt")).is_err());
        assert!(command_for_dropped_path(Path::new("missing.mp4")).is_err());
    }

    #[test]
    fn command_palette_returns_all_commands_for_an_empty_query() {
        assert_eq!(command_palette_items("").len(), PALETTE_COMMANDS.len());
        assert_eq!(command_palette_items("   ").len(), PALETTE_COMMANDS.len());
    }

    #[test]
    fn reset_hint_keeps_factory_binding_after_customization() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        let entries = command_palette_items_with_registry("mute", &shortcuts);
        assert_eq!(entries[0].shortcut, "Ctrl+B");
        assert_eq!(entries[0].default_shortcut, "M");
        let entries = command_palette_items_with_registry("", &shortcuts);
        let open = entries
            .iter()
            .find(|entry| entry.action == ShortcutAction::OpenFile as i32)
            .unwrap();
        assert_eq!(open.default_shortcut, "Ctrl+O / O");
        let settings = entries
            .iter()
            .find(|entry| entry.action == ShortcutAction::ToggleSettings as i32)
            .unwrap();
        assert_eq!(settings.default_shortcut, "не назначено");
    }

    #[test]
    fn command_palette_searches_titles_descriptions_and_aliases() {
        let mute = command_palette_items("mute");
        assert_eq!(mute.len(), 1);
        assert_eq!(mute[0].action, 5);

        let folder = command_palette_items("папку плейлист");
        assert_eq!(folder.len(), 1);
        assert_eq!(folder[0].action, 1);

        assert!(command_palette_items("несуществующая-команда").is_empty());
    }

    #[test]
    fn command_palette_marks_media_dependent_actions() {
        let open_file = command_palette_items("открыть файл");
        assert!(!open_file[0].requires_media);

        let seek = command_palette_items("перемотать назад");
        assert!(seek[0].requires_media);

        let playlist = command_palette_items("показать плейлист");
        assert!(playlist[0].requires_media);
    }

    #[test]
    fn shortcuts_are_resolved_from_physical_keys_and_exact_modifiers() {
        let shortcuts = ShortcutRegistry::default();
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::empty())),
            Some(ShortcutAction::PlayPause)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::CONTROL)),
            Some(ShortcutAction::ToggleCommandPalette)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(
                KeyCode::KeyO,
                ModifiersState::CONTROL | ModifiersState::SHIFT
            )),
            Some(ShortcutAction::OpenFolder)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyK, ModifiersState::ALT)),
            None
        );
    }

    #[test]
    fn rebind_replaces_aliases_for_an_action() {
        let mut shortcuts = ShortcutRegistry::default();
        let replacement = ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL);

        shortcuts
            .rebind(ShortcutAction::ToggleMute, replacement)
            .expect("unused shortcut must be accepted");

        assert_eq!(
            shortcuts.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            None
        );
        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleMute), "Ctrl+B");
    }

    #[test]
    fn rebind_rejects_conflicts_without_changing_existing_bindings() {
        let mut shortcuts = ShortcutRegistry::default();
        let play_pause = ShortcutBinding::new(KeyCode::Space, ModifiersState::empty());

        let conflict = shortcuts
            .rebind(ShortcutAction::ToggleMute, play_pause)
            .expect_err("a shortcut assigned to another action must be rejected");

        assert_eq!(conflict.action, ShortcutAction::PlayPause);
        assert_eq!(conflict.binding, play_pause);
        assert_eq!(
            shortcuts.action_for(play_pause),
            Some(ShortcutAction::PlayPause)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn shortcut_preview_requires_save_and_can_be_discarded() {
        let mut shortcuts = ShortcutRegistry::default();
        let mut edit = ShortcutEdit::default();
        let replacement = ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL);
        edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![replacement])
            .unwrap();
        assert_eq!(shortcuts.action_for(replacement), None);
        assert!(shortcuts.preferences().is_empty());
        // Cancelling or opening another editor discards the pending assignment.
        edit.discard();
        assert!(!edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleMute), "M");
        edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![replacement])
            .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        let restored = ShortcutRegistry::from_preferences(&shortcuts.preferences());
        assert_eq!(
            restored.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn conflicting_preview_cannot_save_an_earlier_valid_candidate() {
        let mut shortcuts = ShortcutRegistry::default();
        let mut edit = ShortcutEdit::default();
        edit.stage(
            &shortcuts,
            ShortcutAction::ToggleMute,
            vec![ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL)],
        )
        .unwrap();
        let conflict = edit
            .stage(
                &shortcuts,
                ShortcutAction::ToggleMute,
                vec![ShortcutBinding::new(
                    KeyCode::Space,
                    ModifiersState::empty(),
                )],
            )
            .unwrap_err();
        assert_eq!(conflict.action, ShortcutAction::PlayPause);
        assert!(!edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        assert!(shortcuts.preferences().is_empty());

        // A rejected candidate can be replaced in this edit without restarting capture.
        let replacement = ShortcutBinding::new(KeyCode::KeyD, ModifiersState::CONTROL);
        edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![replacement])
            .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::ToggleMute));
        assert_eq!(
            shortcuts.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(
                KeyCode::Space,
                ModifiersState::empty()
            )),
            Some(ShortcutAction::PlayPause)
        );
    }

    #[test]
    fn explicit_transfer_disables_previous_owner_and_survives_reload() {
        // Exercise both orders in the settings snapshot, including a donor with aliases.
        for (recipient, donor, code) in [
            (
                ShortcutAction::PlayPause,
                ShortcutAction::ToggleMute,
                KeyCode::KeyM,
            ),
            (
                ShortcutAction::ToggleMute,
                ShortcutAction::PlayPause,
                KeyCode::Space,
            ),
        ] {
            let mut shortcuts = ShortcutRegistry::default();
            let mut edit = ShortcutEdit::default();
            let binding = ShortcutBinding::new(code, ModifiersState::empty());
            assert!(edit.stage(&shortcuts, recipient, vec![binding]).is_err());
            assert_eq!(shortcuts.action_for(binding), Some(donor));
            assert!(shortcuts.preferences().is_empty());
            assert!(edit.overwrite(&mut shortcuts, recipient));
            assert_eq!(shortcuts.action_for(binding), Some(recipient));
            assert_eq!(shortcuts.display_for(donor), "—");
            let mut preferences = shortcuts.preferences();
            for _ in 0..2 {
                let restored = ShortcutRegistry::from_preferences(&preferences);
                assert_eq!(restored.action_for(binding), Some(recipient));
                assert_eq!(restored.display_for(donor), "—");
                for alias in donor.default_bindings() {
                    assert_ne!(restored.action_for(alias), Some(donor));
                }
                preferences.reverse();
            }
        }
    }

    #[test]
    fn cancelled_or_different_command_cannot_transfer_a_staged_conflict() {
        let mut shortcuts = ShortcutRegistry::default();
        let mut edit = ShortcutEdit::default();
        let binding = ShortcutBinding::new(KeyCode::Space, ModifiersState::empty());
        assert!(
            edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![binding])
                .is_err()
        );
        edit.discard();
        assert!(!edit.overwrite(&mut shortcuts, ShortcutAction::ToggleMute));
        assert!(
            edit.stage(&shortcuts, ShortcutAction::ToggleMute, vec![binding])
                .is_err()
        );
        assert!(!edit.overwrite(&mut shortcuts, ShortcutAction::ToggleSettings));
        assert!(shortcuts.preferences().is_empty());
    }

    #[test]
    fn transferred_default_aliases_can_be_restored_from_multiple_owners() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts.transfer_bindings(
            ShortcutAction::ToggleMute,
            vec![ShortcutBinding::new(
                KeyCode::Space,
                ModifiersState::empty(),
            )],
        );
        shortcuts.transfer_bindings(
            ShortcutAction::ToggleSettings,
            vec![ShortcutBinding::new(KeyCode::KeyK, ModifiersState::empty())],
        );
        let defaults = ShortcutAction::PlayPause.default_bindings();
        let notice = shortcuts.transfer_notice(ShortcutAction::PlayPause, &defaults);
        assert!(notice.contains(ShortcutAction::ToggleMute.title()));
        assert!(notice.contains(ShortcutAction::ToggleSettings.title()));
        let mut edit = ShortcutEdit::default();
        assert!(
            edit.stage(&shortcuts, ShortcutAction::PlayPause, defaults.clone())
                .is_err()
        );
        assert!(edit.overwrite(&mut shortcuts, ShortcutAction::PlayPause));
        let restored = ShortcutRegistry::from_preferences(&shortcuts.preferences());
        for binding in defaults {
            assert_eq!(
                restored.action_for(binding),
                Some(ShortcutAction::PlayPause)
            );
        }
        assert_eq!(restored.display_for(ShortcutAction::ToggleMute), "—");
        assert_eq!(restored.display_for(ShortcutAction::ToggleSettings), "—");
    }

    #[test]
    fn swapped_assignments_reload_without_default_order_conflicts() {
        let mut shortcuts = ShortcutRegistry::default();
        let mute = ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty());
        let play = ShortcutBinding::new(KeyCode::Space, ModifiersState::empty());
        shortcuts.transfer_bindings(ShortcutAction::PlayPause, vec![mute]);
        shortcuts.rebind(ShortcutAction::ToggleMute, play).unwrap();
        let restored = ShortcutRegistry::from_preferences(&shortcuts.preferences());
        assert_eq!(restored.action_for(mute), Some(ShortcutAction::PlayPause));
        assert_eq!(restored.action_for(play), Some(ShortcutAction::ToggleMute));
    }

    #[test]
    fn restoring_one_command_preserves_other_changes_and_all_default_aliases() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::PlayPause,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::KeyD, ModifiersState::CONTROL),
            )
            .unwrap();
        let mut edit = ShortcutEdit::default();
        edit.stage(
            &shortcuts,
            ShortcutAction::PlayPause,
            ShortcutAction::PlayPause.default_bindings(),
        )
        .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::PlayPause));
        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleMute), "Ctrl+D");
        for binding in ShortcutAction::PlayPause.default_bindings() {
            assert_eq!(
                shortcuts.action_for(binding),
                Some(ShortcutAction::PlayPause)
            );
        }
        // A default reassigned to another action must not be stolen by reset.
        shortcuts
            .rebind(
                ShortcutAction::PlayPause,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::Space, ModifiersState::empty()),
            )
            .unwrap();
        assert!(
            edit.stage(
                &shortcuts,
                ShortcutAction::PlayPause,
                ShortcutAction::PlayPause.default_bindings()
            )
            .is_err()
        );
    }

    #[test]
    fn restoring_unassigned_default_removes_the_persisted_override() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::ToggleSettings,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .unwrap();
        let mut edit = ShortcutEdit::default();
        edit.stage(
            &shortcuts,
            ShortcutAction::ToggleSettings,
            ShortcutAction::ToggleSettings.default_bindings(),
        )
        .unwrap();
        assert!(edit.save(&mut shortcuts, ShortcutAction::ToggleSettings));
        assert!(shortcuts.preferences().is_empty());
        assert_eq!(
            ShortcutRegistry::from_preferences(&shortcuts.preferences())
                .display_for(ShortcutAction::ToggleSettings),
            "—"
        );
    }

    #[test]
    fn reset_restores_default_shortcuts() {
        let mut shortcuts = ShortcutRegistry::default();
        shortcuts
            .rebind(
                ShortcutAction::ToggleMute,
                ShortcutBinding::new(KeyCode::KeyB, ModifiersState::CONTROL),
            )
            .expect("unused shortcut must be accepted");

        shortcuts.reset();

        assert_eq!(
            shortcuts.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
    }

    #[test]
    fn shortcut_overrides_round_trip_as_physical_keys() {
        let mut shortcuts = ShortcutRegistry::default();
        let replacement = ShortcutBinding::new(
            KeyCode::KeyB,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        );
        shortcuts
            .rebind(ShortcutAction::ToggleMute, replacement)
            .expect("unused shortcut must be accepted");

        let preferences = shortcuts.preferences();
        assert_eq!(preferences.len(), 1);
        let restored = ShortcutRegistry::from_preferences(&preferences);

        assert_eq!(
            restored.action_for(replacement),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            restored.display_for(ShortcutAction::ToggleMute),
            "Ctrl+Shift+B"
        );
    }

    #[test]
    fn previously_unassigned_commands_can_receive_shortcuts() {
        let mut shortcuts = ShortcutRegistry::default();
        let binding = ShortcutBinding::new(
            KeyCode::Comma,
            ModifiersState::CONTROL | ModifiersState::SHIFT,
        );

        assert_eq!(shortcuts.display_for(ShortcutAction::ToggleSettings), "—");
        shortcuts
            .rebind(ShortcutAction::ToggleSettings, binding)
            .expect("unused shortcut must be accepted");

        assert_eq!(
            shortcuts.action_for(binding),
            Some(ShortcutAction::ToggleSettings)
        );
        assert_eq!(
            shortcuts.display_for(ShortcutAction::ToggleSettings),
            "Ctrl+Shift+,"
        );
    }

    #[test]
    fn invalid_shortcut_preferences_keep_safe_defaults() {
        let restored = ShortcutRegistry::from_preferences(&[
            ShortcutPreference {
                action_id: ShortcutAction::ToggleMute as i32,
                key_code: "not-json".to_owned(),
                modifiers: 0,
            },
            ShortcutPreference {
                action_id: ShortcutAction::PlayPause as i32,
                key_code: serde_json::to_string(&KeyCode::KeyB).expect("key code must serialize"),
                modifiers: u8::MAX,
            },
        ]);

        assert_eq!(
            restored.action_for(ShortcutBinding::new(KeyCode::KeyM, ModifiersState::empty())),
            Some(ShortcutAction::ToggleMute)
        );
        assert_eq!(
            restored.action_for(ShortcutBinding::new(
                KeyCode::Space,
                ModifiersState::empty()
            )),
            Some(ShortcutAction::PlayPause)
        );
    }

    #[test]
    fn only_continuous_shortcuts_repeat_while_held() {
        assert!(ShortcutAction::SeekForwardLong.allows_repeat());
        assert!(ShortcutAction::VolumeUp.allows_repeat());
        assert!(!ShortcutAction::ToggleMute.allows_repeat());
        assert!(!ShortcutAction::ToggleCommandPalette.allows_repeat());
    }

    #[test]
    fn creates_open_file_command_for_supported_media_drop() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time must be after Unix epoch")
            .as_nanos();
        let video_path = std::env::temp_dir().join(format!("oxide-drop-{unique}.mkv"));
        let audio_path = std::env::temp_dir().join(format!("oxide-drop-{unique}.flac"));
        std::fs::write(&video_path, []).expect("temporary video must be created");
        std::fs::write(&audio_path, []).expect("temporary audio must be created");

        assert!(matches!(
            command_for_dropped_path(&video_path),
            Ok(Command::OpenFile(_))
        ));
        assert!(matches!(
            command_for_dropped_path(&audio_path),
            Ok(Command::OpenFile(_))
        ));

        std::fs::remove_file(video_path).expect("temporary video must be removed");
        std::fs::remove_file(audio_path).expect("temporary audio must be removed");
    }
}
