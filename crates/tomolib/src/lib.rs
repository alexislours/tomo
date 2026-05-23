//! Parsers and writers for the Nintendo first-party file formats found in
//! recent Switch titles (BYML, SARC, AINB, BNTX, BWAV, BNVIB, BARS, RSTBL, and
//! the MSBT/MSBP message formats), plus zstd (de)compression.
//!
//! Each format lives in its own module under [`formats`]. The usual entry
//! point is a `parse` constructor that reads a byte buffer, paired with a
//! serializer (`to_bytes`, `to_binary`, `write`) that produces one back.

mod error;
mod hashlist;

pub mod formats;

pub use error::{Error, Result};
pub(crate) use hashlist::murmur3_x86_32_seed0;
