use super::*;
use std::time::Instant;

#[test]
fn stalled_player_keeps_only_latest_adjacent_absolute_value() {
    let (tx, rx, errors) = channel();
    let started = Instant::now();
    for value in 0..100_000 {
        tx.send(Command::Seek(f64::from(value))).unwrap();
    }
    assert_eq!(tx.shared.queue.lock().unwrap().commands.len(), 1);
    assert!(matches!(rx.try_recv(true), Some(Command::Seek(99_999.0))));
    assert!(rx.try_recv(true).is_none());
    assert!(errors.is_empty());
    eprintln!(
        "command overload: 100000 seeks, 1 pending command, producer {:?}",
        started.elapsed()
    );
}

#[test]
fn coalescing_does_not_cross_media_relative_or_toggle_boundaries() {
    let (tx, rx, _) = channel();
    for command in [
        Command::Seek(1.0),
        Command::Seek(2.0),
        Command::OpenFile("next.mp4".into()),
        Command::Seek(3.0),
        Command::SeekRelative(-5.0),
        Command::Seek(4.0),
        Command::TogglePlay,
        Command::TogglePlay,
        Command::AdjustVolume(-5.0),
        Command::AdjustVolume(5.0),
    ] {
        tx.send(command).unwrap();
    }
    assert!(matches!(rx.try_recv(true), Some(Command::Seek(2.0))));
    assert!(matches!(rx.try_recv(true), Some(Command::OpenFile(path)) if path == "next.mp4"));
    assert!(matches!(rx.try_recv(true), Some(Command::Seek(3.0))));
    assert!(matches!(
        rx.try_recv(true),
        Some(Command::SeekRelative(-5.0))
    ));
    assert!(matches!(rx.try_recv(true), Some(Command::Seek(4.0))));
    assert!(matches!(rx.try_recv(true), Some(Command::TogglePlay)));
    assert!(matches!(rx.try_recv(true), Some(Command::TogglePlay)));
    assert!(matches!(
        rx.try_recv(true),
        Some(Command::AdjustVolume(-5.0))
    ));
    assert!(matches!(
        rx.try_recv(true),
        Some(Command::AdjustVolume(5.0))
    ));
    assert!(rx.try_recv(true).is_none());
}

#[test]
fn full_mailbox_rejects_new_actions_but_can_replace_its_tail() {
    let (tx, rx, errors) = channel();
    for _ in 0..CAPACITY - 1 {
        tx.send(Command::NextVideo).unwrap();
    }
    tx.send(Command::SetVolume(20.0)).unwrap();
    tx.send(Command::SetVolume(80.0)).unwrap();
    for _ in 0..100_000 {
        assert_eq!(tx.send(Command::TogglePlay), Err(SendError::Full));
    }
    assert_eq!(errors.len(), 1);
    assert_eq!(errors.try_recv(), Ok(SendError::Full));
    assert_eq!(tx.shared.queue.lock().unwrap().commands.len(), CAPACITY);
    for _ in 0..CAPACITY - 1 {
        assert!(matches!(rx.try_recv(true), Some(Command::NextVideo)));
    }
    assert!(matches!(rx.try_recv(true), Some(Command::SetVolume(80.0))));
    assert!(rx.try_recv(true).is_none());
    tx.send(Command::TogglePlay).unwrap(); // free capacity can be reused
}

#[test]
fn output_startup_never_moves_commands_into_an_unbounded_pending_queue() {
    let (tx, rx, _) = channel();
    tx.send(Command::SetVolume(35.0)).unwrap();
    tx.send(Command::OpenFile("first.mp4".into())).unwrap();
    tx.send(Command::Seek(10.0)).unwrap();
    assert!(matches!(rx.try_recv(false), Some(Command::SetVolume(35.0))));
    for _ in 0..1000 {
        assert!(rx.try_recv(false).is_none());
    }
    for _ in 2..CAPACITY {
        tx.send(Command::NextVideo).unwrap();
    }
    assert_eq!(
        tx.send(Command::OpenFile("extra.mp4".into())),
        Err(SendError::Full)
    );
    // Reserved signals bypass the blocked front and a full mailbox.
    tx.send(Command::SetVideoWindow(42)).unwrap();
    tx.send(Command::SetVideoWindow(43)).unwrap();
    tx.send(Command::RenderApiReady).unwrap();
    tx.send(Command::RenderApiReady).unwrap();
    assert!(matches!(rx.try_recv(false), Some(Command::RenderApiReady)));
    assert!(matches!(
        rx.try_recv(false),
        Some(Command::SetVideoWindow(43))
    ));
    assert!(matches!(rx.try_recv(true), Some(Command::OpenFile(path)) if path == "first.mp4"));
    assert!(matches!(rx.try_recv(true), Some(Command::Seek(10.0))));
}

