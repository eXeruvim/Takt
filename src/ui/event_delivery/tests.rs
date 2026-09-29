use super::*;
use crossbeam_channel::TrySendError;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
enum TestEvent {
    Position(u32),
    Boundary(&'static str),
}

fn replaces(a: &TestEvent, b: &TestEvent) -> bool {
    matches!((a, b), (TestEvent::Position(_), TestEvent::Position(_)))
}

#[test]
fn slow_ui_has_one_callback_and_a_fixed_source_budget() {
    let (tx, rx) = bounded(PLAYER_EVENT_CAPACITY);
    let (jobs_tx, jobs) = bounded(1);
    let bridge = spawn(
        rx,
        |_: &u32, _: &u32| false,
        move |batch, done| jobs_tx.try_send((batch, done)).is_ok(),
    )
    .unwrap();
    tx.send(0).unwrap();
    let held_callback = jobs.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(held_callback.0.len() <= BATCH_LIMIT);
    let mut accepted = 0;
    let mut sampled_out = 0;
    let started = Instant::now();
    for value in 1..=100_000 {
        match tx.try_send(value) {
            Ok(()) => accepted += 1,
            Err(TrySendError::Full(_)) => sampled_out += 1,
            Err(TrySendError::Disconnected(_)) => panic!("unexpected disconnect"),
        }
    }
    let producer_time = started.elapsed();
    assert_eq!(accepted, PLAYER_EVENT_CAPACITY);
    assert_eq!(accepted + sampled_out, 100_000);
    assert_eq!(tx.len(), PLAYER_EVENT_CAPACITY);
    assert!(jobs.is_empty()); // cannot schedule a second callback before acknowledgement
    let stopping = Instant::now();
    drop(bridge); // must not wait for held_callback or a running UI event loop
    assert!(stopping.elapsed() < Duration::from_secs(2));
    assert!(matches!(tx.try_send(7), Err(TrySendError::Disconnected(_))));
    eprintln!(
        "event overload: 100000 offers, {accepted} queued, {sampled_out} sampled out, 1 UI callback, producer {producer_time:?}, stop {:?}",
        stopping.elapsed()
    );
    drop(held_callback);
}

#[test]
fn coalescing_never_crosses_file_or_terminal_boundaries() {
    let (tx, rx) = bounded(32);
    for event in [
        TestEvent::Position(1),
        TestEvent::Position(2),
        TestEvent::Boundary("loaded next file"),
        TestEvent::Position(0),
        TestEvent::Position(3),
        TestEvent::Boundary("completed"),
        TestEvent::Position(4),
        TestEvent::Position(5),
        TestEvent::Boundary("failed"),
    ] {
        tx.send(event).unwrap();
    }
    drop(tx);
    let (jobs_tx, jobs) = bounded(1);
    let bridge = spawn(rx, replaces, move |batch, done| {
        jobs_tx.try_send((batch, done)).is_ok()
    })
    .unwrap();
    let mut received = Vec::new();
    while let Ok((batch, done)) = jobs.recv_timeout(Duration::from_secs(2)) {
        assert!(batch.len() <= BATCH_LIMIT);
        received.extend(batch);
        drop(done);
    }
    assert_eq!(
        received,
        [
            TestEvent::Position(2),
            TestEvent::Boundary("loaded next file"),
            TestEvent::Position(3),
            TestEvent::Boundary("completed"),
            TestEvent::Position(5),
            TestEvent::Boundary("failed")
        ]
    );
    drop(bridge);
}

#[test]
fn player_command_and_event_channels_preserve_media_boundaries() {
    use crate::{
        ipc::{Command, Event},
        player::commands,
    };

    let (command_tx, command_rx, command_errors) = commands::channel();
    command_tx
        .send(Command::OpenFile(r"C:\Музыка\трек с пробелом.mp3".into()))
        .unwrap();
    let Some(Command::OpenFile(path)) = command_rx.try_recv(true) else {
        panic!("media command was not delivered to the player");
    };
    assert_eq!(path, r"C:\Музыка\трек с пробелом.mp3");
    assert!(command_errors.is_empty());

    let (event_tx, event_rx) = bounded(PLAYER_EVENT_CAPACITY);
    let (jobs_tx, jobs) = bounded(1);
    let bridge = spawn(
        event_rx,
        |previous, next| matches!((previous, next), (Event::TimePos(_), Event::TimePos(_))),
        move |batch, done| jobs_tx.try_send((batch, done)).is_ok(),
    )
    .unwrap();
    for event in [
        Event::LoadStarted("трек с пробелом.mp3".into()),
        Event::TimePos(1.0),
        Event::TimePos(2.0),
        Event::FileLoaded {
            name: "трек с пробелом.mp3".into(),
            path: path.clone(),
        },
        Event::PlaybackState(true),
    ] {
        event_tx.send(event).unwrap();
    }

    let (batch, completion) = jobs.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(batch.len(), 4);
    assert!(matches!(&batch[0], Event::LoadStarted(name) if name == "трек с пробелом.mp3"));
    assert!(matches!(batch[1], Event::TimePos(value) if value == 2.0));
    assert!(matches!(&batch[2], Event::FileLoaded { name, path: delivered } if
        name == "трек с пробелом.mp3" && delivered == &path));
    assert!(matches!(batch[3], Event::PlaybackState(true)));
    drop(completion);
    drop(bridge);
    assert!(event_tx.send(Event::PlaybackStopped).is_err());
}

#[test]
fn stopping_bridge_unblocks_a_critical_sender_even_if_ui_never_runs() {
    let (tx, rx) = bounded(1);
    let (jobs_tx, jobs) = bounded(1);
    let bridge = spawn(
        rx,
        |_: &u8, _: &u8| false,
        move |batch, done| jobs_tx.try_send((batch, done)).is_ok(),
    )
    .unwrap();
    tx.send(1).unwrap();
    let callback = jobs.recv_timeout(Duration::from_secs(2)).unwrap();
    tx.send(2).unwrap();
    let sender = thread::spawn(move || tx.send(3));
    drop(bridge);
    assert!(sender.join().unwrap().is_err());
    drop(callback);
}

#[test]
fn scheduling_failure_disconnects_producers() {
    let (tx, rx) = bounded(1);
    let (failed_tx, failed_rx) = bounded(1);
    let bridge = spawn(
        rx,
        |_: &u8, _: &u8| false,
        move |_, _| {
            failed_tx.send(()).unwrap();
            false
        },
    )
    .unwrap();
    tx.send(1).unwrap();
    failed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(bridge);
    assert!(tx.try_send(2).is_err());
}

#[test]
fn early_callback_exit_acknowledges_the_batch() {
    let (tx, rx) = bounded(16);
    for i in 0..16 {
        tx.send(i).unwrap();
    }
    drop(tx);
    let (observed_tx, observed_rx) = bounded(2);
    let bridge = spawn(
        rx,
        |_: &u8, _: &u8| false,
        move |batch, _completion| {
            observed_tx.send(batch.len()).unwrap();
            true // returning without retaining Completion models an early UI return
        },
    )
    .unwrap();
    assert_eq!(
        observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        BATCH_LIMIT
    );
    assert_eq!(
        observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        BATCH_LIMIT
    );
    drop(bridge);
}
