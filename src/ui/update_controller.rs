use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use slint::ComponentHandle;
use oxide::updater::{
    config,
    download::{DownloadEvent, DownloadRequest, DownloadService},
    manifest::VerifiedUpdate,
    service::{CheckEvent, CheckRequest, CheckService},
};

use crate::settings::AppSettings;

use super::ui_logic::AppWindow;

const STATUS_UNCONFIGURED: i32 = 0;
const STATUS_IDLE: i32 = 1;
const STATUS_CHECKING: i32 = 2;
const STATUS_AVAILABLE: i32 = 3;
const STATUS_DOWNLOADING: i32 = 4;
const STATUS_PAUSED: i32 = 5;
const STATUS_READY: i32 = 6;
const STATUS_CURRENT: i32 = 7;
const STATUS_FAILED: i32 = 8;
const STATUS_SKIPPED: i32 = 9;

#[derive(Default)]
struct UpdateState {
    available: Option<VerifiedUpdate>,
    downloaded_path: Option<PathBuf>,
}

pub(super) struct UpdateController {
    _check_service: Arc<CheckService>,
    _download_service: Arc<DownloadService>,
    _check_bridge: super::event_delivery::Bridge,
    _download_bridge: super::event_delivery::Bridge,
    _automatic_check: Option<slint::Timer>,
}

pub(super) fn install(
    app: &AppWindow,
    settings: Arc<Mutex<AppSettings>>,
) -> Result<UpdateController> {
    let request = config::current_request().map_err(anyhow::Error::msg)?;
    let check_service = Arc::new(CheckService::spawn()?);
    let download_service = Arc::new(DownloadService::spawn()?);
    let state = Arc::new(Mutex::new(UpdateState::default()));
    let manual_check = Arc::new(AtomicBool::new(false));

    if request.is_some() {
        app.set_update_status(STATUS_IDLE);
        app.set_update_message("Обновления проверяются автоматически".into());
    } else {
        app.set_update_status(STATUS_UNCONFIGURED);
        app.set_update_message("Проверка доступна в релизной сборке".into());
    }

    let check_events = check_service.event_receiver();
    let check_app = app.as_weak();
    let check_state = state.clone();
    let check_settings = settings.clone();
    let check_manual = manual_check.clone();
    let check_bridge = super::event_delivery::spawn(
        check_events,
        |_, _| false,
        move |events, completion| {
            let app = check_app.clone();
            let state = check_state.clone();
            let settings = check_settings.clone();
            let manual = check_manual.clone();
            slint::invoke_from_event_loop(move || {
                let _completion = completion;
                let Some(app) = app.upgrade() else {
                    return;
                };
                for event in events {
                    apply_check_event(&app, event, &state, &settings, &manual);
                }
            })
            .is_ok()
        },
    )?;

    let download_events = download_service.event_receiver();
    let download_app = app.as_weak();
    let download_state = state.clone();
    let download_bridge = super::event_delivery::spawn(
        download_events,
        |previous, next| {
            matches!(previous, DownloadEvent::Progress { .. })
                && matches!(next, DownloadEvent::Progress { .. })
        },
        move |events, completion| {
            let app = download_app.clone();
            let state = download_state.clone();
            slint::invoke_from_event_loop(move || {
                let _completion = completion;
                let Some(app) = app.upgrade() else {
                    return;
                };
                for event in events {
                    apply_download_event(&app, event, &state);
                }
            })
            .is_ok()
        },
    )?;

    register_callbacks(
        app,
        request.clone(),
        check_service.clone(),
        download_service.clone(),
        state,
        settings,
        manual_check,
    );

    let automatic_check = request.map(|request| {
        let timer = slint::Timer::default();
        let service = check_service.clone();
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(1_500),
            move || {
                if let Err(error) = service.request(request.clone()) {
                    eprintln!("Не удалось запустить автоматическую проверку обновлений: {error}");
                }
            },
        );
        timer
    });

    Ok(UpdateController {
        _check_service: check_service,
        _download_service: download_service,
        _check_bridge: check_bridge,
        _download_bridge: download_bridge,
        _automatic_check: automatic_check,
    })
}

