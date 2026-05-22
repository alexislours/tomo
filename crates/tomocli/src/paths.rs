use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

pub(crate) fn unnamed_entry(index: usize) -> String {
    format!("unnamed_{index:04}.bin")
}

pub(crate) fn append_ext(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

pub(crate) fn strip_ext(input: &Path, exts: &[&str]) -> Result<PathBuf> {
    match input.extension().and_then(|e| e.to_str()) {
        Some(ext) if exts.iter().any(|e| ext.eq_ignore_ascii_case(e)) => {
            Ok(input.with_extension(""))
        }
        _ => {
            let list = exts
                .iter()
                .map(|e| format!("`.{e}`"))
                .collect::<Vec<_>>()
                .join(" or ");
            bail!(
                "cannot derive output path: `{}` does not end in {list}. Pass --out",
                input.display()
            )
        }
    }
}

pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("read `{}`", path.display()))
}

pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("write `{}`", path.display()))
}

pub(crate) fn sanitize_relative(name: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => bail!("path traversal in entry: {name}"),
            other => out.push(other),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("empty entry name");
    }
    for comp in out.components() {
        if !matches!(comp, Component::Normal(_)) {
            bail!("unsafe component in entry: {name}");
        }
    }
    Ok(out)
}
