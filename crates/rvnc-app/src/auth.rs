//! Where passwords come from, and what happens after a login fails.
//!
//! Sources, in order: an explicit password (`--password-file` or
//! `RVNC_PASSWORD`), the keychain, then a terminal prompt. A wrong password
//! from the keychain falls back to prompting; a wrong explicit password is
//! final. See `docs/auth.md`.

use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex, MutexGuard};

use rvnc_client::{CredentialProvider, CredentialRequest, Credentials};
use tracing::{info, warn};
use zeroize::Zeroizing;

use crate::keychain::{self, Kind};

/// How many times to prompt before giving up.
pub const MAX_PROMPTS: u32 = 3;

pub struct Config {
    /// `host:port`, for prompts and keychain item names.
    pub server: String,
    /// Username for Apple Remote Desktop auth.
    pub user: Option<String>,
    /// From `--password-file` or `RVNC_PASSWORD`.
    pub explicit_password: Option<Zeroizing<String>>,
    /// Look up saved passwords.
    pub use_keychain: bool,
    /// Save the password after a successful login.
    pub save: bool,
    /// Prompting is possible (stdin is a terminal).
    pub interactive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Explicit,
    Keychain,
    Prompt,
}

struct Used {
    source: Source,
    service: String,
    account: String,
    password: Zeroizing<String>,
}

#[derive(Default)]
struct State {
    /// Set after a saved password was rejected.
    keychain_failed: bool,
    prompts: u32,
    /// The username last used, as the default for the next prompt.
    last_user: Option<String>,
    /// What the most recent attempt used.
    last: Option<Used>,
}

/// Shared between connection attempts.
#[derive(Clone)]
pub struct Auth {
    cfg: Arc<Config>,
    state: Arc<Mutex<State>>,
}

impl Auth {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg: Arc::new(cfg),
            state: Default::default(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A credential provider for one connection attempt.
    pub fn provider(&self) -> Box<dyn CredentialProvider> {
        self.state().last = None;
        Box::new(self.clone())
    }

    /// Called after the server rejected the credentials. Returns true if
    /// it's worth trying again with different ones.
    pub fn retry_after_failure(&self) -> bool {
        let mut st = self.state();
        match st.last.as_ref().map(|u| u.source) {
            Some(Source::Keychain) => {
                warn!("the saved password was rejected");
                st.keychain_failed = true;
                self.cfg.interactive
            }
            Some(Source::Prompt) => st.prompts < MAX_PROMPTS,
            Some(Source::Explicit) | None => false,
        }
    }

    /// Called once logged in: saves the password if asked to, or if it
    /// replaces a saved one that was rejected.
    pub fn succeeded(&self) {
        let st = self.state();
        let Some(used) = &st.last else { return };
        if used.source == Source::Keychain || !self.cfg.use_keychain {
            return;
        }
        if !(self.cfg.save || st.keychain_failed) {
            return;
        }
        match keychain::set(&used.service, &used.account, &used.password) {
            Ok(()) => {
                info!(service = %used.service, account = %used.account, "saved password in keychain")
            }
            Err(e) => warn!("could not save password in keychain: {e}"),
        }
    }

    fn username(&self, st: &mut State, service: &str) -> Result<String, String> {
        if let Some(u) = &self.cfg.user {
            return Ok(u.clone());
        }
        if self.cfg.use_keychain && !st.keychain_failed {
            match keychain::accounts(service) {
                Ok(accounts) => {
                    if let Some(a) = accounts.into_iter().next() {
                        info!(account = %a, "using saved username from keychain");
                        return Ok(a);
                    }
                }
                Err(e) => warn!("keychain lookup failed: {e}"),
            }
        }
        let default = st
            .last_user
            .clone()
            .or_else(|| std::env::var("USER").ok())
            .unwrap_or_default();
        if !self.cfg.interactive {
            if default.is_empty() {
                return Err("no username given: use --user".into());
            }
            return Ok(default);
        }
        prompt_line(&format!(
            "Username (macOS account) for {} [{default}]: ",
            self.cfg.server
        ))
        .map(|u| if u.is_empty() { default } else { u })
    }

    fn password(
        &self,
        st: &mut State,
        service: &str,
        account: &str,
        prompt: &str,
    ) -> Result<(Zeroizing<String>, Source), String> {
        if let Some(p) = &self.cfg.explicit_password {
            return Ok((p.clone(), Source::Explicit));
        }
        if self.cfg.use_keychain && !st.keychain_failed {
            match keychain::get(service, account) {
                Ok(Some(p)) => {
                    info!("using saved password from keychain");
                    return Ok((p, Source::Keychain));
                }
                Ok(None) => {}
                Err(e) => warn!("keychain lookup failed: {e}"),
            }
        }
        if !self.cfg.interactive {
            return Err(
                "no password was given: run rvnc in a terminal, or use --password-file or \
                 RVNC_PASSWORD"
                    .into(),
            );
        }
        st.prompts += 1;
        let p = rpassword::prompt_password(prompt).map_err(|e| format!("reading password: {e}"))?;
        Ok((Zeroizing::new(p), Source::Prompt))
    }
}

impl CredentialProvider for Auth {
    fn credentials(&mut self, req: &CredentialRequest) -> Result<Credentials, String> {
        let this = self.clone();
        let mut st = this.state();
        let server = &self.cfg.server;
        let (kind, username) = if req.username {
            let service = keychain::service(Kind::MacAccount, server);
            (Kind::MacAccount, Some(self.username(&mut st, &service)?))
        } else {
            (Kind::VncPassword, None)
        };
        let service = keychain::service(kind, server);
        let (account, prompt) = match &username {
            Some(u) => (u.clone(), format!("Password for {u}@{server}: ")),
            None => (
                keychain::VNC_ACCOUNT.to_owned(),
                format!("VNC password for {server}: "),
            ),
        };
        let (password, source) = self.password(&mut st, &service, &account, &prompt)?;

        st.last_user = username.clone();
        st.last = Some(Used {
            source,
            service,
            account,
            password: password.clone(),
        });
        Ok(Credentials {
            username: username.map(Zeroizing::new),
            password,
        })
    }
}

/// Reads one line from stdin after printing `prompt` to stderr.
fn prompt_line(prompt: &str) -> Result<String, String> {
    let mut err = std::io::stderr();
    let _ = write!(err, "{prompt}");
    let _ = err.flush();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("reading username: {e}"))?;
    Ok(line.trim().to_owned())
}

