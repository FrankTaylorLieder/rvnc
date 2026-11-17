//! Saved passwords, stored as generic password items in the macOS login
//! keychain. They are visible and removable in Keychain Access under names
//! starting with "rvnc:".

use zeroize::Zeroizing;

/// Which kind of credential an item holds. A server can offer both, with
/// different secrets, so they are stored separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The server's VNC password (security type 2). Has no username.
    VncPassword,
    /// A macOS account, for Apple Remote Desktop auth (type 30).
    MacAccount,
}

/// The account name used for VNC passwords, which have no username.
pub const VNC_ACCOUNT: &str = "vnc";

/// The keychain service name for `server` (`host:port`).
pub fn service(kind: Kind, server: &str) -> String {
    match kind {
        Kind::VncPassword => format!("rvnc: {server} (VNC password)"),
        Kind::MacAccount => format!("rvnc: {server} (macOS account)"),
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use security_framework::base::Error;
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    use security_framework::passwords;
    use zeroize::Zeroizing;

    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25300;

    fn describe(e: Error) -> String {
        e.message()
            .unwrap_or_else(|| format!("keychain error {}", e.code()))
    }

    pub fn get(service: &str, account: &str) -> Result<Option<Zeroizing<String>>, String> {
        match passwords::get_generic_password(service, account) {
            Ok(bytes) => {
                let bytes = Zeroizing::new(bytes);
                String::from_utf8(bytes.to_vec())
                    .map(|s| Some(Zeroizing::new(s)))
                    .map_err(|_| "saved password is not valid UTF-8".to_owned())
            }
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(describe(e)),
        }
    }

    pub fn set(service: &str, account: &str, password: &str) -> Result<(), String> {
        passwords::set_generic_password(service, account, password.as_bytes()).map_err(describe)
    }

    pub fn delete(service: &str, account: &str) -> Result<bool, String> {
        match passwords::delete_generic_password(service, account) {
            Ok(()) => Ok(true),
            Err(e) if e.code() == NOT_FOUND => Ok(false),
            Err(e) => Err(describe(e)),
        }
    }

    pub fn accounts(service: &str) -> Result<Vec<String>, String> {
        let found = ItemSearchOptions::new()
            .class(ItemClass::generic_password())
            .service(service)
            .load_attributes(true)
            .limit(Limit::All)
            .search();
        match found {
            Ok(items) => Ok(items
                .iter()
                .filter_map(|i| i.simplify_dict()?.remove("acct"))
                .collect()),
            Err(e) if e.code() == NOT_FOUND => Ok(Vec::new()),
            Err(e) => Err(describe(e)),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use zeroize::Zeroizing;

    const UNSUPPORTED: &str = "saved passwords need the macOS keychain";

    pub fn get(_: &str, _: &str) -> Result<Option<Zeroizing<String>>, String> {
        Ok(None)
    }

    pub fn set(_: &str, _: &str, _: &str) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }

    pub fn delete(_: &str, _: &str) -> Result<bool, String> {
        Ok(false)
    }

    pub fn accounts(_: &str) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }
}

/// The saved password, if there is one.
pub fn get(service: &str, account: &str) -> Result<Option<Zeroizing<String>>, String> {
    imp::get(service, account)
}

/// Saves a password, replacing any existing one.
pub fn set(service: &str, account: &str, password: &str) -> Result<(), String> {
    imp::set(service, account, password)
}

/// Removes a saved password. Returns false if there wasn't one.
pub fn delete(service: &str, account: &str) -> Result<bool, String> {
    imp::delete(service, account)
}

/// The accounts with saved passwords for `service`.
pub fn accounts(service: &str) -> Result<Vec<String>, String> {
    imp::accounts(service)
}

/// Removes every saved password for `server`. Returns how many were removed.
pub fn forget(server: &str) -> Result<usize, String> {
    let mut n = usize::from(delete(&service(Kind::VncPassword, server), VNC_ACCOUNT)?);
    let mac = service(Kind::MacAccount, server);
    for account in accounts(&mac)? {
        n += usize::from(delete(&mac, &account)?);
    }
    Ok(n)
}
