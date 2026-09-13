use super::*;
use crossbeam_channel::TrySendError;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
enum Event {
    Position(u32),
    Boundary(&'static str),
}

fn replaces(a: &Event, b: &Event) -> bool {
    matches!((a, b), (Event::Position(_), Event::Position(_)))
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
        Event::Position(1),
        Event::Position(2),
        Event::Boundary("loaded next file"),
        Event::Position(0),
        Event::Position(3),
        Event::Boundary("completed"),
        Event::Position(4),
        Event::Position(5),
        Event::Boundary("failed"),
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
            Event::Position(2),
            Event::Boundary("loaded next file"),
            Event::Position(3),
            Event::Boundary("completed"),
            Event::Position(5),
            Event::Boundary("failed")
        ]
    );
    drop(bridge);
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
