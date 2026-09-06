mod converter;
mod ipc;

mod player;
mod render;
mod settings;
mod ui;

use crossbeam_channel::unbounded;
use std::thread;

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

    let converter_thread = thread::spawn(move || {
        converter::run(converter_rx, converter_event_tx);
    });

    let ui_result = ui::ui_logic::run(
        ui_tx.clone(),
        ui_rx,
        converter_tx.clone(),
        converter_event_rx,
    );

    let _ = ui_tx.send(ipc::Command::Quit);
    let _ = converter_tx.send(converter::ConverterCommand::Quit);

    let _ = player_thread.join();
    let _ = converter_thread.join();

    ui_result
}
