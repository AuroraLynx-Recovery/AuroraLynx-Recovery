//! Persistent state for the independent recovery lock, accessible before /data.

use anyhow::{Context, Result, ensure};
use skyward_crypto::recovery::{Kind, RECORD_BYTES, Verifier};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

#[cfg(target_os = "android")]
const FILE: &str = "/metadata/skyward/recovery.lock";

pub struct Lock {
    verifier: Verifier,
    recovery: Option<Verifier>,
    failures: u8,
    next_try: Instant,
}

pub enum Attempt {
    Unlocked,
    Wrong,
    Wait(Duration),
}

impl Lock {
    #[cfg(target_os = "android")]
    pub fn load() -> Result<Option<Self>> {
        crate::settings::with_metadata(|| Self::read(Path::new(FILE)))
    }

    fn read(path: &Path) -> Result<Option<Self>> {
        let file = match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("open {}", path.display())),
        };
        ensure!(file.metadata()?.is_file(), "recovery lock record must be a regular file");
        let mut bytes = Vec::new();
        file.take((2 * RECORD_BYTES + 2) as u64).read_to_end(&mut bytes)?;
        let (&failures, record) = bytes.split_first().context("empty recovery lock record")?;
        let (primary, recovery) = record.split_at_checked(RECORD_BYTES).context("truncated recovery lock record")?;
        let verifier = Verifier::decode(primary)?;
        let recovery = if recovery.is_empty() { None } else { Some(Verifier::decode(recovery)?) };
        ensure!(recovery.as_ref().is_none_or(|code| code.kind() == Kind::Password), "invalid recovery code record");
        Ok(Some(Self { verifier, recovery, failures, next_try: Instant::now() + delay(failures) }))
    }

    pub fn kind(&self) -> Kind {
        self.verifier.kind()
    }

    #[cfg(target_os = "android")]
    pub fn attempt(&mut self, credential: &[u8]) -> Result<Attempt> {
        self.check(credential, |lock| lock.write())
    }

    pub fn has_recovery_code(&self) -> bool {
        self.recovery.is_some()
    }

    #[cfg(target_os = "android")]
    pub fn attempt_recovery_code(&mut self, code: &[u8]) -> Result<Attempt> {
        self.check_with(code, |lock| lock.recovery.as_ref(), |lock| lock.write())
    }

    /// Generate a new rescue code only after verifying the current credential.
    #[cfg(target_os = "android")]
    pub fn issue_recovery_code(&mut self, credential: &[u8]) -> Result<(Attempt, Option<String>)> {
        self.issue_with(credential, |lock| lock.write())
    }

    fn issue_with(&mut self, credential: &[u8], save: impl Fn(&Self) -> Result<()>) -> Result<(Attempt, Option<String>)> {
        let attempt = self.check(credential, &save)?;
        if !matches!(attempt, Attempt::Unlocked) {
            return Ok((attempt, None));
        }
        let mut random = [0u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut random)?;
        let code: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        let verifier = Verifier::new(Kind::Password, code.as_bytes())?;
        let old = self.recovery.replace(verifier);
        if let Err(error) = save(self) {
            self.recovery = old;
            return Err(error);
        }
        Ok((Attempt::Unlocked, Some(code)))
    }

    fn check(&mut self, credential: &[u8], save: impl Fn(&Self) -> Result<()>) -> Result<Attempt> {
        self.check_with(credential, |lock| Some(&lock.verifier), save)
    }

    fn check_with(
        &mut self,
        credential: &[u8],
        verifier: impl Fn(&Self) -> Option<&Verifier>,
        save: impl Fn(&Self) -> Result<()>,
    ) -> Result<Attempt> {
        let wait = self.next_try.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            return Ok(Attempt::Wait(wait));
        }
        // Persist the attempt before deriving a key. An interrupted attempt
        // still counts, and a restart reinstates the delay for that count.
        self.failures = self.failures.saturating_add(1);
        self.next_try = Instant::now() + delay(self.failures);
        save(self)?;
        let matched = verifier(self).map(|key| key.verify(credential)).transpose()?.unwrap_or(false);
        if !matched {
            return Ok(Attempt::Wrong);
        }
        self.failures = 0;
        self.next_try = Instant::now();
        save(self)?;
        Ok(Attempt::Unlocked)
    }

    fn new(kind: Kind, credential: &[u8]) -> Result<Self> {
        Ok(Self { verifier: Verifier::new(kind, credential)?, recovery: None, failures: 0, next_try: Instant::now() })
    }

    #[cfg(target_os = "android")]
    fn write(&self) -> Result<()> {
        write_metadata(|| self.write_to(Path::new(FILE)))
    }

    fn write_to(&self, path: &Path) -> Result<()> {
        let parent = path.parent().context("recovery lock directory missing")?;
        fs::create_dir_all(parent)?;
        let mut stage = tempfile::NamedTempFile::new_in(parent)?;
        stage.write_all(&[self.failures])?;
        stage.write_all(&self.verifier.encode())?;
        if let Some(recovery) = &self.recovery {
            stage.write_all(&recovery.encode())?;
        }
        stage.as_file().sync_all()?;
        stage.persist(path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    }
}

pub enum Change<'a> {
    Set { kind: Kind, credential: &'a [u8] },
    Disable,
}

