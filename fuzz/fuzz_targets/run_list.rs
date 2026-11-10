#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    xylem_scan_mft::fuzzing::runs(data);
});
