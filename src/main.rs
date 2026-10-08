mod benchmark_diagnostics;
mod converter;
mod ipc;

mod player;
mod render;
mod settings;
mod storage;
mod ui;

use crossbeam_channel::bounded;
use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use ui::event_delivery::{CONVERTER_EVENT_CAPACITY, PLAYER_EVENT_CAPACITY};

mod shutdown;

#[derive(Debug, Eq, PartialEq)]
enum StartupRequest {
    Run(Option<PathBuf>),
    BuildInfo,
}

fn parse_startup_request(
    args: impl IntoIterator<Item = OsString>,
) -> anyhow::Result<StartupRequest> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(StartupRequest::Run(None));
    };

    if first == OsStr::new("--build-info") {
        if args.next().is_some() {
            anyhow::bail!("Параметр --build-info нельзя сочетать с путём к медиафайлу");
        }
        return Ok(StartupRequest::BuildInfo);
    }

    let path = if first == OsStr::new("--") {
        args.next()
            .ok_or_else(|| anyhow::anyhow!("После -- укажите путь к файлу или папке"))?
    } else {
        if first.to_string_lossy().starts_with("--") {
            anyhow::bail!("Неизвестный параметр: {}", first.to_string_lossy());
        }
        first
    };

    if args.next().is_some() {
        anyhow::bail!("Oxide принимает только один путь при запуске");
    }

    Ok(StartupRequest::Run(Some(PathBuf::from(path))))
}

fn print_build_info() {
    println!(
        "{}",
        serde_json::json!({
            "schema": 1,
            "package_version": env!("CARGO_PKG_VERSION"),
            "git_commit": env!("OXIDE_BUILD_GIT_COMMIT"),
            "tracked_clean": env!("OXIDE_BUILD_TRACKED_CLEAN") == "true",
            "tracked_state": env!("OXIDE_BUILD_TRACKED_CLEAN"),
        })
    );
}

fn main() -> anyhow::Result<()> {
    let startup_path = match parse_startup_request(std::env::args_os().skip(1))? {
        StartupRequest::BuildInfo => {
            print_build_info();
            return Ok(());
        }
        StartupRequest::Run(path) => path,
    };

    let (ui_tx, player_rx, command_errors) = player::commands::channel();
    if let Some(path) = startup_path {
        let command = if path.is_dir() {
            ipc::Command::OpenFolder(path)
        } else {
            ipc::Command::OpenFile(path)
        };
        ui_tx
            .send(command)
            .map_err(|error| anyhow::anyhow!(error.message()))?;
    }
    let (player_tx, ui_rx) = bounded::<ipc::Event>(PLAYER_EVENT_CAPACITY);
    let (render_source_tx, render_source_rx) = bounded::<render::mpv_opengl::MpvRenderSource>(1);
    let (converter_tx, converter_rx) =
        bounded::<converter::ConverterCommand>(converter::COMMAND_CAPACITY);
    let (converter_event_tx, converter_event_rx) =
        bounded::<converter::ConverterEvent>(CONVERTER_EVENT_CAPACITY);

    let player_thread = thread::spawn(move || {
        let error_tx = player_tx.clone();
        if let Err(err) = player::mpv::run(player_rx, player_tx, render_source_tx) {
            eprintln!("Player error: {err}");
            let _ = error_tx.send(ipc::Event::PlaybackFailed(format!(
                "Не удалось запустить проигрыватель: {err}"
            )));
        }
    });

    let converter_shutdown = Arc::new(AtomicBool::new(false));
    let converter_control = Arc::new(converter::ConverterControl::default());
    let worker_shutdown = converter_shutdown.clone();
    let worker_control = converter_control.clone();
    let converter_thread = thread::spawn(move || {
        converter::run(
            converter_rx,
            converter_event_tx,
            worker_shutdown,
            worker_control,
        );
    });

    let ui_result = ui::ui_logic::run(
        ui_tx.clone(),
        ui_rx,
        render_source_rx,
        command_errors,
        converter_tx.clone(),
        converter_event_rx,
        converter_control,
    );

    converter_shutdown.store(true, Ordering::Release);
    let _ = ui_tx.send(ipc::Command::Quit);
    // Shutdown has its own flag; a full command queue must not hold the UI exit.
    let _ = converter_tx.try_send(converter::ConverterCommand::Quit);

    // A queued Quit cannot interrupt a blocked native call or filesystem access.
    // Give workers time for normal cleanup, then report failure and let process
    // exit stop its remaining threads. Do not hang forever on a native join.
    shutdown::finish_workers(
        vec![("player", player_thread), ("converter", converter_thread)],
        Duration::from_secs(5),
    )?;

    ui_result
}

#[cfg(test)]
mod tests {
    use super::{StartupRequest, parse_startup_request};
    use std::{ffi::OsString, path::PathBuf};

    fn arguments(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn startup_without_arguments_opens_empty_window() {
        assert_eq!(
            parse_startup_request(Vec::<OsString>::new()).unwrap(),
            StartupRequest::Run(None)
        );
    }

    #[test]
    fn startup_accepts_one_media_path() {
        assert_eq!(
            parse_startup_request(arguments(&[r"C:\Музыка\трек.mp3"])).unwrap(),
            StartupRequest::Run(Some(PathBuf::from(r"C:\Музыка\трек.mp3")))
        );
    }

    #[test]
    fn startup_accepts_path_after_option_separator() {
        assert_eq!(
            parse_startup_request(arguments(&["--", "--не-параметр.mp4"])).unwrap(),
            StartupRequest::Run(Some(PathBuf::from("--не-параметр.mp4")))
        );
    }

    #[test]
    fn build_info_is_a_separate_mode() {
        assert_eq!(
            parse_startup_request(arguments(&["--build-info"])).unwrap(),
            StartupRequest::BuildInfo
        );
        assert!(parse_startup_request(arguments(&["--build-info", "video.mp4"])).is_err());
    }

    #[test]
    fn startup_rejects_unknown_options_and_extra_paths() {
        assert!(parse_startup_request(arguments(&["--unknown"])).is_err());
        assert!(parse_startup_request(arguments(&["one.mp4", "two.mp4"])).is_err());
        assert!(parse_startup_request(arguments(&["--"])).is_err());
    }
}
