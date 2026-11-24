//! Update KernelSU preinit copies that Android will read on the next boot.
//!
//! `ksud` rebuilds these copies from the running kernel. A recovery kernel
//! that does not implement preinit deletes the copy instead of refreshing it,
//! so recovery never calls `ksud` for this. Existing directories are updated
//! in place. A missing directory is left missing: creating one would pick a
//! location from the recovery kernel.

use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const HOSTS: &[&str] = &["vold/ksu", "watchdog/ksu", "ksu"];
const LIMIT: u64 = 4 << 20;
const RESERVE: u64 = 8 << 20;
const BLOCK: u64 = 4096;

#[cfg(target_os = "android")]
pub fn sync_existing() -> Result<()> {
    sync_existing_at(Path::new("/data/adb/modules"), Path::new("/metadata"))
}

fn sync_existing_at(modules_root: &Path, metadata: &Path) -> Result<()> {
    if metadata.symlink_metadata().is_err() {
        return Ok(());
    }
    ensure!(
        !metadata.symlink_metadata()?.file_type().is_symlink(),
        "metadata path is a symlink"
    );
    let modules = next_boot_modules(modules_root)?;
    let adb = modules_root
        .parent()
        .context("module directory has no parent")?;
    for relative in HOSTS {
        let host = metadata.join(relative);
        if !existing_host(&host)? {
            continue;
        }
        refresh_host(&host, adb, &modules)?;
    }
    Ok(())
}

fn existing_host(host: &Path) -> Result<bool> {
    let Ok(meta) = host.symlink_metadata() else {
        return Ok(false);
    };
    ensure!(
        !meta.file_type().is_symlink(),
        "KernelSU preinit path is a symlink"
    );
    if !meta.is_dir() {
        return Ok(false);
    }
    Ok(host
        .join("modules.rc")
        .symlink_metadata()
        .is_ok_and(|meta| meta.is_file())
        || host
            .join("preinit")
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_dir()))
}

fn next_boot_modules(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut found: BTreeMap<String, Option<PathBuf>> = BTreeMap::new();
    for directory in [root.with_file_name("modules_update"), root.to_path_buf()] {
        let entries = match directory.symlink_metadata() {
            Ok(meta) if meta.is_dir() => fs::read_dir(&directory)?,
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let Ok(id) = entry.file_name().into_string() else {
                continue;
            };
            if !module_id(&id) {
                continue;
            }
            let module = directory.join(&id);
            let Ok(meta) = module.symlink_metadata() else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }
            if module.join("disable").exists() || module.join("remove").exists() {
                found.insert(id, None);
                continue;
            }
            found.entry(id).or_insert(Some(module));
        }
    }
    Ok(found
        .into_iter()
        .filter_map(|(id, path)| path.map(|path| (id, path)))
        .collect())
}

fn module_id(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('.')
        && id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.'
        })
}

fn refresh_host(host: &Path, adb: &Path, modules: &BTreeMap<String, PathBuf>) -> Result<()> {
    let mut rc = Vec::new();
    append_rc(&adb.join("initrc.d"), None, true, &mut rc)?;
    for (id, module) in modules {
        append_rc(&module.join("initrc"), Some(id), false, &mut rc)?;
    }
    write_file(&host.join("modules.rc"), &rc)?;

    let mut candidates = Vec::new();
    for (id, module) in modules {
        let source = module.join("preinit");
        if source
            .join("preinit.conf")
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_file())
        {
            candidates.push((id, source));
        }
    }
    let preinit = host.join("preinit");
    let present = preinit.symlink_metadata().is_ok_and(|meta| meta.is_dir());
    if candidates.is_empty() {
        if present {
            remove_tree(&preinit)?;
            let _ = fs::remove_file(host.join(".preinit_bootcount"));
        }
        return Ok(());
    }

    fs::create_dir_all(&preinit)?;
    tighten(&preinit, 0o755);
    let keep: Vec<&str> = candidates.iter().map(|(id, _)| id.as_str()).collect();
    let mut changed = remove_unlisted(&preinit, &keep)?;
    let mut used = BLOCK;
    for (id, source) in &candidates {
        let need = tree_usage(source)?;
        used = used.saturating_add(need);
        ensure!(used <= LIMIT, "KernelSU preinit copy of {id} exceeds 4 MiB");
        let destination = preinit.join(id);
        if same_tree(source, &destination)? {
            continue;
        }
        let room = available(host)?.saturating_sub(RESERVE);
        ensure!(
            need <= room,
            "not enough space on /metadata for KernelSU preinit module {id}"
        );
        install_tree(&preinit, id, source)?;
        changed = true;
    }
    if changed {
        let _ = fs::remove_file(preinit.join(".manifest"));
        let _ = fs::remove_file(host.join(".preinit_bootcount"));
    }
    let _ = File::open(&preinit).and_then(|file| file.sync_all());
    Ok(())
}

