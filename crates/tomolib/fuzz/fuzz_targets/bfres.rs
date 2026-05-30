#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::bfres::Bfres;

fuzz_target!(|data: &[u8]| {
    if let Ok(bfres) = Bfres::parse(data) {
        let _ = tomolib::formats::bfres::model::parse_models(&bfres);
        let noop = vec![None; bfres.embedded_files.len()];
        let _ = bfres.rebuild_with_embedded(&noop);
    }
});
