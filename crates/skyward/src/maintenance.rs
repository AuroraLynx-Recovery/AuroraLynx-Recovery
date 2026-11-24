use anyhow::{ensure, Context, Result};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
        && name != "." && name != ".."
}

/// A partition that can be selected in the extractor.
struct ExportSource {
    name: String,
    path: Option<PathBuf>,
    size: u64,
}

fn export_sources(slot: Option<skyward_vab::Slot>) -> Result<Vec<ExportSource>> {
    let mut catalog = Vec::new();
    for part in skyward_block::list_by_name(skyward_block::BY_NAME)? {
        if valid_name(&part.name) && let Ok(size) = skyward_block::size_bytes(&part.path) && size != 0 {
            catalog.push(ExportSource { name: part.name, path: Some(part.path), size });
        }
    }
    // Physical partition discovery still works if super is absent or damaged.
    if let Some(slot) = slot && let Ok(entries) = skyward_vab::slot_sizes(slot) {
        for (name, size) in entries {
            if size != 0 && valid_name(&name) && !catalog.iter().any(|source| source.name == name) {
                catalog.push(ExportSource { name, path: None, size });
            }
        }
    }
    catalog.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(catalog)
}

pub fn partition_catalog() -> Result<Vec<(String, u64)>> {
    Ok(export_sources(skyward_vab::current_slot()?)?.into_iter().map(|source| (source.name, source.size)).collect())
}

pub fn list_partitions() -> Result<()> {
    for source in export_sources(skyward_vab::current_slot()?)? {
        println!("{}\t{}", source.name, source.size);
    }
    Ok(())
}

pub fn extract(output: &Path, names: &[String]) -> Result<()> {
    ensure!(!names.is_empty(), "choose at least one partition");
    ensure!(output.is_dir(), "choose an existing output directory");
    let output = output.canonicalize().context("resolve extraction directory")?;
    let usb = Path::new(skyward_ui::app::USB_DRIVE);
    if output.starts_with(usb) {
        ensure!(skyward_block::mount::is_mount_point(usb)?, "the USB drive is no longer mounted");
    }
    let slot = skyward_vab::current_slot()?;
    let catalog = export_sources(slot)?;
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    let mut needed = 0u64;
    let limit = image_file_limit(&output)?;
    for name in names {
        ensure!(valid_name(name) && seen.insert(name), "invalid or duplicate partition {name:?}");
        let filename = output.join(format!("{name}.img"));
        match filename.symlink_metadata() {
            Ok(_) => anyhow::bail!("{} already exists", filename.display()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).with_context(|| format!("check {}", filename.display())),
        }
        let source = catalog.iter().find(|source| &source.name == name)
            .with_context(|| format!("unknown or unreadable partition {name}"))?;
        ensure!(source.size <= limit, "{name}.img needs {} bytes, above the destination's {limit}-byte file limit", source.size);
        needed = needed.checked_add(source.size).context("partition size total overflow")?;
        sources.push((name, filename, source));
    }
    if sources.iter().any(|(name, _, source)| source.path.is_none() || name.as_str() == "super") {
        ensure!(!skyward_vab::merge_outstanding()?, "a Virtual A/B update may still be using super");
    }
    let available = skyward_block::mount::space(&output)?.available;
    ensure!(needed <= available, "extraction needs {needed} bytes, but only {available} are available");
    for (name, filename, source) in sources {
        let mapping = if source.path.is_none() {
            Some(skyward_vab::map_whole(slot.context("no active slot")?, name, &format!("skyward-extract-{}", std::process::id()), true)?)
        } else {
            None
        };
        let path = source.path.as_deref().or_else(|| mapping.as_ref().map(|m| m.device.as_path())).context("missing partition source")?;
        let result = copy_image(path, &filename);
        let cleanup = mapping.as_ref().map(skyward_vab::unmap_whole).transpose();
        result?;
        cleanup?;
        println!("{}", filename.display());
    }
    Ok(())
}

fn image_file_limit(output: &Path) -> Result<u64> {
    const MSDOS_SUPER_MAGIC: u64 = 0x4d44;
    let path = CString::new(output.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    if unsafe { libc::statfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error()).with_context(|| format!("inspect filesystem at {}", output.display()));
    }
    let filesystem = unsafe { stats.assume_init() }.f_type;
    Ok(if filesystem as u64 == MSDOS_SUPER_MAGIC { u64::from(u32::MAX) } else { u64::MAX })
}