fn append_rc(dir: &Path, id: Option<&str>, executable_only: bool, out: &mut Vec<u8>) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut paths: Vec<_> = entries.collect::<io::Result<Vec<_>>>()?;
    paths.sort_by_key(fs::DirEntry::file_name);
    for entry in paths {
        let path = entry.path();
        let meta = path.symlink_metadata()?;
        if meta.file_type().is_symlink()
            || !meta.is_file()
            || path.extension().is_none_or(|ext| ext != "rc")
        {
            continue;
        }
        if executable_only && meta.permissions().mode() & 0o111 == 0 {
            continue;
        }
        match id {
            Some(id) => writeln!(out, "# === from {id}:{} ===", path.display())?,
            None => writeln!(out, "# === from {} ===", path.display())?,
        }
        out.extend(fs::read(&path).with_context(|| format!("read {}", path.display()))?);
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        out.push(b'\n');
    }
    Ok(())
}

fn remove_unlisted(preinit: &Path, keep: &[&str]) -> Result<bool> {
    let mut changed = false;
    for entry in fs::read_dir(preinit)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            remove_tree(&entry.path())?;
            changed = true;
            continue;
        };
        if name == ".manifest" || keep.contains(&name) {
            continue;
        }
        remove_tree(&entry.path())?;
        changed = true;
    }
    Ok(changed)
}

fn install_tree(preinit: &Path, id: &str, source: &Path) -> Result<()> {
    let temporary = preinit.join(format!(".{id}.tmp"));
    if temporary.symlink_metadata().is_ok() {
        remove_tree(&temporary)?;
    }
    copy_dir(source, &temporary)?;
    let destination = preinit.join(id);
    let previous = preinit.join(format!(".{id}.old"));
    if destination.symlink_metadata().is_ok() {
        if previous.symlink_metadata().is_ok() {
            remove_tree(&previous)?;
        }
        fs::rename(&destination, &previous)?;
    }
    fs::rename(&temporary, &destination)?;
    if previous.symlink_metadata().is_ok() {
        remove_tree(&previous)?;
    }
    Ok(())
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    let meta = source.symlink_metadata()?;
    ensure!(meta.is_dir(), "KernelSU preinit entry is not a directory");
    fs::create_dir(destination)?;
    tighten(destination, 0o755);
    let mut entries: Vec<_> = fs::read_dir(source)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let from = entry.path();
        let meta = from.symlink_metadata()?;
        ensure!(
            !meta.file_type().is_symlink(),
            "KernelSU preinit contains a symlink"
        );
        let to = destination.join(entry.file_name());
        if meta.is_dir() {
            copy_dir(&from, &to)?;
        } else if meta.is_file() {
            fs::copy(&from, &to)?;
            let _ = File::open(&to).and_then(|file| file.sync_all());
            tighten(&to, meta.permissions().mode() & 0o755);
        } else {
            anyhow::bail!("unsupported KernelSU preinit file");
        }
    }
    Ok(())
}

