use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::converter::{
    ConversionOptions, ConversionPreset, ConverterCommand, ConverterControl, ConverterEvent,
    output_filter, suggested_output_path,
};
use crate::player::media_info::MediaInfo;
use crate::player::playlist::{AUDIO_EXTENSIONS, MEDIA_EXTENSIONS, VIDEO_EXTENSIONS};
use crate::ui::presentation::{
    format_codec, format_container, format_precise_file_size, format_remaining_time,
    format_size_change, format_time, is_audio_only_media,
};
#[cfg(target_os = "windows")]
use crate::ui::system_notification::{self, NotificationKind};
#[cfg(target_os = "windows")]
use crate::ui::window_interaction::native_window_handle;

use super::{
    event_delivery::{self, Bridge},
    ui_logic::{AppWindow, ConversionHistoryEntry, ConversionPresetEntry},
};

const MAX_HISTORY_ENTRIES: usize = 50;

#[derive(Clone, Debug)]
struct HistoryRecord {
    title: String,
    path: String,
    details: String,
    status: String,
    state: i32,
}

pub(super) fn apply_presets(app: &AppWindow, audio_media: bool, audio_codec: &str) {
    let presets = preset_entries(audio_media, audio_codec);
    let selected = presets
        .iter()
        .find(|preset| preset.index == app.get_conversion_preset_index())
        .or_else(|| presets.first())
        .cloned();

    if let Some(preset) = selected {
        app.set_conversion_preset_index(preset.index);
        app.set_conversion_preset_title(preset.title);
        app.set_conversion_preset_description(preset.description);
        app.set_conversion_preset_summary(preset.summary);
        app.set_conversion_preset_size_note(preset.size_note);
    }
    app.set_conversion_presets(ModelRc::new(VecModel::from(presets)));
}

