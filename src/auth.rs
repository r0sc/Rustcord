//! Login token handling. The token lives only in memory and in the OS credential store
//! (Windows Credential Manager / macOS Keychain / Secret Service). It is never written to
//! files, never logged (`Debug` is redacted, no `Display`), and only sent to discord.com.

use std::fmt;

const SERVICE: &str = "Rustcord";
const ACCOUNT: &str = "user-token";

/// Password / 2FA code / voice token: redacted in `Debug`, no `Display`, memory wiped on drop.
#[derive(Clone)]
pub struct Secret(zeroize::Zeroizing<String>);

impl Secret {
    pub fn new(s: String) -> Self {
        Self(zeroize::Zeroizing::new(s))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// For comparing an activity that carries one (a game's join secret); not constant-time.
impl PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        self.expose() == other.expose()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

impl Token {
    /// Cleans up a pasted token (whitespace, surrounding quotes). Returns None if it
    /// cannot be a token.
    pub fn parse(input: &str) -> Option<Token> {
        let t = input.trim().trim_matches(|c| c == '"' || c == '\'').trim();
        let ok = t.len() >= 20
            && t.len() <= 512
            && !t.to_ascii_lowercase().starts_with("bot ")
            && t.chars().all(|c| c.is_ascii_graphic());
        ok.then(|| Token(t.to_owned()))
    }

    /// The raw secret. Only for the Authorization header / gateway identify.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| format!("credential store unavailable: {e}"))
}

/// Blocking (fast on Windows). Returns None if no token is stored or the store is unavailable.
pub fn load_token() -> Option<Token> {
    let pw = entry().ok()?.get_password().ok()?;
    Token::parse(&pw)
}

pub fn save_token(token: &Token) -> Result<(), String> {
    entry()?.set_password(token.expose()).map_err(|e| format!("could not store token: {e}"))
}

pub fn clear_token() -> Result<(), String> {
    match entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("could not remove token: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cleans_and_rejects() {
        let t = Token::parse("  \"abcdefghijklmnopqrstuvwxyz.123\"  ").unwrap();
        assert_eq!(t.expose(), "abcdefghijklmnopqrstuvwxyz.123");
        assert!(Token::parse("short").is_none());
        assert!(Token::parse("Bot abcdefghijklmnopqrstuvwxyz").is_none());
        assert!(Token::parse("abc def ghijklmnopqrstuvwxyz").is_none());
        assert_eq!(format!("{t:?}"), "Token(<redacted>)");
    }
}
