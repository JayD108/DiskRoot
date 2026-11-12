#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok((idx, _, _)) = diskroot_core::Index::load_checked(&mut &data[..], data.len() as u64) {
        for i in 1..idx.len() as u32 {
            assert!(idx.parent(i) < i);
            let _ = idx.path(i);
        }
    }
});