fn copy_image(input: &Path, output: &Path) -> Result<()> {
    let bytes = skyward_block::size_bytes(input)?;
    let mut input = File::open(input)?.take(bytes);
    let partial = output.with_extension(format!("img.{}.part", std::process::id()));
    let mut file = skyward_files::make_file(&partial, crate::ui::ownership(output.parent().context("missing output directory")?)?)?;
    let result = (|| -> Result<()> {
        ensure!(io::copy(&mut input, &mut file)? == bytes, "partition read ended early");
        file.sync_all()?;
        publish_image(&partial, output)
    })();
    if let Err(error) = result {
        return match fs::remove_file(&partial) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(error.context(format!("remove {}: {cleanup}", partial.display()))),
        };
    }
    Ok(())
}

/// Rename on the same filesystem without overwriting, including on vfat and exFAT.
fn publish_image(partial: &Path, output: &Path) -> Result<()> {
    let from = CString::new(partial.as_os_str().as_bytes())?;
    let to = CString::new(output.as_os_str().as_bytes())?;
    let status = unsafe {
        libc::syscall(libc::SYS_renameat2, libc::AT_FDCWD, from.as_ptr(), libc::AT_FDCWD, to.as_ptr(), libc::RENAME_NOREPLACE)
    };
    if status != 0 {
        return Err(io::Error::last_os_error()).with_context(|| format!("publish {}", output.display()));
    }
    Ok(())
}

