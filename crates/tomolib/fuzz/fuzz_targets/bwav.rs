#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::bwav::Bwav;

fuzz_target!(|data: &[u8]| {
    let _ = Bwav::parse(data.to_vec());
});
