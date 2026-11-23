//! Bring up hardware using the Android product's fstab and module order.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const MODULE_LISTS: &str = "/skyward/modules";

pub fn prepare_devices() {
    if let Err(e) = mount_firmware() {
        eprintln!("Android firmware mount discovery: {e:#}");
    }
    if let Err(e) = load_modules() {
        eprintln!("Android kernel module discovery: {e:#}");
    }
}

fn mount_firmware() -> Result<()> {
    use skyward_block::{fstab, mount};
    for entry in fstab::table()? {
        let target = Path::new(&entry.mount_point);
        // The board sets the mount paths used by vendor drivers and ueventd.
        // Only import read-only physical filesystems below vendor or odm.
        if !entry.options.iter().any(|option| option == "ro")
            || !entry.device.starts_with("/dev/block/")
            || !(target.starts_with("/vendor/") || target.starts_with("/odm/"))
            || target == Path::new("/vendor") || target == Path::new("/odm")
            || !target.is_dir() || mount::is_mount_point(target)?
        {
            continue;
        }
        let result = fstab::device(&entry)
            .and_then(|device| mount::mount(Path::new(&device), target, &entry.filesystem, &entry.options));
        if let Err(e) = result {
            eprintln!("Android mount {}: {e:#}", target.display());
        }
    }
    Ok(())
}

fn load_modules() -> Result<()> {
    let root = Path::new(MODULE_LISTS);
    if !root.is_dir() {
        return Ok(());
    }
    let mut lists = Vec::new();
    collect_lists(root, &mut lists)?;
    lists.sort_by(|a, b| {
        let priority = |p: &Path| {
            let p = p.strip_prefix(root).unwrap_or(p);
            if p.starts_with("system") || p.starts_with("system_dlkm") { 0 }
            else if p.starts_with("vendor") || p.starts_with("vendor_dlkm") { 1 }
            else { 2 }
        };
        priority(a).cmp(&priority(b)).then_with(|| a.cmp(b))
    });
    let mut dirs = vec![PathBuf::from("/lib/modules")];
    let mut modules = Vec::new();
    for list in lists {
        let relative = list.strip_prefix(root)?;
        let partition = relative.components().next().context("module list without a partition")?;
        let partition = partition.as_os_str().to_str().context("invalid module partition")?;
        let mount = skyward_vab::mount_point(partition);
        let path = relative.parent().context("module list without a directory")?.strip_prefix(partition)?;
        let mut dir = mount.join(path);
        if partition == "system" && !dir.is_dir() {
            dir = mount.join("system").join(path);
        }
        if !dir.join("modules.dep").is_file() {
            continue;
        }
        dirs.push(dir);
        for line in fs::read_to_string(&list)?.lines() {
            let module = line.split('#').next().unwrap_or_default().trim();
            if !module.is_empty() {
                modules.push(module.to_owned());
            }
        }
    }
    if !modules.is_empty() {
        let mut cmd = Command::new("/system/bin/modprobe");
        for dir in dirs {
            cmd.arg("-d").arg(dir);
        }
        let status = cmd.args(["-a", "-b", "--"]).args(modules).status().context("run Android modprobe")?;
        if !status.success() {
            eprintln!("some Android kernel modules did not load: {status}");
        }
    }
    Ok(())
}

fn collect_lists(dir: &Path, lists: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_lists(&entry.path(), lists)?;
        } else if entry.file_name() == "modules.load" {
            lists.push(entry.path());
        }
    }
    Ok(())
}