/// Reads a password file (`-` for stdin), dropping one trailing newline.
pub fn read_password_file(path: &str) -> std::io::Result<Zeroizing<String>> {
    let mut s = Zeroizing::new(if path == "-" {
        let mut s = String::new();
        std::io::stdin().lock().read_line(&mut s)?;
        s
    } else {
        std::fs::read_to_string(path)?
    });
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(explicit: Option<&str>, interactive: bool) -> Auth {
        Auth::new(Config {
            server: "h:5900".into(),
            user: Some("alice".into()),
            explicit_password: explicit.map(|p| Zeroizing::new(p.into())),
            use_keychain: false,
            save: false,
            interactive,
        })
    }

    const VNC: CredentialRequest = CredentialRequest {
        security_type: 2,
        username: false,
    };
    const ARD: CredentialRequest = CredentialRequest {
        security_type: 30,
        username: true,
    };

    #[test]
    fn explicit_password_is_used_and_never_retried() {
        let a = auth(Some("pw"), false);
        let mut p = a.provider();
        let c = p.credentials(&VNC).unwrap();
        assert_eq!(c.password.as_str(), "pw");
        assert!(c.username.is_none());
        assert!(!a.retry_after_failure());
    }

    #[test]
    fn ard_gets_the_username() {
        let a = auth(Some("pw"), false);
        let c = a.provider().credentials(&ARD).unwrap();
        assert_eq!(c.username.as_deref().map(String::as_str), Some("alice"));
    }

    #[test]
    fn no_password_without_a_terminal_is_an_error() {
        let a = auth(None, false);
        let e = a.provider().credentials(&VNC).unwrap_err();
        assert!(e.contains("--password-file"), "{e}");
        assert!(!a.retry_after_failure());
    }

    #[test]
    fn password_file_drops_one_newline() {
        let dir = std::env::temp_dir().join(format!("rvnc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (content, want) in [
            ("pw\n", "pw"),
            ("pw\r\n", "pw"),
            ("pw", "pw"),
            ("pw \n\n", "pw \n"),
        ] {
            let f = dir.join("p");
            std::fs::write(&f, content).unwrap();
            assert_eq!(
                read_password_file(f.to_str().unwrap()).unwrap().as_str(),
                want
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