pub(super) fn register_callbacks(
    app: &AppWindow,
    converter_tx: Sender<ConverterCommand>,
    converter_control: Arc<ConverterControl>,
    dialog_active: Arc<AtomicBool>,
) {
    {
        let app_weak = app.as_weak();
        let converter_tx = converter_tx.clone();
        let converter_control = converter_control.clone();

        app.on_open_converter(move || {
            if let Some(app) = app_weak.upgrade() {
                app.set_command_palette_visible(false);
                app.set_info_drawer_visible(false);
                app.set_playlist_drawer_visible(false);
                app.set_settings_drawer_visible(false);
                app.set_inspector_reserved(false);
                app.set_converter_visible(true);
                if let Err(message) = converter_control.request_refresh(&converter_tx) {
                    show_error(&app, message);
                }
            }
        });
    }

    {
        let app_weak = app.as_weak();
        let dialog_active = dialog_active.clone();

        app.on_choose_conversion_source(move || {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let Some(app) = app_weak.upgrade() else {
                dialog_active.store(false, Ordering::Release);
                return;
            };
            app.set_conversion_source_loading(true);
            app.set_conversion_source_error("".into());
            let app_weak = app.as_weak();
            let dialog_active = dialog_active.clone();

            std::thread::spawn(move || {
                let path = rfd::FileDialog::new()
                    .add_filter("Медиа", MEDIA_EXTENSIONS)
                    .add_filter("Видео", VIDEO_EXTENSIONS)
                    .add_filter("Аудио", AUDIO_EXTENSIONS)
                    .pick_file();

                let probe = path
                    .as_deref()
                    .map(|path| probe_source(path).map(|info| (path.to_path_buf(), info)));
                dialog_active.store(false, Ordering::Release);

                let _ = slint::invoke_from_event_loop(move || {
                    let Some(app) = app_weak.upgrade() else {
                        return;
                    };
                    app.set_conversion_source_loading(false);

                    match probe {
                        None => {}
                        Some(Err(error)) => {
                            app.set_conversion_source_error(
                                format!("Не удалось прочитать файл: {error}").into(),
                            );
                        }
                        Some(Ok((path, info))) => apply_source(&app, path, info),
                    }
                });
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let dialog_active = dialog_active.clone();

        app.on_choose_conversion_output(move |index| {
            if dialog_active
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return;
            }

            let Some(app) = app_weak.upgrade() else {
                dialog_active.store(false, Ordering::Release);
                return;
            };
            let Some(preset) = ConversionPreset::from_ui_index(index) else {
                dialog_active.store(false, Ordering::Release);
                return;
            };
            app.set_conversion_preset_index(index);
            let source = PathBuf::from(app.get_conversion_source_path().as_str());
            let app_weak = app.as_weak();
            let dialog_active = dialog_active.clone();

            std::thread::spawn(move || {
                let suggested = suggested_output_path(&source, preset);
                let (filter_name, extensions) = output_filter(preset);
                let mut dialog = rfd::FileDialog::new().add_filter(filter_name, extensions);
                if let Some(path) = suggested.as_ref() {
                    if let Some(parent) = path.parent() {
                        dialog = dialog.set_directory(parent);
                    }
                    if let Some(file_name) = path.file_name() {
                        dialog = dialog.set_file_name(file_name.to_string_lossy());
                    }
                }
                let path = dialog.save_file();
                dialog_active.store(false, Ordering::Release);

                if let Some(path) = path {
                    let value = path.to_string_lossy().into_owned();
                    let weak = app_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(app) = weak.upgrade() {
                            app.set_conversion_output_path(value.into());
                            if !app.get_conversion_running() {
                                app.set_conversion_progress(0.0);
                                app.set_conversion_status("Готово к запуску".into());
                            }
                        }
                    });
                }
            });
        });
    }

    {
        let app_weak = app.as_weak();
        let converter_tx = converter_tx.clone();
        let converter_control = converter_control.clone();

        app.on_start_conversion(move |index| {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let Some(preset) = ConversionPreset::from_ui_index(index) else {
                show_error(&app, "Неизвестный профиль конвертации".to_owned());
                return;
            };
            let source = PathBuf::from(app.get_conversion_source_path().as_str());
            let output = PathBuf::from(app.get_conversion_output_path().as_str());
            let options = ConversionOptions::from_ui_indices(
                app.get_conversion_resolution_index(),
                app.get_conversion_bitrate_index(),
                app.get_conversion_encoding_speed_index(),
                app.get_conversion_keep_audio_tracks(),
                app.get_conversion_keep_subtitle_tracks(),
            );
            if !app.get_conversion_running() {
                app.set_conversion_status("Подготовка FFmpeg…".into());
                app.set_conversion_size_status("Ожидание данных FFmpeg…".into());
            }
            match crate::converter::submit(
                &converter_tx,
                ConverterCommand::Start {
                    queue_generation: converter_control.queue_generation(),
                    source,
                    output,
                    preset,
                    options,
                    duration_seconds: app.get_conversion_source_duration_seconds() as f64,
                },
            ) {
                Ok(()) => app.set_conversion_output_path("".into()),
                Err(message) => {
                    if !app.get_conversion_running() {
                        app.set_conversion_status("Задача не добавлена".into());
                    }
                    show_error(&app, message);
                }
            }
        });
    }

    {
        let converter_control = converter_control.clone();
        app.on_cancel_conversion(move || converter_control.cancel_active());
    }

    app.on_clear_conversion_queue(move || converter_control.clear_pending());
}

pub(super) fn spawn_event_bridge(
    app: &AppWindow,
    converter_rx: Receiver<ConverterEvent>,
) -> std::io::Result<Bridge> {
    let converter_weak = app.as_weak();
    let mut history = Vec::new();
    event_delivery::spawn(
        converter_rx,
        |previous, next| {
            matches!(
                (previous, next),
                (
                    ConverterEvent::Progress { .. },
                    ConverterEvent::Progress { .. }
                )
            )
        },
        move |batch, completion| {
            let batch: Vec<_> = batch
                .into_iter()
                .map(|event| {
                    let history_snapshot = history_record(&event).map(|entry| {
                        history.insert(0, entry);
                        history.truncate(MAX_HISTORY_ENTRIES);
                        history.clone()
                    });
                    (event, history_snapshot)
                })
                .collect();
            let weak = converter_weak.clone();
            slint::invoke_from_event_loop(move || {
                let _completion = completion;
                let Some(app) = weak.upgrade() else {
                    return;
                };

                for (event, history_snapshot) in batch {
                    if let Some(records) = history_snapshot {
                        app.set_conversion_history(ModelRc::new(VecModel::from(history_entries(
                            records,
                        ))));
                    }
                    apply_event(&app, event);
                }
            })
            .is_ok()
        },
    )
}

fn apply_source(app: &AppWindow, path: PathBuf, info: MediaInfo) {
    let audio_media = is_audio_only_media(info.video_track_count, &info.codec, &info.audio_codec);
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    app.set_conversion_source_path(path.to_string_lossy().into_owned().into());
    app.set_conversion_source_name(name.into());
    app.set_conversion_source_size(format_precise_file_size(info.file_size_bytes).into());
    app.set_conversion_source_duration_seconds(info.duration_seconds.max(0.0) as f32);
    app.set_conversion_source_duration(
        if info.duration_seconds.is_finite() && info.duration_seconds > 0.0 {
            format_time(info.duration_seconds).into()
        } else {
            "—".into()
        },
    );
    app.set_conversion_source_format(if audio_media {
        format_codec(&info.audio_codec).into()
    } else {
        format_container(&info.container).into()
    });
    app.set_conversion_audio_media(audio_media);
    app.set_conversion_source_error("".into());
    app.set_conversion_output_path("".into());
    app.set_conversion_progress(0.0);
    app.set_conversion_status("".into());
    app.set_conversion_size_status("".into());
    apply_presets(app, audio_media, &info.audio_codec);
}

fn apply_event(app: &AppWindow, event: ConverterEvent) {
    match event {
        ConverterEvent::Availability { available, details } => {
            app.set_ffmpeg_available(available);
            app.set_ffmpeg_details(details.into());
            if !available && !app.get_conversion_running() {
                app.set_conversion_status("Установите FFmpeg для продолжения".into());
            }
        }
        ConverterEvent::Started { output } => {
            app.set_conversion_running(true);
            app.set_conversion_progress(0.0);
            app.set_conversion_status(format!("Конвертация: {}", output.display()).into());
            app.set_conversion_size_status("Записано: 0 Б".into());
        }
        ConverterEvent::QueueUpdated { pending } => {
            app.set_conversion_queue_count(i32::try_from(pending).unwrap_or(i32::MAX));
        }
        ConverterEvent::Progress {
            fraction,
            remaining_seconds,
            current_size_bytes,
        } => {
            app.set_conversion_progress(fraction.clamp(0.0, 1.0) as f32);
            let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u32;
            app.set_conversion_status(
                remaining_seconds
                    .map(|seconds| {
                        format!("{percent}% · осталось {}", format_remaining_time(seconds))
                    })
                    .unwrap_or_else(|| format!("{percent}%"))
                    .into(),
            );
            app.set_conversion_size_status(
                format!(
                    "Записано сейчас: {}",
                    format_precise_file_size(current_size_bytes)
                )
                .into(),
            );
        }
        ConverterEvent::Completed {
            output,
            source_size_bytes,
            output_size_bytes,
        } => {
            let has_next = app.get_conversion_queue_count() > 0;
            app.set_conversion_running(has_next);
            apply_presets(app, app.get_audio_media(), app.get_audio_codec().as_str());
            app.set_conversion_progress(1.0);
            app.set_conversion_status(
                if has_next {
                    format!(
                        "Готово: {} · запускается следующая задача",
                        output.display()
                    )
                } else {
                    format!("Готово: {}", output.display())
                }
                .into(),
            );
            app.set_conversion_size_status(
                format_size_change(source_size_bytes, output_size_bytes).into(),
            );
            notify_completed(app, &output, source_size_bytes, output_size_bytes);
        }
        ConverterEvent::Canceled { .. } => {
            let has_next = app.get_conversion_queue_count() > 0;
            app.set_conversion_running(has_next);
            apply_presets(app, app.get_audio_media(), app.get_audio_codec().as_str());
            app.set_conversion_progress(0.0);
            app.set_conversion_status(
                if has_next {
                    "Текущая задача отменена · запускается следующая"
                } else {
                    "Конвертация отменена"
                }
                .into(),
            );
            app.set_conversion_size_status("Временный файл удалён; исходник не изменён".into());
        }
        ConverterEvent::Rejected {
            message, active, ..
        } => {
            if !active {
                app.set_conversion_running(false);
                app.set_conversion_status("Задача не запущена".into());
            }
            show_error(app, message);
        }
        ConverterEvent::Failed { output, message } => {
            let has_next = app.get_conversion_queue_count() > 0;
            app.set_conversion_running(has_next);
            apply_presets(app, app.get_audio_media(), app.get_audio_codec().as_str());
            app.set_conversion_status(
                if has_next {
                    "Ошибка конвертации · запускается следующая задача"
                } else {
                    "Ошибка конвертации"
                }
                .into(),
            );
            app.set_conversion_size_status("Итоговый файл не создан; исходник не изменён".into());
            notify_failed(app, &output, &message);
            show_error(app, message);
        }
    }
}

fn show_error(app: &AppWindow, message: impl Into<SharedString>) {
    app.set_error_message(message.into());
    app.set_error_visible(true);
}

#[cfg(target_os = "windows")]
fn notify_completed(
    app: &AppWindow,
    output: &Path,
    source_size_bytes: u64,
    output_size_bytes: u64,
) {
    if let Some(hwnd) = native_window_handle(app.window()) {
        let notification = format!(
            "{} · {}",
            output_title(output),
            format_size_change(source_size_bytes, output_size_bytes)
        );
        system_notification::show(
            hwnd as usize,
            "Конвертация завершена",
            &notification,
            NotificationKind::Information,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn notify_completed(_: &AppWindow, _: &Path, _: u64, _: u64) {}

#[cfg(target_os = "windows")]
fn notify_failed(app: &AppWindow, output: &Path, message: &str) {
    if let Some(hwnd) = native_window_handle(app.window()) {
        let notification = format!(
            "{} · {}",
            output_title(output),
            compact_error_message(message)
        );
        system_notification::show(
            hwnd as usize,
            "Ошибка конвертации",
            &notification,
            NotificationKind::Error,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn notify_failed(_: &AppWindow, _: &Path, _: &str) {}

fn preset_entries(audio_media: bool, audio_codec: &str) -> Vec<ConversionPresetEntry> {
    if !audio_media {
        return vec![
            preset(
                0,
                "Видео",
                true,
                "MP4 · H.264",
                "Максимальная совместимость",
                "MP4 · H.264 · AAC · CRF 20",
                "Точный размер зависит от содержимого видео и станет известен после кодирования",
            ),
            preset(
                1,
                "Видео",
                false,
                "MP4 · H.265",
                "Меньше размер при высоком качестве",
                "MP4 · H.265/HEVC · AAC · CRF 24",
                "Точный размер зависит от содержимого видео и станет известен после кодирования",
            ),
            preset(
                2,
                "Видео",
                false,
                "WebM",
                "Открытый формат для веба",
                "WebM · VP9 · Opus · CRF 30",
                "Точный размер зависит от содержимого видео и станет известен после кодирования",
            ),
            preset(
                4,
                "Видео",
                false,
                "Компактный MP4",
                "Заметно уменьшить размер файла",
                "MP4 · H.264 · AAC · CRF 28",
                "Профиль нацелен на уменьшение, но точный результат зависит от содержимого",
            ),
            preset(
                3,
                "Аудио",
                true,
                "Извлечь MP3",
                "Оставить только звуковую дорожку",
                "MP3 · VBR высокого качества",
                "VBR меняет bitrate по содержимому; точный размер появится во время работы",
            ),
            preset(
                5,
                "Без потерь",
                true,
                "Скопировать потоки в MKV",
                "Быстро сменить контейнер без потери качества",
                "MKV · исходные видео, аудио и субтитры",
                "Будет близок к исходнику; небольшая разница зависит от контейнера MKV",
            ),
        ];
    }

    let codec = audio_codec.trim().to_ascii_lowercase();
    let mut entries = Vec::new();
    let mut add = |index, title, description, summary, size_note| {
        let show_category = entries.is_empty();
        entries.push(preset(
            index,
            "Аудио",
            show_category,
            title,
            description,
            summary,
            size_note,
        ));
    };

    if !codec.contains("mp3") {
        add(
            3,
            "MP3",
            "Компактный и максимально совместимый",
            "MP3 · VBR высокого качества",
            "VBR меняет bitrate по содержимому; точный размер появится во время работы",
        );
    }
    if !codec.contains("aac") {
        add(
            6,
            "M4A · AAC",
            "Эффективный формат для телефонов и плееров",
            "M4A · AAC · 192 кбит/с",
            "Аудиопоток кодируется в 192 кбит/с; итог дополнительно включает контейнер M4A",
        );
    }
    if !codec.contains("flac") {
        add(
            7,
            "FLAC",
            "Сжатие без потери качества",
            "FLAC · lossless · compression 8",
            "Lossless-размер зависит от сжимаемости записи и заранее точно неизвестен",
        );
    }
    if !codec.contains("pcm") {
        add(
            8,
            "WAV",
            "Несжатый PCM для монтажа и обработки",
            "WAV · PCM 16-bit",
            "Размер определяется частотой, каналами и длительностью исходной дорожки",
        );
    }
    if !codec.contains("opus") {
        add(
            9,
            "Opus",
            "Небольшой размер при хорошем качестве",
            "Opus · 128 кбит/с",
            "Аудиопоток кодируется в 128 кбит/с; итог дополнительно включает контейнер",
        );
    }

    entries
}

fn preset(
    index: i32,
    category: &str,
    show_category: bool,
    title: &str,
    description: &str,
    summary: &str,
    size_note: &str,
) -> ConversionPresetEntry {
    ConversionPresetEntry {
        index,
        category: category.into(),
        show_category,
        title: title.into(),
        description: description.into(),
        summary: summary.into(),
        size_note: size_note.into(),
    }
}

fn output_title(output: &Path) -> String {
    output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Файл конвертации".to_owned())
}

fn compact_error_message(message: &str) -> String {
    message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn history_record(event: &ConverterEvent) -> Option<HistoryRecord> {
    let (output, details, status, state) = match event {
        ConverterEvent::Completed {
            output,
            source_size_bytes,
            output_size_bytes,
        } => (
            output,
            format_size_change(*source_size_bytes, *output_size_bytes),
            "Готово",
            0,
        ),
        ConverterEvent::Canceled { output } => (
            output,
            "Временный файл удалён; исходник не изменён".to_owned(),
            "Отменено",
            2,
        ),
        ConverterEvent::Rejected {
            output, message, ..
        } => (output, compact_error_message(message), "Не запущено", 1),
        ConverterEvent::Failed { output, message } => {
            (output, compact_error_message(message), "Ошибка", 1)
        }
        _ => return None,
    };

    Some(HistoryRecord {
        title: output_title(output),
        path: output.display().to_string(),
        details,
        status: status.to_owned(),
        state,
    })
}

fn history_entries(records: Vec<HistoryRecord>) -> Vec<ConversionHistoryEntry> {
    records
        .into_iter()
        .map(|record| ConversionHistoryEntry {
            title: record.title.into(),
            path: record.path.into(),
            details: record.details.into(),
            status: record.status.into(),
            state: record.state,
        })
        .collect()
}

fn quote_mpv_argument(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn probe_source(path: &Path) -> Result<MediaInfo> {
    let mpv = libmpv::Mpv::with_initializer(|initializer| {
        initializer.set_property("vo", "null".to_owned())?;
        initializer.set_property("ao", "null".to_owned())?;
        initializer.set_property("pause", true)?;
        initializer.set_property("audio-display", "no".to_owned())?;
        initializer.set_property("terminal", false)?;
        Ok(())
    })
    .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let mut events = mpv.create_event_context();
    let quoted_path = quote_mpv_argument(&path.to_string_lossy());
    mpv.command("loadfile", &[&quoted_path, "replace"])
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while std::time::Instant::now() < deadline {
        match events.wait_event(0.25) {
            Some(Ok(libmpv::events::Event::FileLoaded)) => return Ok(MediaInfo::read(&mpv)),
            Some(Err(error)) => return Err(anyhow::anyhow!("{error:?}")),
            _ => {}
        }
    }

    anyhow::bail!("файл не ответил за 8 секунд")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::converter::ConverterEvent;

    use super::{history_record, preset_entries};

    #[test]
    fn history_preserves_completed_result_details() {
        let event = ConverterEvent::Completed {
            output: Path::new("exports/clip.mp4").to_path_buf(),
            source_size_bytes: 100 * 1024 * 1024,
            output_size_bytes: 60 * 1024 * 1024,
        };

        let record = history_record(&event).expect("completed job must be recorded");
        assert_eq!(record.title, "clip.mp4");
        assert_eq!(record.status, "Готово");
        assert_eq!(record.state, 0);
        assert_eq!(record.details, "Результат: 60,00 МБ · на 40% меньше");
    }

    #[test]
    fn history_compacts_multiline_ffmpeg_errors() {
        let event = ConverterEvent::Failed {
            output: Path::new("exports/clip.webm").to_path_buf(),
            message: "FFmpeg завершился с ошибкой:\n  invalid codec  \n".to_owned(),
        };

        let record = history_record(&event).expect("failed job must be recorded");
        assert_eq!(record.status, "Ошибка");
        assert_eq!(record.state, 1);
        assert_eq!(
            record.details,
            "FFmpeg завершился с ошибкой: · invalid codec"
        );
    }

    #[test]
    fn presets_follow_actual_media_type_and_codec() {
        let video = preset_entries(false, "aac");
        assert!(video.iter().any(|preset| preset.index == 0));
        assert!(video.iter().any(|preset| preset.index == 3));

        let mp3 = preset_entries(true, "mp3");
        assert!(!mp3.iter().any(|preset| preset.index == 0));
        assert!(!mp3.iter().any(|preset| preset.index == 3));
        assert!(mp3.iter().any(|preset| preset.index == 7));

        let flac = preset_entries(true, "flac");
        assert!(!flac.iter().any(|preset| preset.index == 7));
        assert!(flac.iter().any(|preset| preset.index == 3));
    }
}
