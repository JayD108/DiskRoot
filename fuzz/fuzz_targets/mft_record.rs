#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut rec = data.to_vec();
    xylem_scan_mft::fuzzing::parse_record(&mut rec);
});
