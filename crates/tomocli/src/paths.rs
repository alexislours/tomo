use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};

static FORCE: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_force(force: bool) {
    FORCE.store(force, Ordering::Relaxed);
}

fn force() -> bool {
    FORCE.load(Ordering::Relaxed)
}

pub(crate) fn create_overwrite(path: &Path) -> Result<File> {
    File::create(path).with_context(|| format!("create `{}`", path.display()))
}

pub(crate) fn create(path: &Path) -> Result<File> {
    if force() {
        return create_overwrite(path);
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(f) => Ok(f),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => bail!(
            "`{}` already exists; pass --force to overwrite",
            path.display()
        ),
        Err(e) => Err(e).with_context(|| format!("create `{}`", path.display())),
    }
}

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
    let mut f = create(path)?;
    f.write_all(bytes)
        .with_context(|| format!("write `{}`", path.display()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("tomo-create-{}-{nanos}.tmp", std::process::id()))
    }

    #[test]
    fn create_refuses_existing_without_force() {
        let path = unique_path();
        write_file(&path, b"first").unwrap();
        let err = create(&path).unwrap_err().to_string();
        assert!(err.contains("already exists"), "got: {err}");
        assert!(err.contains("--force"), "got: {err}");
        fs::remove_file(&path).ok();
    }
}