/// Own the settings screen's copy while it is open so repeated submissions
/// use the same monotonic deadline. A load error must prevent editing.
#[cfg(target_os = "android")]
pub fn change(current: &mut Option<Lock>, previous: &[u8], desired: Change<'_>) -> Result<Attempt> {
    change_with(current, previous, desired, |state| {
        write_metadata(|| save_to(Path::new(FILE), state))
    })
}

fn change_with(
    current: &mut Option<Lock>,
    previous: &[u8],
    desired: Change<'_>,
    save: impl Fn(Option<&Lock>) -> Result<()>,
) -> Result<Attempt> {
    if let Some(lock) = current {
        match lock.check(previous, |lock| save(Some(lock)))? {
            Attempt::Unlocked => {}
            rejected => return Ok(rejected),
        }
    } else {
        ensure!(previous.is_empty(), "there is no recovery credential to verify");
    }
    let replacement = match desired {
        Change::Set { kind, credential } => Some(Lock::new(kind, credential)?),
        Change::Disable if current.is_none() => return Ok(Attempt::Unlocked),
        Change::Disable => None,
    };
    save(replacement.as_ref())?;
    *current = replacement;
    Ok(Attempt::Unlocked)
}

fn save_to(path: &Path, lock: Option<&Lock>) -> Result<()> {
    if let Some(lock) = lock {
        return lock.write_to(path);
    }
    fs::remove_file(path)?;
    File::open(path.parent().context("recovery lock directory missing")?)?.sync_all()?;
    Ok(())
}

#[cfg(target_os = "android")]
fn write_metadata(work: impl FnOnce() -> Result<()>) -> Result<()> {
    crate::settings::with_metadata(|| {
        skyward_vab::metadata_writable(true)?;
        let result = work();
        let remount = skyward_vab::metadata_writable(false);
        result.and(remount)
    })
}

fn delay(failures: u8) -> Duration {
    match failures {
        0..=2 => Duration::ZERO,
        _ => Duration::from_secs((5 << (failures - 3).min(7)).min(300)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_keeps_failed_attempts_and_reinstates_delay_after_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.lock");
        assert!(Lock::read(&path).unwrap().is_none());
        let mut lock = Lock { verifier: Verifier::new(Kind::Pin, b"123456").unwrap(), recovery: None, failures: 2, next_try: Instant::now() };
        let save = |lock: &Lock| lock.write_to(&path);
        assert!(matches!(lock.check(b"123", save).unwrap(), Attempt::Wrong));
        let mut loaded = Lock::read(&path).unwrap().unwrap();
        assert_eq!(loaded.failures, 3);
        assert_eq!(loaded.kind(), Kind::Pin);
        assert!(matches!(loaded.check(b"123456", save).unwrap(), Attempt::Wait(_)));
        loaded.next_try = Instant::now();
        assert!(matches!(loaded.check(b"123456", save).unwrap(), Attempt::Unlocked));
        assert_eq!(Lock::read(&path).unwrap().unwrap().failures, 0);
        fs::write(&path, []).unwrap();
        assert!(Lock::read(&path).is_err());
    }

    #[test]
    fn a_lock_can_only_be_changed_or_disabled_with_the_current_credential() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.lock");
        let save = |state: Option<&Lock>| save_to(&path, state);
        let mut lock = None;
        let set = |credential| Change::Set { kind: Kind::Pin, credential };
        assert!(matches!(change_with(&mut lock, b"", set(b"123456"), save).unwrap(), Attempt::Unlocked));
        assert!(matches!(change_with(&mut lock, b"", Change::Disable, save).unwrap(), Attempt::Wrong));
        assert!(Lock::read(&path).unwrap().is_some());
        assert!(matches!(change_with(&mut lock, b"123456", set(b"654321"), save).unwrap(), Attempt::Unlocked));
        assert!(matches!(change_with(&mut lock, b"123456", Change::Disable, save).unwrap(), Attempt::Wrong));
        assert!(matches!(change_with(&mut lock, b"654321", Change::Disable, save).unwrap(), Attempt::Unlocked));
        assert!(Lock::read(&path).unwrap().is_none());
    }


    #[test]
    fn recovery_code_is_authenticated_persisted_and_uses_the_lock_counter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.lock");
        let save = |lock: &Lock| lock.write_to(&path);
        let mut lock = Lock::new(Kind::Pin, b"123456").unwrap();
        assert!(matches!(lock.issue_with(b"", save).unwrap(), (Attempt::Wrong, None)));
        let (_, code) = lock.issue_with(b"123456", save).unwrap();
        let code = code.unwrap();
        assert_eq!(code.len(), 32);
        let mut loaded = Lock::read(&path).unwrap().unwrap();
        assert!(loaded.has_recovery_code());
        assert!(matches!(loaded.check_with(b"wrong", |lock| lock.recovery.as_ref(), save).unwrap(), Attempt::Wrong));
        assert_eq!(Lock::read(&path).unwrap().unwrap().failures, 1);
        assert!(matches!(loaded.check_with(code.as_bytes(), |lock| lock.recovery.as_ref(), save).unwrap(), Attempt::Unlocked));
        assert_eq!(Lock::read(&path).unwrap().unwrap().failures, 0);
    }

}
