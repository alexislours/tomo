#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::ainb::Ainb;

fuzz_target!(|data: &[u8]| {
    let _ = Ainb::parse(data);
});
