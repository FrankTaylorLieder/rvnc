//! Supplying usernames and passwords to security handlers.
//!
//! The client crate has no UI, so it asks a [`CredentialProvider`] supplied
//! by the application, and only once the server has actually asked for
//! authentication.

use zeroize::Zeroizing;

/// What a security handler needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialRequest {
    /// The security type being performed (see `proto::handshake::security`).
    pub security_type: u8,
    /// True if a username is needed as well as a password.
    pub username: bool,
}

/// A username (when requested) and password. Both are wiped on drop.
pub struct Credentials {
    pub username: Option<Zeroizing<String>>,
    pub password: Zeroizing<String>,
}

impl Credentials {
    pub fn password(password: impl Into<String>) -> Self {
        Self {
            username: None,
            password: Zeroizing::new(password.into()),
        }
    }

    pub fn with_username(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: Some(Zeroizing::new(username.into())),
            password: Zeroizing::new(password.into()),
        }
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("username", &self.username.as_deref())
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Supplies credentials on request. Returning `Err(reason)` aborts the
/// connection with [`crate::ConnectError::NoCredentials`].
pub trait CredentialProvider: Send {
    fn credentials(&mut self, request: &CredentialRequest) -> Result<Credentials, String>;
}

impl<F> CredentialProvider for F
where
    F: FnMut(&CredentialRequest) -> Result<Credentials, String> + Send,
{
    fn credentials(&mut self, request: &CredentialRequest) -> Result<Credentials, String> {
        self(request)
    }
}

/// A provider that has no credentials. The default.
#[derive(Debug, Default)]
pub struct NoCredentials;

impl CredentialProvider for NoCredentials {
    fn credentials(&mut self, _: &CredentialRequest) -> Result<Credentials, String> {
        Err("no password was supplied".into())
    }
}
