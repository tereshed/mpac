//! Serialized host state contains credential hashes, never bearer credentials.
use crate::{equal, secret, secret_hash, valid_secret};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PAIR_SECONDS: u64 = 180;
pub const MAX_DEVICES: usize = 16;
#[derive(Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub key_hash: String,
    pub approved_at: u64,
}
#[derive(Clone, Serialize)]
pub struct Pending {
    pub id: String,
    pub name: String,
    pub code: String,
    pub expires_at: u64,
    #[serde(skip)]
    pub key_hash: String,
}
#[derive(Default)]
pub struct Pairing {
    invite: Option<(String, u64)>,
    pending: HashMap<String, Pending>,
    pub devices: Vec<Device>,
}
impl Pairing {
    pub fn from_devices(devices: Vec<Device>) -> Self {
        Self {
            devices,
            ..Self::default()
        }
    }
    pub fn invite(&mut self, now: u64) -> String {
        let secret = secret();
        self.invite = Some((secret_hash(&secret), now.saturating_add(PAIR_SECONDS)));
        secret
    }
    pub fn request(&mut self, invite: &str, key: &str, name: &str, now: u64) -> Result<Pending> {
        self.pending.retain(|_, p| p.expires_at > now);
        ensure!(
            valid_secret(key) && valid_secret(invite),
            "Invalid pairing credentials"
        );
        ensure!(
            !name.trim().is_empty() && name.len() <= 80 && !name.chars().any(char::is_control),
            "Invalid device name"
        );
        ensure!(
            self.devices.len() < MAX_DEVICES && self.pending.len() < MAX_DEVICES,
            "Too many devices"
        );
        let Some((expected, expires)) = &self.invite else {
            bail!("Invitation expired or already used")
        };
        ensure!(
            *expires > now && equal(expected, &secret_hash(invite)),
            "Invitation expired or already used"
        );
        // Caller holds one lock across consume and insert. A losing request
        // cannot obtain the same invitation or replace the pending fingerprint.
        self.invite = None;
        let hash = secret_hash(key);
        ensure!(
            !self.devices.iter().any(|d| equal(&d.key_hash, &hash)),
            "Device already paired"
        );
        let code = verification_code(key);
        let pending = Pending {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.trim().to_owned(),
            code,
            expires_at: now.saturating_add(PAIR_SECONDS),
            key_hash: hash,
        };
        self.pending.insert(pending.id.clone(), pending.clone());
        Ok(pending)
    }
    pub fn pending(&self, now: u64) -> Vec<Pending> {
        self.pending
            .values()
            .filter(|p| p.expires_at > now)
            .cloned()
            .collect()
    }
    pub fn approve(&mut self, id: &str, code: &str, now: u64) -> Result<Device> {
        let pending = self
            .pending
            .get(id)
            .filter(|p| p.expires_at > now)
            .context_pair()?;
        ensure!(equal(code, &pending.code), "Pairing fingerprint changed");
        ensure!(self.devices.len() < MAX_DEVICES, "Too many devices");
        let device = Device {
            id: pending.id.clone(),
            name: pending.name.clone(),
            key_hash: pending.key_hash.clone(),
            approved_at: now,
        };
        self.pending.remove(id);
        self.devices.push(device.clone());
        Ok(device)
    }
    pub fn status(&self, key: &str, id: &str, now: u64) -> &'static str {
        if !valid_secret(key) {
            return "expired";
        }
        let hash = secret_hash(key);
        if self
            .devices
            .iter()
            .any(|d| d.id == id && equal(&d.key_hash, &hash))
        {
            "approved"
        } else if self
            .pending
            .get(id)
            .is_some_and(|p| p.expires_at > now && equal(&p.key_hash, &hash))
        {
            "pending"
        } else {
            "expired"
        }
    }
    pub fn reject(&mut self, id: &str, code: &str, now: u64) -> Result<()> {
        let pending = self
            .pending
            .get(id)
            .filter(|p| p.expires_at > now)
            .context_pair()?;
        ensure!(equal(code, &pending.code), "Pairing fingerprint changed");
        self.pending.remove(id);
        Ok(())
    }
    pub fn authenticated(&self, key: &str) -> bool {
        valid_secret(key)
            && self
                .devices
                .iter()
                .any(|d| equal(&d.key_hash, &secret_hash(key)))
    }
    pub fn revoke(&mut self, id: &str) -> bool {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != id);
        before != self.devices.len()
    }
}
trait PairContext<T> {
    fn context_pair(self) -> Result<T>;
}
impl<T> PairContext<T> for Option<T> {
    fn context_pair(self) -> Result<T> {
        self.ok_or_else(|| anyhow::anyhow!("Pairing request expired"))
    }
}
pub fn verification_code(key: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(key.as_bytes());
    format!(
        "{:06}",
        u32::from_be_bytes(hash[..4].try_into().unwrap()) % 1_000_000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejection_is_bound_to_a_live_request_and_preserves_existing_devices() {
        let mut p = Pairing::default();
        let connected_key = secret();
        let invite = p.invite(100);
        let connected = p.request(&invite, &connected_key, "iPad", 101).unwrap();
        p.approve(&connected.id, &connected.code, 102).unwrap();
        let invite = p.invite(103);
        let key = secret();
        let pending = p.request(&invite, &key, "iPhone", 104).unwrap();
        assert!(p.reject(&pending.id, "wrong", 105).is_err());
        assert_eq!(p.status(&key, &pending.id, 105), "pending");
        p.reject(&pending.id, &pending.code, 106).unwrap();
        assert_eq!(p.status(&key, &pending.id, 107), "expired");
        assert!(!p.authenticated(&key));
        assert!(p.approve(&pending.id, &pending.code, 107).is_err());
        assert!(p.authenticated(&connected_key));
        assert!(p.reject(&connected.id, &connected.code, 107).is_err());
        let invite = p.invite(108);
        let expired = p.request(&invite, &secret(), "Android", 109).unwrap();
        assert!(
            p.reject(&expired.id, &expired.code, 109 + PAIR_SECONDS)
                .is_err()
        );
    }
    #[test]
    fn invitation_is_single_use_and_never_authorizes_before_local_approval() {
        let mut p = Pairing::default();
        let invite = p.invite(100);
        let key = secret();
        let attempt = p.request(&invite, &key, "iPhone", 101).unwrap();
        assert!(!p.authenticated(&key));
        assert_eq!(p.status(&key, &attempt.id, 102), "pending");
        assert!(p.request(&invite, &secret(), "Android", 102).is_err());
        assert!(p.approve(&attempt.id, "wrong", 102).is_err());
        p.approve(&attempt.id, &attempt.code, 102).unwrap();
        assert!(p.authenticated(&key));
        assert_eq!(p.status(&secret(), &attempt.id, 103), "expired");
        assert!(p.revoke(&attempt.id));
        assert!(!p.authenticated(&key));
        assert_eq!(p.status(&key, &attempt.id, 104), "expired");
    }
    #[test]
    fn expiration_restart_and_rotated_invitation_fail_closed() {
        let mut p = Pairing::default();
        let old = p.invite(10);
        let current = p.invite(11);
        let key = secret();
        assert!(p.request(&old, &key, "Android", 12).is_err());
        assert!(
            p.request(&current, &key, "Android", 11 + PAIR_SECONDS)
                .is_err()
        );
        let invite = p.invite(500);
        let pending = p.request(&invite, &key, "Android", 501).unwrap();
        assert!(
            p.approve(&pending.id, &pending.code, 501 + PAIR_SECONDS)
                .is_err()
        );
        assert_eq!(Pairing::default().status(&key, &pending.id, 502), "expired");
    }
    #[test]
    fn serialized_metadata_cannot_contain_device_or_invite_secrets() {
        let mut p = Pairing::default();
        let invite = p.invite(1);
        let key = secret();
        let request = p.request(&invite, &key, "Android", 2).unwrap();
        let metadata = serde_json::to_string(&request).unwrap();
        assert!(!metadata.contains(&key));
        assert!(!metadata.contains(&invite));
        assert!(!metadata.contains("key_hash"));
        p.approve(&request.id, &request.code, 3).unwrap();
        let stored = serde_json::to_string(&p.devices).unwrap();
        assert!(!stored.contains(&key));
    }
}
