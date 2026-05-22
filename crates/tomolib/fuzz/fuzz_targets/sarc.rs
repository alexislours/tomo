#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::sarc::Sarc;

fuzz_target!(|data: &[u8]| {
    let _ = Sarc::parse(data.to_vec());
});
