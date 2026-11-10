#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = xylem_session::remember::Snapshot::load(&mut &data[..]);
});
