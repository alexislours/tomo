#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::msbt::Msbt;

fuzz_target!(|data: &[u8]| {
    let _ = Msbt::parse(data);
});
