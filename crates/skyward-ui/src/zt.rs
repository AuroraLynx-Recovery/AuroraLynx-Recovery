//! Recovery ZeroTier: the same private network as Aurora 互联, started
//! with `/system/bin/aurora_zt` and a TAP named `aurorazt0`.

use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub const BINARY: &str = "/system/bin/aurora_zt";
pub const STATUS: &str = "/skyward/run/zerotier.status";
pub const STATE_DIR: &str = "/skyward/run/zerotier";
const SHARE_PREFIX: &str = "AURORAZT1:";
const IDENTITY_LEN: usize = 270;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub node: String,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinSpec {
    pub network: String,
    pub identity: Option<String>,
}

/// Sixteen hex digits, as ZeroTier network IDs are written.
pub fn parse_network(text: &str) -> Option<String> {
    let text = text.trim();
    if text.len() != 16 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(text.to_ascii_lowercase())
}

fn identity_ok(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == IDENTITY_LEN
        && b[10] == b':'
        && b[11] == b'0'
        && b[12] == b':'
        && b[141] == b':'
        && b.iter().enumerate().all(|(i, c)| {
            matches!(i, 10 | 12 | 141) || c.is_ascii_hexdigit()
        })
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A 16-digit network ID, or an Interconnect share string that also carries
/// the authorized node identity.
pub fn parse_join(text: &str) -> Result<JoinSpec, String> {
    let text = text.trim();
    if let Some(network) = parse_network(text) {
        return Ok(JoinSpec { network, identity: None });
    }
    let upper = text.to_ascii_uppercase();
    if !upper.starts_with(SHARE_PREFIX) {
        return Err("need a 16-digit network ID or an Aurora ZeroTier share".into());
    }
    let rest = &text[SHARE_PREFIX.len()..];
    let Some((network_raw, rest)) = rest.split_once(':') else {
        return Err("share is truncated".into());
    };
    let network = parse_network(network_raw).ok_or_else(|| "share network ID is invalid".to_string())?;
    // identity contains colons; checksum is the last colon field.
    let Some((identity_raw, crc_raw)) = rest.rsplit_once(':') else {
        return Err("share is truncated".into());
    };
    let identity = identity_raw.to_ascii_lowercase();
    if !identity_ok(&identity) {
        return Err("share identity is invalid".into());
    }
    if crc_raw.len() != 8 || !crc_raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("share checksum is invalid".into());
    }
    let body = format!("{network}:{identity}");
    let got = u32::from_str_radix(&crc_raw.to_ascii_lowercase(), 16).unwrap_or(0);
    if got != crc32(body.as_bytes()) {
        return Err("share checksum mismatch".into());
    }
    Ok(JoinSpec { network, identity: Some(identity) })
}

fn write_identity(secret: &str) -> Result<(), String> {
    fs::create_dir_all(STATE_DIR).map_err(|e| e.to_string())?;
    let path = Path::new(STATE_DIR).join("identity.secret");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| e.to_string())?;
    file.write_all(secret.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

pub fn read_status(path: &Path) -> Status {
    let Ok(text) = fs::read_to_string(path) else {
        return Status::default();
    };
    serde_json::from_str(text.trim()).unwrap_or_default()
}

pub fn present() -> bool {
    Path::new(BINARY).is_file()
}

/// One join: the child, and a generation so the page redraws when status changes.
pub struct Link {
    child: Mutex<Option<Child>>,
    seq: AtomicU64,
    last: Mutex<Status>,
}

impl Link {
    pub fn start(text: &str) -> Result<Arc<Self>, String> {
        let spec = parse_join(text)?;
        if !present() {
            return Err("this recovery was built without ZeroTier".into());
        }
        let _ = fs::create_dir_all(STATE_DIR);
        if let Some(identity) = spec.identity.as_deref() {
            write_identity(identity)?;
        }
        let child = Command::new(BINARY)
            .args(["join", &spec.network, "--dir", STATE_DIR, "--status", STATUS])
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Link {
            child: Mutex::new(Some(child)),
            seq: AtomicU64::new(1),
            last: Mutex::new(Status {
                state: "connecting".into(),
                network: spec.network,
                ..Status::default()
            }),
        }))
    }

    pub fn generation(&self) -> u64 {
        self.seq.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> Status {
        self.last.lock().expect("zerotier status").clone()
    }

    pub fn poll(&self) -> bool {
        if let Some(child) = self.child.lock().expect("zerotier child").as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                let mut last = self.last.lock().expect("zerotier status");
                if last.state != "failed" && last.state != "idle" {
                    last.state = if status.success() { "idle".into() } else { "failed".into() };
                    if !status.success() && last.error.is_empty() {
                        last.error = status.to_string();
                    }
                    self.seq.fetch_add(1, Ordering::Relaxed);
                }
                return true;
            }
        }
        let next = read_status(Path::new(STATUS));
        let mut last = self.last.lock().expect("zerotier status");
        if *last != next {
            *last = next;
            self.seq.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().expect("zerotier child").take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = fs::remove_file(STATUS);
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_ids_are_sixteen_hex_digits() {
        assert_eq!(parse_network(" 8056C2E21C000001 "), Some("8056c2e21c000001".into()));
        assert_eq!(parse_network("8056c2e21c00000"), None);
        assert_eq!(parse_network("gggggggggggggggg"), None);
    }

    #[test]
    fn share_string_round_trips_the_authorized_node() {
        let identity = format!("fedcba9876:0:{}:{}", "ab".repeat(64), "cd".repeat(64));
        let token = format!(
            "AURORAZT1:8056c2e21c000001:{identity}:b3da4c70"
        );
        let spec = parse_join(&token).unwrap();
        assert_eq!(spec.network, "8056c2e21c000001");
        assert_eq!(spec.identity.as_deref(), Some(identity.as_str()));
        assert!(parse_join(&token.to_ascii_lowercase()).is_ok());
        let mut bad = token;
        bad.replace_range(bad.len() - 1.., "1");
        assert!(parse_join(&bad).unwrap_err().contains("checksum"));
    }

    #[test]
    fn status_json_round_trips() {
        let status: Status = serde_json::from_str(
            r#"{"state":"online","node":"0123456789","network":"8056c2e21c000001","addresses":["10.147.17.1/24"],"error":""}"#,
        )
        .unwrap();
        assert_eq!(status.state, "online");
        assert_eq!(status.addresses, ["10.147.17.1/24"]);
    }
}