#[test]
fn quit_bypasses_capacity_mutex_and_reserved_controls() {
    let (tx, rx, _) = channel();
    for _ in 0..CAPACITY {
        tx.send(Command::TogglePlay).unwrap();
    }
    tx.send(Command::RenderApiReady).unwrap();
    let held_queue = tx.shared.queue.lock().unwrap();
    tx.send(Command::Quit).unwrap(); // cannot acquire the already-held mutex
    assert!(rx.quit_requested());
    assert!(matches!(rx.try_recv(false), Some(Command::Quit)));
    drop(held_queue);
    assert_eq!(tx.send(Command::TogglePlay), Err(SendError::Disconnected));
}

#[test]
fn dropping_receiver_releases_pending_paths_and_reports_disconnect() {
    let (tx, rx, errors) = channel();
    tx.send(Command::OpenFile("pending.mp4".into())).unwrap();
    tx.send(Command::SetVideoWindow(42)).unwrap();
    drop(rx);
    assert!(tx.shared.queue.lock().unwrap().commands.is_empty());
    assert!(tx.shared.queue.lock().unwrap().video_window.is_none());
    assert_eq!(tx.send(Command::TogglePlay), Err(SendError::Disconnected));
    assert_eq!(errors.try_recv(), Ok(SendError::Disconnected));
}

#[test]
fn oversized_path_payload_never_enters_the_mailbox() {
    let (tx, rx, errors) = channel();
    let result = tx.send(Command::OpenFile("x".repeat(PATH_TEXT_BYTES + 1)));
    assert_eq!(result, Err(SendError::PayloadTooLarge));
    assert_eq!(errors.try_recv(), Ok(SendError::PayloadTooLarge));
    assert!(rx.try_recv(true).is_none());
}

#[test]
fn absolute_settings_coalesce_without_losing_other_settings() {
    let (tx, rx, _) = channel();
    for command in [
        Command::SetVolume(5.0),
        Command::SetVolume(15.0),
        Command::SetSpeed(0.5),
        Command::SetSpeed(1.5),
        Command::SetDarkTheme(true),
        Command::SetDarkTheme(false),
        Command::RequestMediaInfo,
        Command::RequestMediaInfo,
    ] {
        tx.send(command).unwrap();
    }
    assert!(matches!(rx.try_recv(true), Some(Command::SetVolume(15.0))));
    assert!(matches!(rx.try_recv(true), Some(Command::SetSpeed(1.5))));
    assert!(matches!(
        rx.try_recv(true),
        Some(Command::SetDarkTheme(false))
    ));
    assert!(matches!(rx.try_recv(true), Some(Command::RequestMediaInfo)));
    assert!(rx.try_recv(true).is_none());
}

#[test]
fn concurrent_producers_share_one_limit_and_keep_each_producers_order() {
    let (tx, rx, _) = channel();
    let workers: Vec<_> = (0..4)
        .map(|producer| {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut accepted = Vec::new();
                for index in 0..1000 {
                    let id = producer * 1000 + index;
                    if tx.send(Command::PlayIndex(id)).is_ok() {
                        accepted.push(id);
                    }
                }
                accepted
            })
        })
        .collect();
    let expected: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(expected.iter().map(Vec::len).sum::<usize>(), CAPACITY);
    let mut actual = vec![Vec::new(); 4];
    while let Some(Command::PlayIndex(id)) = rx.try_recv(true) {
        actual[id / 1000].push(id);
    }
    assert_eq!(actual, expected);
}
