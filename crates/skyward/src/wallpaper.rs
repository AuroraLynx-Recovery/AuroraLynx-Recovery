//! Keep only normalized copies of the owner's wallpaper in recovery storage.

use anyhow::{Context, Result, ensure};
use skyward_ui::wallpaper::{MAX_BYTES, Wallpaper};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, ErrorKind};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const DIRECTORY: &str = "/data/unencrypted/skyward";

fn asset(slot: bool) -> PathBuf {
    Path::new(DIRECTORY).join(if slot { "wallpaper-b.jpg" } else { "wallpaper-a.jpg" })
}

fn open(path: &Path) -> Result<File> {
    let file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let meta = file.metadata()?;
    ensure!(meta.is_file() && meta.len() <= MAX_BYTES, "wallpaper must be a regular file no larger than 24 MiB");
    Ok(file)
}

pub fn read(source: &Path, root: &Path) -> Result<Wallpaper> {
    let root = root.canonicalize().context("open the storage root")?;
    let file = open(source)?;
    let file_path = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).context("locate opened wallpaper")?;
    ensure!(file_path.starts_with(&root), "wallpaper is outside the selected storage");
    Wallpaper::read(BufReader::new(file))
}

pub fn load(slot: bool) -> Result<Wallpaper> {
    Wallpaper::read(BufReader::new(open(&asset(slot))?))
}

pub fn save(slot: bool, picture: &Wallpaper) -> Result<()> {
    let directory = Path::new(DIRECTORY);
    if let Err(error) = fs::DirBuilder::new().mode(0o700).create(directory)
        && error.kind() != ErrorKind::AlreadyExists
    {
        return Err(error).context("create the wallpaper directory");
    }
    ensure!(directory.symlink_metadata()?.is_dir(), "the wallpaper directory is not a directory");
    let next = directory.join("wallpaper.new");
    remove(&next)?;
    let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&next)?;
    let result = (|| -> Result<()> {
        picture.write(&file)?;
        file.sync_all()?;
        fs::rename(&next, asset(slot))?;
        File::open(directory)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = remove(&next);
    }
    result
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

pub fn forget(slot: bool) -> Result<()> {
    remove(&asset(slot))
}
