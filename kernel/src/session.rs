//! Runtime session tracking for the interactive shell and spawned processes.

use alloc::string::String;
use alloc::vec::Vec;
use spin::{Lazy, Mutex};

use crate::fs::Credentials;
use crate::users::{self, UserProfile};

/// Snapshot describing the active login session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserSession {
    /// Login/username.
    pub username: String,
    /// Primary user identifier.
    pub uid: u32,
    /// Primary group identifier.
    pub gid: u32,
    /// Supplemental groups.
    pub groups: Vec<u32>,
    /// Home directory path.
    pub home: String,
    /// Preferred shell path.
    pub shell: String,
}

impl From<UserProfile> for UserSession {
    fn from(profile: UserProfile) -> Self {
        Self {
            username: profile.username,
            uid: profile.uid,
            gid: profile.gid,
            groups: profile.groups,
            home: profile.home,
            shell: profile.shell,
        }
    }
}

impl UserSession {
    /// Builds a [`Credentials`] snapshot from the session.
    pub fn credentials(&self) -> Credentials {
        let mut groups = self.groups.clone();
        if !groups.iter().any(|&gid| gid == self.gid) {
            groups.push(self.gid);
        }
        Credentials::new(self.uid, self.gid, groups)
    }
}

static CURRENT_SESSION: Lazy<Mutex<UserSession>> = Lazy::new(|| {
    Mutex::new(UserSession::from(users::root_profile()))
});

/// Returns the current session snapshot.
pub fn current_session() -> UserSession {
    CURRENT_SESSION.lock().clone()
}

/// Updates the active session to the provided user profile.
pub fn set_session(profile: &UserProfile) {
    *CURRENT_SESSION.lock() = UserSession::from(profile.clone());
}

/// Returns filesystem credentials for the active session.
pub fn current_credentials() -> Credentials {
    CURRENT_SESSION.lock().credentials()
}

/// Ensures the default session reflects the root account.
pub fn reset_to_root() {
    let root = users::root_profile();
    set_session(&root);
}