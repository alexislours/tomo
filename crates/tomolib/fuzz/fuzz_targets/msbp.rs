#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::msbp::Msbp;

fuzz_target!(|data: &[u8]| {
    let _ = Msbp::parse(data);
});