#[derive(clap::Subcommand)]
pub enum ModuleAction {
    List,
    /// Install a module ZIP through the selected installed manager.
    Install { #[arg(value_enum)] manager: RootManager, zip: PathBuf },
    /// Show the installed module metadata and its current boot flags.
    Info { id: String },
    Enable { id: String },
    Disable { id: String },
    Remove { id: String },
    CancelRemoval { id: String },
}

pub fn modules(action: ModuleAction) -> Result<()> {
    let root = Path::new("/data/adb/modules");
    ensure!(skyward_block::mount::is_mount_point(Path::new("/data"))?, "open /data first");
    if let ModuleAction::Install { manager, zip } = action {
        return install_module(manager, &zip);
    }
    let (id, flag, set) = match action {
        ModuleAction::Install { .. } => unreachable!("install handled before opening module directory"),
        ModuleAction::Info { id } => return module_info(root, &id),
        ModuleAction::List => {
            for module in module_catalog_at(root)? {
                println!("{}\t{}", module.id, module_state(&module));
            }
            return Ok(());
        }
        ModuleAction::Enable { id } => (id, "disable", false),
        ModuleAction::Disable { id } => (id, "disable", true),
        ModuleAction::Remove { id } => (id, "remove", true),
        ModuleAction::CancelRemoval { id } => (id, "remove", false),
    };
    let module = module_path(root, &id)?;
    let marker = module.join(flag);
    if flag == "remove" {
        let ap = root.with_file_name("ap");
        if root.with_file_name("apd").is_file() && ap.symlink_metadata().is_ok_and(|metadata| metadata.is_dir()) {
            // APatch's native uninstall and undo operations also notify its
            // boot-time module updater through /data/adb/ap/update.
            if !set {
                let staging = root.with_file_name("modules_update");
                let pending = staging.join(&id);
                if staging.symlink_metadata().is_ok_and(|metadata| metadata.is_dir())
                    && pending.symlink_metadata().is_ok_and(|metadata| metadata.is_dir())
                {
                    // APatch can activate the pending tree directly at boot.
                    set_module_marker(&pending.join("remove"), false)?;
                }
            }
            set_module_marker(&ap.join("update"), true)?;
        }
    }
    set_module_marker(&marker, set)
}

fn set_module_marker(flag: &Path, set: bool) -> Result<()> {
    match flag.symlink_metadata() {
        Ok(meta) => {
            ensure!(meta.is_file(), "invalid module marker {}", flag.display());
            if !set {
                fs::remove_file(flag)?;
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if set {
                let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(flag)?;
                file.sync_all()?;
            }
        }
        Err(error) => return Err(error.into()),
    }
    File::open(flag.parent().context("module marker has no parent")?)?.sync_all()?;
    Ok(())
}

fn module_path(root: &Path, id: &str) -> Result<PathBuf> {
    ensure!(valid_name(id), "invalid module ID");
    let module = root.join(id);
    ensure!(module.symlink_metadata()?.is_dir() && module.join("module.prop").symlink_metadata()?.is_file(), "not an installed module");
    Ok(module)
}

fn module_state(module: &skyward_ui::app::RootModule) -> &'static str {
    match (module.installed, module.removing, module.disabled) {
        (false, _, _) => "pending installation",
        (_, true, _) => "pending removal",
        (_, _, true) => "disabled",
        _ => "enabled",
    }
}

pub fn module_catalog() -> Result<Vec<skyward_ui::app::RootModule>> {
    ensure!(skyward_block::mount::is_mount_point(Path::new("/data"))?, "open /data first");
    module_catalog_at(Path::new("/data/adb/modules"))
}

fn module_catalog_at(root: &Path) -> Result<Vec<skyward_ui::app::RootModule>> {
    use skyward_ui::app::RootModule;
    let mut modules = Vec::new();
    let staging = root.with_file_name("modules_update");
    for (directory, installed) in [(root, true), (staging.as_path(), false)] {
        match directory.symlink_metadata() {
            Ok(metadata) if !installed && !metadata.is_dir() => continue,
            Ok(metadata) => ensure!(metadata.is_dir(), "module directory is unavailable"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let Ok(id) = entry.file_name().into_string() else { continue };
            let Ok(module) = module_path(directory, &id) else { continue };
            if !installed && modules.iter().any(|entry: &RootModule| entry.id == id) {
                continue;
            }
            let (properties, metadata_error) = match module_properties(&module) {
                Ok(properties) => (properties, None),
                Err(error) => (Vec::new(), Some(format!("{error:#}"))),
            };
            modules.push(RootModule {
                properties,
                metadata_error,
                installed,
                disabled: installed && module.join("disable").symlink_metadata().is_ok(),
                removing: installed && module.join("remove").symlink_metadata().is_ok(),
                pending_update: !installed || pending_update(root, &module, &id),
                id,
            });
        }
    }
    modules.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(modules)
}

fn pending_update(root: &Path, module: &Path, id: &str) -> bool {
    if module.join("update").symlink_metadata().is_ok_and(|meta| meta.is_file()) {
        return true;
    }
    let staging = root.with_file_name("modules_update");
    staging.symlink_metadata().is_ok_and(|meta| meta.is_dir()) && module_path(&staging, id).is_ok()
}

fn module_info(root: &Path, id: &str) -> Result<()> {
    ensure!(valid_name(id), "invalid module ID");
    let module = module_catalog_at(root)?.into_iter().find(|module| module.id == id).context("unknown module ID")?;
    println!("id={id}");
    println!("state={}", module_state(&module));
    println!("pending_update={}", module.pending_update);
    if let Some(error) = module.metadata_error {
        println!("metadata_error={error}");
    }
    for (key, value) in module.properties {
        println!("{key}={value}");
    }
    Ok(())
}

fn module_properties(module: &Path) -> Result<Vec<(String, String)>> {
    let property = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(module.join("module.prop"))?;
    let mut contents = String::new();
    property.take(65537).read_to_string(&mut contents)?;
    ensure!(contents.len() <= 65536, "module metadata exceeds 64 KiB");
    let mut properties = Vec::new();
    for key in ["name", "version", "versionCode", "author", "description"] {
        if let Some(value) = contents.lines().filter_map(|line| line.split_once('='))
            .find_map(|(name, value)| (name.trim() == key).then_some(value.trim()))
        {
            let value: String = value.chars().filter(|ch| !ch.is_control()).collect();
            properties.push((key.to_string(), value));
        }
    }
    Ok(properties)
}


#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum RootManager {
    Magisk,
    KernelSu,
    APatch,
}

impl RootManager {
    fn program(self) -> &'static str {
        match self {
            Self::Magisk => "/data/adb/magisk/magisk",
            Self::KernelSu => "/data/adb/ksud",
            Self::APatch => "/data/adb/apd",
        }
    }

    fn arguments(self) -> &'static [&'static str] {
        match self {
            Self::Magisk => &["--install-module"],
            Self::KernelSu | Self::APatch => &["module", "install"],
        }
    }
}

