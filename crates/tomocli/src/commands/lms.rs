use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tomolib::formats::msbp::Msbp;

pub(crate) use tomolib::formats::lms::yaml::{
    Registry, emit_msbp, emit_msbt, msbp_to_yaml, msbt_to_yaml, parse_msbp, parse_msbt,
    registry_from_msbp_bytes,
};

fn discover_msbp(input: &Path) -> Option<PathBuf> {
    let mut dir = input.parent();
    while let Some(d) = dir {
        let cand = d.join("project.msbp");
        if cand.is_file() {
            return Some(cand);
        }
        dir = d.parent();
    }
    None
}

pub(crate) fn load_registry(input: &Path, explicit: Option<PathBuf>) -> Result<Option<Registry>> {
    let path = explicit.or_else(|| discover_msbp(input));
    match path {
        Some(p) => {
            let bytes = fs::read(&p).with_context(|| format!("read msbp `{}`", p.display()))?;
            let msbp =
                Msbp::parse(&bytes).with_context(|| format!("parse msbp `{}`", p.display()))?;
            Ok(Some(Registry::from_msbp(&msbp)))
        }
        None => Ok(None),
    }
}
