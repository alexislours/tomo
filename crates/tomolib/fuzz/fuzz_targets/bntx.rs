#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::bntx::Bntx;

fuzz_target!(|data: &[u8]| {
    let _ = Bntx::parse(data);
});