fn install_module(manager: RootManager, archive: &Path) -> Result<()> {
    let archive = archive.canonicalize().context("resolve module ZIP")?;
    ensure!(archive.is_file(), "select a module ZIP file");
    let mut zip = zip::ZipArchive::new(File::open(&archive)?).context("open module ZIP")?;
    let prop = zip.by_name("module.prop").context("ZIP does not contain a root-level module.prop")?;
    ensure!(prop.is_file() && prop.size() <= 65536, "invalid module.prop in ZIP");
    let mut properties = String::new();
    prop.take(65537).read_to_string(&mut properties)?;
    ensure!(properties.len() <= 65536, "module metadata exceeds 64 KiB");
    module_zip_id(&properties)?;

    use std::process::Stdio;
    let mut command = module_installer(manager, &archive)?;
    let (mut reader, writer) = io::pipe()?;
    use std::os::fd::AsRawFd;
    // Only the reader is nonblocking. The installer still gets ordinary
    // blocking stdout and stderr.
    if unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error()).context("configure installer diagnostics");
    }
    let stderr = writer.try_clone()?;
    let mut child = command.stdin(Stdio::null())
        .stdout(writer)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("start {manager:?} module installer"))?;
    let (status, output) = installer_output(&mut reader, &mut child)
        .with_context(|| format!("collect {manager:?} module installer result"))?;
    if !output.is_empty() {
        println!("{output}");
    }
    if !status.success() {
        anyhow::bail!("{manager:?} module installer exited with {status}\n{output}");
    }
    Ok(())
}


fn module_zip_id(properties: &str) -> Result<&str> {
    let mut ids = properties.lines().filter_map(|line| line.split_once('=')).filter(|(key, _)| key.trim() == "id");
    let (key, id) = ids.next().context("module.prop has no ID")?;
    ensure!(
        key == "id" && id.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && valid_name(id) && ids.next().is_none(),
        "invalid or duplicate module ID"
    );
    Ok(id)
}

fn module_installer(manager: RootManager, archive: &Path) -> Result<std::process::Command> {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    let magisk_util = Path::new("/data/adb/magisk/util_functions.sh");
    if matches!(manager, RootManager::Magisk) && magisk_util.is_file() {
        // Adapt Magisk's module_installer.sh to the already open /data and
        // the recovery's stdout. The package path is a positional argument.
        let mut command = Command::new("/system/bin/sh");
        command.args(["-c", r#"
            umask 022
            BOOTMODE=false
            OUTFD=1
            ZIPFILE="$1"
            . /data/adb/magisk/util_functions.sh || exit 1
            [ "${MAGISK_VER_CODE:-0}" -ge 20400 ] || {
                echo "Magisk 20.4 or later is required" >&2
                exit 1
            }
            install_module
        "#, "auroralynx-magisk-module"]).arg(archive);
        // Magisk's recovery functions mount and unmount partitions. Their
        // mount changes must stay in the installer's own namespace.
        unsafe {
            command.pre_exec(|| {
                if libc::unshare(libc::CLONE_NEWNS) != 0
                    || libc::mount(
                        std::ptr::null(),
                        c"/".as_ptr(),
                        std::ptr::null(),
                        libc::MS_REC | libc::MS_PRIVATE,
                        std::ptr::null(),
                    ) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        return Ok(command);
    }

    let program = manager.program();
    ensure!(Path::new(program).is_file(), "{manager:?} installer is unavailable ({program})");
    let mut command = Command::new(program);
    command.args(manager.arguments()).arg(archive);
    Ok(command)
}

/// Retain bounded diagnostics while draining both streams so verbose module
/// scripts do not block or allocate memory proportional to their output.
fn installer_output(
    mut reader: impl Read,
    child: &mut std::process::Child,
) -> io::Result<(std::process::ExitStatus, String)> {
    const LIMIT: usize = 4096;
    let mut tail = Vec::with_capacity(LIMIT * 2);
    let mut buffer = [0; LIMIT];
    let mut after_exit = 16;
    let status = loop {
        let status = child.try_wait()?;
        match reader.read(&mut buffer) {
            Ok(0) => break child.wait()?,
            Ok(count) => {
                tail.extend_from_slice(&buffer[..count]);
                if tail.len() > LIMIT {
                    tail.drain(..tail.len() - LIMIT);
                }
                if let Some(status) = status {
                    // A script may have left a descendant with this pipe
                    // open. Read the remaining buffered output with a cap.
                    after_exit -= 1;
                    if after_exit == 0 {
                        break status;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(status) = status {
                    break status;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    };
    let text = String::from_utf8_lossy(&tail).chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect::<String>().trim().to_string();
    Ok((status, text))
}
