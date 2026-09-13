mod converter;
mod ipc;

mod player;
mod render;
mod settings;
mod storage;
mod ui;

use crossbeam_channel::unbounded;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

mod shutdown;

fn main() -> anyhow::Result<()> {
    let (ui_tx, player_rx) = unbounded::<ipc::Command>();
    let (player_tx, ui_rx) = unbounded::<ipc::Event>();
    let (converter_tx, converter_rx) = unbounded::<converter::ConverterCommand>();
    let (converter_event_tx, converter_event_rx) = unbounded::<converter::ConverterEvent>();

    let player_thread = thread::spawn(move || {
        let error_tx = player_tx.clone();
        if let Err(err) = player::mpv::run(player_rx, player_tx) {
            eprintln!("Player error: {err}");
            let _ = error_tx.send(ipc::Event::PlaybackFailed(format!(
                "Не удалось запустить проигрыватель: {err}"
            )));
        }
    });

    let converter_shutdown = Arc::new(AtomicBool::new(false));
    let worker_shutdown = converter_shutdown.clone();
    let converter_thread = thread::spawn(move || {
        converter::run(converter_rx, converter_event_tx, worker_shutdown);
    });

    let ui_result = ui::ui_logic::run(
        ui_tx.clone(),
        ui_rx,
        converter_tx.clone(),
        converter_event_rx,
    );

    converter_shutdown.store(true, Ordering::Release);
    let _ = ui_tx.send(ipc::Command::Quit);
    let _ = converter_tx.send(converter::ConverterCommand::Quit);

    // A queued Quit cannot interrupt a blocked native call or filesystem access.
    // Give workers time for normal cleanup, then report failure and let process
    // exit stop its remaining threads. Do not hang forever on a native join.
    shutdown::finish_workers(
        vec![("player", player_thread), ("converter", converter_thread)],
        Duration::from_secs(5),
    )?;

    ui_result
}