fn same_tree(source: &Path, destination: &Path) -> Result<bool> {
    let Ok(dest) = destination.symlink_metadata() else {
        return Ok(false);
    };
    if dest.file_type().is_symlink() || !dest.is_dir() {
        return Ok(false);
    }
    let mut left = Vec::new();
    let mut right = Vec::new();
    list_files(source, Path::new(""), &mut left)?;
    list_files(destination, Path::new(""), &mut right)?;
    if left != right {
        return Ok(false);
    }
    for relative in &left {
        let from = source.join(relative);
        if from.symlink_metadata()?.is_file()
            && fs::read(&from)? != fs::read(destination.join(relative))?
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn list_files(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(root.join(relative))?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let relative = relative.join(&name);
        let meta = entry.path().symlink_metadata()?;
        ensure!(
            !meta.file_type().is_symlink(),
            "KernelSU preinit contains a symlink"
        );
        if meta.is_dir() {
            out.push(relative.clone());
            list_files(root, &relative, out)?;
        } else if meta.is_file() {
            out.push(relative);
        }
    }
    Ok(())
}

fn tree_usage(root: &Path) -> Result<u64> {
    fn walk(path: &Path, total: &mut u64) -> Result<()> {
        let meta = path.symlink_metadata()?;
        ensure!(
            !meta.file_type().is_symlink(),
            "KernelSU preinit contains a symlink"
        );
        *total = total.saturating_add(BLOCK);
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                walk(&entry?.path(), total)?;
            }
        } else if meta.is_file() {
            *total = total.saturating_add(meta.len().div_ceil(BLOCK) * BLOCK);
        }
        Ok(())
    }
    let mut total = 0;
    walk(root, &mut total)?;
    Ok(total)
}

fn available(path: &Path) -> Result<u64> {
    let text = CString::new(path.as_os_str().as_bytes()).context("metadata path contains NUL")?;
    let mut stat = unsafe { std::mem::zeroed::<libc::statvfs>() };
    if unsafe { libc::statvfs(text.as_ptr(), &mut stat) } != 0 {
        return Err(io::Error::last_os_error()).context("check metadata free space");
    }
    Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        anyhow::bail!("refusing to write through symlink {}", path.display());
    }
    let temporary = path.with_file_name(".modules.rc.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)?;
    tighten(path, 0o644);
    if let Some(parent) = path.parent() {
        let _ = File::open(parent).and_then(|file| file.sync_all());
    }
    Ok(())
}

