#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::rstbl::Rstbl;

fuzz_target!(|data: &[u8]| {
    let _ = Rstbl::parse(data);
});
