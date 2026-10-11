#![no_main]

use libfuzzer_sys::fuzz_target;

mod player;

fuzz_target!(|data: &[u8]| {
    player::audio_metadata::fuzz_tag_bytes(data);
});