fn tighten(path: &Path, mode: u32) {
    let Ok(text) = CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    unsafe { libc::lchown(text.as_ptr(), 0, 0) };
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

fn remove_tree(path: &Path) -> Result<()> {
    let meta = path.symlink_metadata()?;
    if meta.file_type().is_symlink() || meta.is_file() {
        fs::remove_file(path)?;
    } else {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::sync_existing_at;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn layout(root: &Path) {
        for relative in [
            "adb/modules/alpha/preinit",
            "adb/modules/alpha/initrc",
            "adb/initrc.d",
            "metadata/vold/ksu/preinit/alpha",
            "metadata/watchdog/ksu",
        ] {
            fs::create_dir_all(root.join(relative)).unwrap();
        }
        fs::write(root.join("adb/modules/alpha/module.prop"), "id=alpha\n").unwrap();
        fs::write(
            root.join("adb/modules/alpha/preinit/preinit.conf"),
            "alpha-conf\n",
        )
        .unwrap();
        fs::write(root.join("adb/modules/alpha/initrc/alpha.rc"), "on boot\n").unwrap();
        let common = root.join("adb/initrc.d/common.rc");
        fs::write(&common, "on early\n").unwrap();
        let mut permissions = fs::metadata(&common).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&common, permissions).unwrap();
        fs::write(root.join("adb/initrc.d/skipped.rc"), "ignored\n").unwrap();
        fs::write(root.join("metadata/vold/ksu/modules.rc"), "old\n").unwrap();
        fs::write(
            root.join("metadata/vold/ksu/preinit/alpha/preinit.conf"),
            "stale\n",
        )
        .unwrap();
        fs::write(root.join("metadata/vold/ksu/.preinit_bootcount"), "2\n").unwrap();
        fs::write(
            root.join("metadata/watchdog/ksu/modules.rc"),
            "watchdog-old\n",
        )
        .unwrap();
    }

    #[test]
    fn refreshing_keeps_an_enabled_module_and_updates_every_existing_host() {
        let root = tempfile::tempdir().unwrap();
        layout(root.path());
        sync_existing_at(
            &root.path().join("adb/modules"),
            &root.path().join("metadata"),
        )
        .unwrap();
        let rc = fs::read_to_string(root.path().join("metadata/vold/ksu/modules.rc")).unwrap();
        assert!(rc.contains("on early"));
        assert!(rc.contains("on boot"));
        assert!(!rc.contains("ignored"));
        assert_eq!(
            fs::read(
                root.path()
                    .join("metadata/vold/ksu/preinit/alpha/preinit.conf")
            )
            .unwrap(),
            b"alpha-conf\n"
        );
        assert!(
            !root
                .path()
                .join("metadata/vold/ksu/.preinit_bootcount")
                .exists()
        );
        let watchdog =
            fs::read_to_string(root.path().join("metadata/watchdog/ksu/modules.rc")).unwrap();
        assert!(watchdog.contains("on boot"));
        assert!(!root.path().join("metadata/ksu").exists());
        fs::write(
            root.path().join("metadata/vold/ksu/.preinit_bootcount"),
            "1\n",
        )
        .unwrap();
        sync_existing_at(
            &root.path().join("adb/modules"),
            &root.path().join("metadata"),
        )
        .unwrap();
        assert_eq!(
            fs::read(root.path().join("metadata/vold/ksu/.preinit_bootcount")).unwrap(),
            b"1\n"
        );
    }

    #[test]
    fn disabling_removes_the_preinit_copy_without_deleting_the_host() {
        let root = tempfile::tempdir().unwrap();
        layout(root.path());
        fs::write(root.path().join("adb/modules/alpha/disable"), "").unwrap();
        sync_existing_at(
            &root.path().join("adb/modules"),
            &root.path().join("metadata"),
        )
        .unwrap();
        assert!(!root.path().join("metadata/vold/ksu/preinit").exists());
        assert!(root.path().join("metadata/vold/ksu").is_dir());
        let rc = fs::read_to_string(root.path().join("metadata/vold/ksu/modules.rc")).unwrap();
        assert!(!rc.contains("alpha"));
        assert!(root.path().join("metadata/watchdog/ksu").is_dir());
    }

    #[test]
    fn an_installed_removal_mark_suppresses_the_staged_copy() {
        let root = tempfile::tempdir().unwrap();
        layout(root.path());
        fs::create_dir_all(root.path().join("adb/modules_update/alpha/preinit")).unwrap();
        fs::write(
            root.path()
                .join("adb/modules_update/alpha/preinit/preinit.conf"),
            "staged\n",
        )
        .unwrap();
        fs::write(root.path().join("adb/modules/alpha/remove"), "").unwrap();
        sync_existing_at(
            &root.path().join("adb/modules"),
            &root.path().join("metadata"),
        )
        .unwrap();
        assert!(!root.path().join("metadata/vold/ksu/preinit").exists());
    }

    #[test]
    fn no_existing_host_is_not_created() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("adb/modules/alpha/preinit")).unwrap();
        fs::write(
            root.path().join("adb/modules/alpha/preinit/preinit.conf"),
            "alpha\n",
        )
        .unwrap();
        fs::create_dir_all(root.path().join("metadata")).unwrap();
        sync_existing_at(
            &root.path().join("adb/modules"),
            &root.path().join("metadata"),
        )
        .unwrap();
        assert!(!root.path().join("metadata/vold").exists());
        assert!(!root.path().join("metadata/ksu").exists());
    }

    #[test]
    fn symlinks_are_not_followed() {
        let root = tempfile::tempdir().unwrap();
        layout(root.path());
        let outside = root.path().join("outside");
        fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(
            &outside,
            root.path().join("adb/modules/alpha/preinit/linked"),
        )
        .unwrap();
        let error = sync_existing_at(
            &root.path().join("adb/modules"),
            &root.path().join("metadata"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("symlink"), "{error}");
        assert_eq!(fs::read(&outside).unwrap(), b"secret");
        assert!(
            !root
                .path()
                .join("metadata/vold/ksu/preinit/alpha/linked")
                .exists()
        );
    }

    #[test]
    fn missing_metadata_leaves_module_state_alone() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("adb/modules/alpha")).unwrap();
        sync_existing_at(
            &root.path().join("adb/modules"),
            &root.path().join("metadata"),
        )
        .unwrap();
    }
}
