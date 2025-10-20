//! User and group management primitives for the kernel shell and process subsystem.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use sha2::{Digest, Sha256};
use spin::{Lazy, Mutex};

/// Public information describing a user account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserProfile {
    /// Login/username.
    pub username: String,
    /// Primary numeric user identifier.
    pub uid: u32,
    /// Primary group identifier.
    pub gid: u32,
    /// Supplemental groups associated with the user.
    pub groups: Vec<u32>,
    /// Preferred home directory path.
    pub home: String,
    /// Preferred login shell path.
    pub shell: String,
}

#[derive(Clone, Debug)]
struct UserRecord {
    profile: UserProfile,
    salt: u64,
    hash: [u8; 32],
}

impl UserRecord {
    fn new(profile: UserProfile, password: &str) -> Self {
        let salt = next_salt();
        let hash = hash_password(password, salt);
        Self {
            profile,
            salt,
            hash,
        }
    }

    fn verify_password(&self, password: &str) -> bool {
        self.hash == hash_password(password, self.salt)
    }

    fn update_password(&mut self, password: &str) {
        self.salt = next_salt();
        self.hash = hash_password(password, self.salt);
    }
}

/// Errors surfaced by user management helpers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserError {
    /// The requested username already exists.
    AlreadyExists,
    /// User not found.
    NotFound,
    /// Provided password does not satisfy requirements (e.g. empty).
    InvalidPassword,
    /// Authentication failed due to wrong password.
    AuthenticationFailed,
}

struct UserDatabase {
    by_name: BTreeMap<String, UserRecord>,
    by_uid: BTreeMap<u32, String>,
    next_uid: u32,
    next_gid: u32,
}

impl UserDatabase {
    fn new() -> Self {
        Self {
            by_name: BTreeMap::new(),
            by_uid: BTreeMap::new(),
            next_uid: 1_000,
            next_gid: 1_000,
        }
    }

    fn ensure_root(&mut self) {
        if self.by_name.contains_key("root") {
            return;
        }
        let profile = UserProfile {
            username: String::from("root"),
            uid: 0,
            gid: 0,
            groups: vec![0],
            home: String::from("/root"),
            shell: String::from("/bin/bash"), // Real bash in rootfs
        };
        let record = UserRecord::new(profile.clone(), "root");
        self.by_uid.insert(profile.uid, profile.username.clone());
        self.by_name.insert(profile.username.clone(), record);
    }

    fn allocate_uid(&mut self) -> u32 {
        let uid = self.next_uid;
        self.next_uid = self.next_uid.saturating_add(1);
        uid
    }

    fn allocate_gid(&mut self) -> u32 {
        let gid = self.next_gid;
        self.next_gid = self.next_gid.saturating_add(1);
        gid
    }
}

static USER_DATABASE: Lazy<Mutex<UserDatabase>> = Lazy::new(|| Mutex::new(UserDatabase::new()));
static SALT_COUNTER: AtomicU64 = AtomicU64::new(0xC0DE_CAFE_D15A_B1E5);

fn next_salt() -> u64 {
    SALT_COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::SeqCst)
}

fn hash_password(password: &str, salt: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(salt.to_le_bytes());
    hasher.update(password.as_bytes());
    let digest = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);
    hash
}

fn with_database<T>(mut f: impl FnMut(&mut UserDatabase) -> T) -> T {
    let mut guard = USER_DATABASE.lock();
    guard.ensure_root();
    f(&mut guard)
}

/// Returns a profile snapshot for the given username if present.
pub fn get_user(username: &str) -> Option<UserProfile> {
    with_database(|db| {
        db.by_name
            .get(username)
            .map(|record| record.profile.clone())
    })
}

/// Returns a profile snapshot for the given UID if present.
pub fn get_user_by_uid(uid: u32) -> Option<UserProfile> {
    with_database(|db| {
        let name = db.by_uid.get(&uid)?.clone();
        db.by_name.get(&name).map(|record| record.profile.clone())
    })
}

/// Enumerates all user profiles currently registered.
pub fn list_users() -> Vec<UserProfile> {
    with_database(|db| {
        db.by_name
            .values()
            .map(|record| record.profile.clone())
            .collect()
    })
}

fn validate_password(password: &str) -> Result<(), UserError> {
    if password.is_empty() {
        return Err(UserError::InvalidPassword);
    }
    if password.len() < 4 {
        return Err(UserError::InvalidPassword);
    }
    Ok(())
}

/// Creates a new user with the provided password and optional metadata.
///
/// If `home` is `None`, a default of `/home/<username>` is used. If `shell`
/// is `None`, `/bin/sh` is selected.
pub fn add_user(
    username: &str,
    password: &str,
    home: Option<&str>,
    shell: Option<&str>,
) -> Result<UserProfile, UserError> {
    validate_password(password)?;
    with_database(|db| {
        if db.by_name.contains_key(username) {
            return Err(UserError::AlreadyExists);
        }
        let uid = db.allocate_uid();
        let gid = db.allocate_gid();
        let home_path = home.map(ToString::to_string).unwrap_or_else(|| {
            let mut base = String::from("/home/");
            base.push_str(username);
            base
        });
        let shell_path = shell
            .map(ToString::to_string)
            .unwrap_or_else(|| String::from("/bin/sh"));
        let profile = UserProfile {
            username: username.to_string(),
            uid,
            gid,
            groups: vec![gid],
            home: home_path,
            shell: shell_path,
        };
        let record = UserRecord::new(profile.clone(), password);
        db.by_uid.insert(profile.uid, profile.username.clone());
        db.by_name.insert(profile.username.clone(), record);
        Ok(profile)
    })
}

/// Updates the password for the given user.
pub fn set_password(username: &str, password: &str) -> Result<(), UserError> {
    validate_password(password)?;
    with_database(|db| {
        if let Some(record) = db.by_name.get_mut(username) {
            record.update_password(password);
            Ok(())
        } else {
            Err(UserError::NotFound)
        }
    })
}

/// Verifies the provided credentials, returning the associated profile on success.
pub fn authenticate(username: &str, password: &str) -> Result<UserProfile, UserError> {
    with_database(|db| {
        if let Some(record) = db.by_name.get(username) {
            if record.verify_password(password) {
                Ok(record.profile.clone())
            } else {
                Err(UserError::AuthenticationFailed)
            }
        } else {
            Err(UserError::NotFound)
        }
    })
}

/// Ensures the default root user exists and returns its profile.
pub fn root_profile() -> UserProfile {
    with_database(|db| {
        db.by_name
            .get("root")
            .map(|record| record.profile.clone())
            .expect("root user must be present")
    })
}
