// Oxide regression tests for the local resource-ownership patch.
// These tests use the real libmpv allocator, with no media, window or audio output.
use super::*;
use std::{cell::Cell, panic::catch_unwind};

thread_local! {
    static CONTEXT_DESTROYS: Cell<usize> = const { Cell::new(0) };
    static BUFFER_FREES: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn record_context_destroy() {
    CONTEXT_DESTROYS.with(|count| count.set(count.get() + 1));
}

pub(super) fn record_buffer_free() {
    BUFFER_FREES.with(|count| count.set(count.get() + 1));
}

fn contexts_destroyed() -> usize {
    CONTEXT_DESTROYS.with(Cell::get)
}

fn buffers_freed() -> usize {
    BUFFER_FREES.with(Cell::get)
}

fn headless_mpv() -> Mpv {
    Mpv::with_initializer(|init| {
        init.set_property("config", "no")?;
        init.set_property("load-scripts", "no")?;
        init.set_property("terminal", "no")?;
        init.set_property("vo", "null")?;
        init.set_property("ao", "null")?;
        Ok(())
    })
    .expect("headless libmpv must initialize")
}

fn decode_native_string<T: GetData>(mpv: &Mpv, invalid_utf8: bool) -> Result<T> {
    // SAFETY: the handle is live and the property name is NUL-terminated.
    // mpv transfers ownership of the returned allocation to the caller.
    let buffer = unsafe {
        libmpv_sys::mpv_get_property_string(mpv.ctx.as_ptr(), b"mpv-version\0".as_ptr() as _)
    };
    assert!(!buffer.is_null());
    // SAFETY: mpv-version contains at least one byte and a trailing NUL. The
    // allocation is exclusively ours; altering its first byte preserves its size.
    unsafe {
        assert_ne!(*buffer, 0);
        if invalid_utf8 {
            *buffer = 0xff_u8 as ctype::c_char;
        }
    }
    T::get_from_c_void(|output| {
        // SAFETY: GetData for String/MpvStr provides space for a string pointer;
        // the successful callback hands ownership of our native allocation to it.
        unsafe { ptr::write(output as *mut *mut ctype::c_char, buffer) };
        Ok(())
    })
}

#[test]
fn callback_error_destroys_each_created_context_once() {
    let before = contexts_destroyed();
    for _ in 0..32 {
        let result = Mpv::with_initializer(|_| Err(Error::Raw(mpv_error::OptionError)));
        assert!(matches!(result, Err(Error::Raw(mpv_error::OptionError))));
    }
    assert_eq!(contexts_destroyed() - before, 32);
}

#[test]
fn callback_panic_also_destroys_context() {
    let before = contexts_destroyed();
    let result = catch_unwind(|| Mpv::with_initializer(|_| panic!("initializer failure")));
    assert!(result.is_err());
    assert_eq!(contexts_destroyed() - before, 1);
}

#[test]
fn successful_context_is_destroyed_only_when_owner_is_dropped() {
    let before = contexts_destroyed();
    let mpv = headless_mpv();
    assert_eq!(contexts_destroyed(), before);
    assert!(!mpv
        .get_property::<String>("mpv-version")
        .unwrap()
        .is_empty());
    drop(mpv);
    assert_eq!(contexts_destroyed() - before, 1);
}

#[test]
fn owned_string_releases_native_buffer_on_invalid_utf8() {
    let mpv = headless_mpv();
    let before = buffers_freed();
    for _ in 0..32 {
        assert_eq!(
            decode_native_string::<String>(&mpv, true),
            Err(Error::InvalidUtf8)
        );
    }
    assert_eq!(buffers_freed() - before, 32);
}

#[test]
fn mpv_str_releases_native_buffer_on_invalid_utf8() {
    let mpv = headless_mpv();
    let before = buffers_freed();
    for _ in 0..32 {
        assert_eq!(
            decode_native_string::<MpvStr<'_>>(&mpv, true),
            Err(Error::InvalidUtf8)
        );
    }
    assert_eq!(buffers_freed() - before, 32);
}

#[test]
fn owned_string_releases_buffer_after_copying() {
    let mpv = headless_mpv();
    let before = buffers_freed();
    let value = decode_native_string::<String>(&mpv, false).unwrap();
    assert!(!value.is_empty());
    assert_eq!(buffers_freed() - before, 1);
    drop(value);
    assert_eq!(buffers_freed() - before, 1);
}

#[test]
fn mpv_str_transfers_allocation_until_drop() {
    let mpv = headless_mpv();
    let before = buffers_freed();
    let value = decode_native_string::<MpvStr<'_>>(&mpv, false).unwrap();
    assert!(!value.is_empty());
    assert_eq!(buffers_freed(), before);
    drop(value);
    assert_eq!(buffers_freed() - before, 1);
}

#[test]
fn failed_getter_or_null_pointer_does_not_free_unowned_memory() {
    let before = buffers_freed();
    assert_eq!(
        String::get_from_c_void(|_| Err::<(), _>(Error::Null)),
        Err(Error::Null)
    );
    assert_eq!(
        MpvStr::get_from_c_void(|_| Err::<(), _>(Error::Null)),
        Err(Error::Null)
    );
    assert_eq!(String::get_from_c_void(|_| Ok(())), Err(Error::Null));
    assert_eq!(MpvStr::get_from_c_void(|_| Ok(())), Err(Error::Null));
    assert_eq!(buffers_freed(), before);
}
