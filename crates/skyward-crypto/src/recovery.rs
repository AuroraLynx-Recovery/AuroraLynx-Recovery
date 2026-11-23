//! Versioned verifier for the independent recovery credential.

use anyhow::{Result, anyhow, ensure};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::fs::File;
use std::io::Read;

const VERSION: u8 = 1;
const SALT_BYTES: usize = 16;
const TAG_BYTES: usize = 32;
pub const RECORD_BYTES: usize = 2 + SALT_BYTES + TAG_BYTES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Pin,
    Password,
}

pub struct Verifier {
    kind: Kind,
    salt: [u8; SALT_BYTES],
    tag: [u8; TAG_BYTES],
}

impl Verifier {
    pub fn new(kind: Kind, credential: &[u8]) -> Result<Self> {
        ensure!(valid(kind, credential), "invalid recovery credential length or format");
        let mut salt = [0; SALT_BYTES];
        File::open("/dev/urandom")?.read_exact(&mut salt)?;
        let tag = authentication(&salt, credential)?.finalize().into_bytes().into();
        Ok(Self { kind, salt, tag })
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn verify(&self, credential: &[u8]) -> Result<bool> {
        if !valid(self.kind, credential) {
            return Ok(false);
        }
        Ok(authentication(&self.salt, credential)?.verify_slice(&self.tag).is_ok())
    }

    pub fn encode(&self) -> [u8; RECORD_BYTES] {
        let mut bytes = [0; RECORD_BYTES];
        bytes[0] = VERSION;
        bytes[1] = match self.kind {
            Kind::Pin => 1,
            Kind::Password => 2,
        };
        bytes[2..2 + SALT_BYTES].copy_from_slice(&self.salt);
        bytes[2 + SALT_BYTES..].copy_from_slice(&self.tag);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == RECORD_BYTES && bytes[0] == VERSION, "invalid recovery credential record");
        let kind = match bytes[1] {
            1 => Kind::Pin,
            2 => Kind::Password,
            _ => return Err(anyhow!("unknown recovery credential type")),
        };
        let salt = bytes[2..2 + SALT_BYTES].try_into()?;
        let tag = bytes[2 + SALT_BYTES..].try_into()?;
        Ok(Self { kind, salt, tag })
    }
}

fn valid(kind: Kind, secret: &[u8]) -> bool {
    match kind {
        Kind::Pin => (6..=16).contains(&secret.len()) && secret.iter().all(u8::is_ascii_digit),
        Kind::Password => (8..=128).contains(&secret.len()) && secret.iter().all(|b| b.is_ascii_graphic() || *b == b' '),
    }
}

fn authentication(salt: &[u8; SALT_BYTES], credential: &[u8]) -> Result<Hmac<Sha256>> {
    let params = scrypt::Params::new(16, 8, 1, TAG_BYTES).map_err(|e| anyhow!("recovery credential cost: {e}"))?;
    let mut key = [0; TAG_BYTES];
    scrypt::scrypt(credential, salt, &params, &mut key).map_err(|e| anyhow!("derive recovery credential: {e}"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key)?;
    mac.update(b"AuroraAOSP Skyward recovery credential version 1");
    Ok(mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_verifier_checks_exact_credential() {
        let bytes = Verifier::new(Kind::Pin, b"123456").unwrap().encode();
        let loaded = Verifier::decode(&bytes).unwrap();
        assert_eq!(loaded.kind(), Kind::Pin);
        assert!(loaded.verify(b"123456").unwrap());
        assert!(!loaded.verify(b"123457").unwrap());
        assert!(!loaded.verify(b"12345").unwrap());
        assert_ne!(bytes, Verifier::new(Kind::Pin, b"123456").unwrap().encode());
    }

    #[test]
    fn invalid_records_and_credentials_are_rejected() {
        assert!(Verifier::decode(&[]).is_err());
        assert!(Verifier::decode(&[VERSION; RECORD_BYTES - 1]).is_err());
        assert!(Verifier::decode(&[0; RECORD_BYTES]).is_err());
        let mut bytes = [0; RECORD_BYTES];
        bytes[0] = VERSION;
        assert!(Verifier::decode(&bytes).is_err());
        assert!(Verifier::new(Kind::Pin, b"12a456").is_err());
        assert!(Verifier::new(Kind::Password, b"short").is_err());
    }
}
