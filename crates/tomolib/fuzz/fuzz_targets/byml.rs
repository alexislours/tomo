#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::byml::Byml;

fuzz_target!(|data: &[u8]| {
    let _ = Byml::parse(data);
});
