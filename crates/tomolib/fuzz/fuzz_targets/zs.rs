#![no_main]

use libfuzzer_sys::fuzz_target;
use tomolib::formats::zs;

fuzz_target!(|data: &[u8]| {
    let _ = zs::decompress(data, std::io::sink());
});
