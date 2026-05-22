#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::bars::Bars;

fuzz_target!(|data: &[u8]| {
    let _ = Bars::parse(data.to_vec());
});