fn register_callbacks(
    app: &AppWindow,
    request: Option<CheckRequest>,
    check_service: Arc<CheckService>,
    download_service: Arc<DownloadService>,
    state: Arc<Mutex<UpdateState>>,
    settings: Arc<Mutex<AppSettings>>,
    manual_check: Arc<AtomicBool>,
) {
    let app_weak = app.as_weak();
    app.on_check_update(move || {
        let Some(request) = request.clone() else {
            return;
        };
        manual_check.store(true, Ordering::Release);
        if let Err(error) = check_service.request(request)
            && let Some(app) = app_weak.upgrade()
        {
            app.set_update_status(STATUS_FAILED);
            app.set_update_message(error.to_string().into());
        }
    });

    let download_state = state.clone();
    let download_app = app.as_weak();
    let download = download_service.clone();
    app.on_download_update(move || {
        let update = download_state
            .lock()
            .ok()
            .and_then(|state| state.available.clone());
        let Some(update) = update else {
            return;
        };
        let directory = std::env::temp_dir().join("Oxide").join("updates");
        if let Err(error) = download.start(DownloadRequest { update, directory })
            && let Some(app) = download_app.upgrade()
        {
            app.set_update_status(STATUS_FAILED);
            app.set_update_message(error.to_string().into());
        }
    });

    let pause = download_service.clone();
    app.on_pause_update(move || {
        let _ = pause.pause();
    });
    let resume = download_service.clone();
    app.on_resume_update(move || {
        let _ = resume.resume();
    });
    let cancel = download_service;
    app.on_cancel_update(move || {
        let _ = cancel.cancel();
    });

    let skip_state = state.clone();
    let skip_settings = settings;
    let skip_app = app.as_weak();
    app.on_skip_update(move || {
        let version = skip_state.lock().ok().and_then(|state| {
            state
                .available
                .as_ref()
                .map(|update| update.version.to_string())
        });
        if let Some(version) = version {
            if let Ok(mut settings) = skip_settings.lock() {
                settings.set_skipped_update_version(Some(version.clone()));
                if let Err(error) = settings.save_if_dirty() {
                    eprintln!("Не удалось сохранить пропущенную версию: {error}");
                }
            }
            if let Some(app) = skip_app.upgrade() {
                app.set_update_status(STATUS_SKIPPED);
                app.set_update_message(format!("Версия {version} пропущена").into());
            }
        }
    });

    let install_app = app.as_weak();
    app.on_install_update(move || {
        if let Some(app) = install_app.upgrade() {
            app.set_notice_message("Обновление загружено и проверено".into());
            app.set_notice_visible(true);
        }
    });
}

fn apply_check_event(
    app: &AppWindow,
    event: CheckEvent,
    state: &Mutex<UpdateState>,
    settings: &Mutex<AppSettings>,
    manual_check: &AtomicBool,
) {
    match event {
        CheckEvent::Checking => {
            app.set_update_status(STATUS_CHECKING);
            app.set_update_message("Ищем новую версию…".into());
        }
        CheckEvent::Available(update) => {
            let manual = manual_check.swap(false, Ordering::AcqRel);
            let skipped = settings
                .lock()
                .ok()
                .and_then(|settings| settings.skipped_update_version().map(str::to_owned))
                .is_some_and(|version| version == update.version.to_string());
            if skipped && !manual {
                app.set_update_status(STATUS_SKIPPED);
                app.set_update_version("".into());
                app.set_update_message("Вы пропустили эту версию".into());
                return;
            }
            let size = readable_size(update.package.size);
            app.set_update_version(format!("v{}", update.version).into());
            app.set_update_message(format!("Новая версия готова к загрузке · {size}").into());
            app.set_update_notes(
                update
                    .notes
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" • ")
                    .into(),
            );
            app.set_update_status(STATUS_AVAILABLE);
            if let Ok(mut state) = state.lock() {
                state.available = Some(update);
                state.downloaded_path = None;
            }
        }
        CheckEvent::UpToDate => {
            manual_check.store(false, Ordering::Release);
            app.set_update_status(STATUS_CURRENT);
            app.set_update_version("".into());
            app.set_update_notes("".into());
            app.set_update_message("Установлена последняя версия".into());
        }
        CheckEvent::Failed(error) => {
            let manual = manual_check.swap(false, Ordering::AcqRel);
            if manual {
                app.set_update_status(STATUS_FAILED);
                app.set_update_message(format!("Не удалось проверить обновления: {error}").into());
            } else {
                app.set_update_status(STATUS_IDLE);
                app.set_update_message(
                    "Автоматическая проверка не удалась. Можно повторить вручную.".into(),
                );
                eprintln!("Автоматическая проверка обновлений не удалась: {error}");
            }
        }
    }
}

fn apply_download_event(app: &AppWindow, event: DownloadEvent, state: &Mutex<UpdateState>) {
    match event {
        DownloadEvent::Started { downloaded, total }
        | DownloadEvent::Progress { downloaded, total } => {
            app.set_update_status(STATUS_DOWNLOADING);
            app.set_update_progress(progress(downloaded, total));
            app.set_update_message(
                format!(
                    "Загружаем обновление · {} из {}",
                    readable_size(downloaded),
                    readable_size(total)
                )
                .into(),
            );
        }
        DownloadEvent::Paused { downloaded, total } => {
            app.set_update_status(STATUS_PAUSED);
            app.set_update_progress(progress(downloaded, total));
            app.set_update_message("Загрузка приостановлена".into());
        }
        DownloadEvent::Completed { path } => {
            if let Ok(mut state) = state.lock() {
                state.downloaded_path = Some(path);
            }
            app.set_update_progress(1.0);
            app.set_update_status(STATUS_READY);
            app.set_update_message("Пакет загружен и проверен. Можно установить.".into());
        }
        DownloadEvent::Cancelled => {
            app.set_update_progress(0.0);
            app.set_update_status(STATUS_AVAILABLE);
            app.set_update_message("Загрузка отменена".into());
        }
        DownloadEvent::Failed(error) => {
            app.set_update_status(STATUS_FAILED);
            app.set_update_message(format!("Не удалось загрузить обновление: {error}").into());
        }
    }
}

fn progress(downloaded: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (downloaded as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

fn readable_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} МБ", bytes as f64 / MIB)
    } else {
        format!("{} КБ", bytes.div_ceil(1024))
    }
}
