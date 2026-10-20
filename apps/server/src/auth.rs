//! The API token and browser sessions behind the mutation gate.
//!
//! The gate is off by default: the add-on is normally reachable only through Home Assistant's ingress,
//! which has already authenticated the browser. `require_auth` is a stored setting ([`crate::config`])
//! with an environment override ([`REQUIRE_AUTH_ENV`]), and the token file is not created until it is
//! on.
//!
//! The operator token travels in a request header, never a cookie. A cross-site `<form>` can POST to
//! the daemon but cannot set `X-Kessel-Token`, so the header makes the gate CSRF-safe. Browsers may
//! instead exchange the token for a revocable in-memory session cookie at `POST /api/session`
//! ([`SessionRegistry`]); `SameSite=Strict` keeps a cross-site request from carrying it.
//!
//! Comparison is constant-time (`subtle::ConstantTimeEq`); only the fixed length of a generated token
//! leaks. The token is a `Secret<String>`, so it never reaches a log line, an SSE frame or a `Debug`
//! rendering.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use hc_crypto::Secret;
use rand::Rng as _;
use subtle::ConstantTimeEq as _;

/// The header a mutating request presents its token in. Lowercase: HTTP/2 header names are, and
/// `http`'s `HeaderName` comparison is case-insensitive either way.
pub const TOKEN_HEADER: &str = "x-kessel-token";

/// The cookie a browser session presents instead of [`TOKEN_HEADER`].
///
/// Set by `POST /api/session`; its safety rests on `HttpOnly` and `SameSite=Strict`.
pub const SESSION_COOKIE: &str = "kessel_session";

/// The environment variable naming the token file.
pub const TOKEN_FILE_ENV: &str = "KESSEL_TOKEN_FILE";

/// The environment variable that turns the gate on or off.
///
/// The gate is off by default; see [`require_auth_from_env`] and `crate::config`.
pub const REQUIRE_AUTH_ENV: &str = "KESSEL_REQUIRE_AUTH";

/// Whether [`REQUIRE_AUTH_ENV`] says anything, and what.
///
/// `None` means unset or empty, and the stored config decides. An unrecognised word is `None` too
/// rather than a guess: reading it as "off" would be dangerous and as "on" would lock the operator
/// out over a typo.
#[must_use]
pub fn require_auth_from_env() -> Option<bool> {
    parse_require_auth(&std::env::var(REQUIRE_AUTH_ENV).ok()?)
}

/// [`REQUIRE_AUTH_ENV`]'s grammar.
///
/// Shared with `crate::config::view`, so the UI reports an override only when the daemon applies one.
#[must_use]
pub fn parse_require_auth(raw: &str) -> Option<bool> {
    let value = raw.trim().to_ascii_lowercase();
    match value.as_str() {
        "" => None,
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => {
            tracing::warn!(
                var = REQUIRE_AUTH_ENV,
                "ignoring an unrecognised value; use true or false"
            );
            None
        }
    }
}

/// Where the token lives when [`TOKEN_FILE_ENV`] is unset.
///
/// Relative to the working directory, which in the add-on is the persisted `/data` volume, so the
/// token lands at `/data/secrets/kessel.token`. `secrets/` is gitignored, so `cargo run` from a
/// checkout cannot leave a live credential where `git add` would pick it up. The directory is
/// created on demand by `write_owner_only`.
///
/// A token written to a non-persisted directory is regenerated on every restart, which surfaces as
/// an unexplained 401.
pub const DEFAULT_TOKEN_FILE: &str = "secrets/kessel.token";

/// How many random bytes a generated token carries (rendered as 64 hex characters).
const TOKEN_BYTES: usize = 32;

/// The API token: generated at first run, then read back from disk.
#[derive(Debug, Clone)]
pub struct ApiToken {
    token: Secret<String>,
}

impl ApiToken {
    /// A fresh random token — 32 bytes of `rand::rng()` entropy, hex-encoded.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0_u8; TOKEN_BYTES];
        rand::rng().fill(&mut bytes);
        let mut hex = String::with_capacity(TOKEN_BYTES * 2);
        for byte in bytes {
            // Infallible: writing to a `String` cannot fail.
            let _ = write!(hex, "{byte:02x}");
        }
        Self::from_string(hex)
    }

    /// Wraps an existing token value (the on-disk one, or a test's).
    #[must_use]
    pub fn from_string(token: impl Into<String>) -> Self {
        Self {
            token: Secret::new(token.into()),
        }
    }

    /// Reads the token from `path`, creating it with a fresh one if the file does not exist.
    ///
    /// The file is created with owner-only permissions where the platform has them.
    ///
    /// # Errors
    ///
    /// Any I/O error other than "not found" on read, and any error while creating the file.
    pub fn load_or_create(path: &Path) -> std::io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(existing) => Ok(Self::from_string(existing.trim())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let fresh = Self::generate();
                write_owner_only(path, fresh.expose())?;
                tracing::info!(
                    path = %path.display(),
                    "generated a new API token (mutating requests need it)"
                );
                Ok(fresh)
            }
            Err(error) => Err(error),
        }
    }

    /// Whether `presented` is this token, compared in constant time.
    #[must_use]
    pub fn matches(&self, presented: &str) -> bool {
        let expected = self.token.expose_secret().as_bytes();
        let presented = presented.as_bytes();
        // Length is compared in the clear: `ct_eq` requires equal lengths, and the length of a
        // generated token is a fixed public constant.
        expected.len() == presented.len() && expected.ct_eq(presented).into()
    }

    /// The token value. Used by [`load_or_create`](Self::load_or_create) to write the file and by
    /// the operator-facing startup path; never logged.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.token.expose_secret()
    }
}

/// How long a session cookie is offered for, in seconds (7 days).
///
/// Long enough not to interrupt ordinary use, short enough that a browser left on a shared machine
/// does not stay authorised indefinitely. A restart ends every session regardless.
pub const SESSION_MAX_AGE_SECONDS: u64 = 7 * 24 * 60 * 60;

/// How many sessions may be open at once before the oldest is dropped.
///
/// Every exchange appends, so without a bound a script holding the token could grow the list without
/// limit.
const MAX_SESSIONS: usize = 10;

/// The live browser sessions, each a random credential the operator token was exchanged for.
///
/// Every session cookie is `SameSite=Strict`, so a cross-site request never carries it. `Secure` is
/// not set because the daemon serves plain HTTP and a `Secure` cookie would never be sent back; a TLS
/// reverse proxy in front should add it.
///
/// Sessions are in memory only and end at a restart. They carry no identity, only proof that someone
/// held the operator token.
pub struct SessionRegistry {
    /// Each open session's secret. `ApiToken` is the same primitive and brings its `Secret` redaction.
    sessions: parking_lot::Mutex<Vec<ApiToken>>,
}

impl core::fmt::Debug for SessionRegistry {
    /// A count, never a session. Rendering one would put a live credential in a `Debug`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SessionRegistry")
            .field("open", &self.sessions.lock().len())
            .finish()
    }
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    /// An empty registry: no session is open, so only the token header authorises anything.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sessions: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Opens a session and returns its secret — the **only** time that value leaves this type.
    ///
    /// There is no accessor that hands an existing session's value back out.
    pub fn open(&self) -> String {
        let session = ApiToken::generate();
        let value = session.expose().to_owned();
        // Scoped so the lock is released before the `tracing` call and the return.
        let dropped = {
            let mut sessions = self.sessions.lock();
            sessions.push(session);
            // Oldest first, so the drop is FIFO rather than dropping the session just created.
            let excess = sessions.len().saturating_sub(MAX_SESSIONS);
            if excess > 0 {
                sessions.drain(..excess);
            }
            excess
        };
        if dropped > 0 {
            tracing::warn!(
                max = MAX_SESSIONS,
                dropped,
                "the open-session bound was reached; the oldest session(s) were revoked"
            );
        }
        value
    }

    /// Whether `presented` is an open session, compared in constant time against each.
    ///
    /// Every candidate is compared even after a match, so the work does not depend on *which* session
    /// matched. Iteration order still leaks the count, which is not a secret.
    #[must_use]
    pub fn matches(&self, presented: &str) -> bool {
        if presented.is_empty() {
            return false;
        }
        self.sessions
            .lock()
            .iter()
            .fold(false, |found, session| session.matches(presented) | found)
    }

    /// Closes the session `presented` names, returning whether one was closed.
    pub fn revoke(&self, presented: &str) -> bool {
        let mut sessions = self.sessions.lock();
        let before = sessions.len();
        sessions.retain(|session| !session.matches(presented));
        before != sessions.len()
    }

    /// How many sessions are open.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.lock().len()
    }

    /// Whether no session is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The `Set-Cookie` value that installs `session`, with the attributes that make it safe.
///
/// `Path=/` rather than `/api`: under ingress the app is served from `/api/hassio_ingress/<token>/`,
/// so a cookie scoped to `/api` would also be attached to other add-ons' ingress paths.
#[must_use]
pub fn session_cookie(session: &str) -> String {
    format!(
        "{SESSION_COOKIE}={session}; HttpOnly; SameSite=Strict; Path=/; Max-Age={SESSION_MAX_AGE_SECONDS}"
    )
}

/// The `Set-Cookie` value that clears the session cookie — same attributes, no value, immediate expiry.
///
/// The attributes have to match the ones it was set with or the browser treats it as a different cookie
/// and the old one survives.
#[must_use]
pub fn cleared_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
}

/// The session value a request's `Cookie` header carries, if any.
///
/// Hand-parsed: the grammar needed is only `name=value; name=value`.
#[must_use]
pub fn session_from_cookies(header: &str) -> Option<&str> {
    header.split(';').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name.trim() == SESSION_COOKIE).then(|| value.trim())
    })
}

/// The token file path: [`TOKEN_FILE_ENV`] if set, else [`DEFAULT_TOKEN_FILE`].
#[must_use]
pub fn token_file() -> PathBuf {
    std::env::var(TOKEN_FILE_ENV).map_or_else(|_| PathBuf::from(DEFAULT_TOKEN_FILE), PathBuf::from)
}

/// Writes `contents` to `path`, owner-readable only on Unix.
///
/// A missing parent directory is created with the process umask; the file's own `0600` mode is what
/// protects the credential.
fn write_owner_only(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ApiToken, DEFAULT_TOKEN_FILE, parse_require_auth};

    #[test]
    fn the_require_auth_grammar_reads_both_words_and_ignores_a_typo() {
        for on in ["true", "TRUE", " on ", "yes", "1"] {
            assert_eq!(parse_require_auth(on), Some(true), "{on} means on");
        }
        for off in ["false", "Off", "no", "0"] {
            assert_eq!(parse_require_auth(off), Some(false), "{off} means off");
        }
        assert_eq!(
            parse_require_auth(""),
            None,
            "an empty value is not a decision — the stored config decides"
        );
        assert_eq!(
            parse_require_auth("yes-please"),
            None,
            "and neither is a typo: guessing 'off' would silently open the daemon, guessing 'on' \
             would lock the operator out over a spelling"
        );
    }

    #[test]
    fn the_default_token_file_lives_in_the_ignored_secrets_directory() {
        let default = Path::new(DEFAULT_TOKEN_FILE);
        assert_eq!(
            default.parent(),
            Some(Path::new("secrets")),
            "the default token path must sit inside the gitignored `secrets/` directory, so a \
             daemon started from a checkout cannot deposit a live credential where `git add` \
             would pick it up: {DEFAULT_TOKEN_FILE}"
        );
        assert_eq!(
            default.file_name(),
            Some(std::ffi::OsStr::new("kessel.token")),
            "the file name stays `kessel.token`, which an unanchored ignore rule also \
             matches (belt and braces): {DEFAULT_TOKEN_FILE}"
        );
    }

    #[test]
    fn load_or_create_creates_a_missing_parent_directory_and_still_writes_0600() {
        // A tempdir, never the real `secrets/`: this test must not read, write or notice any
        // credential on the developer's machine.
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("secrets").join("kessel.token");
        assert!(
            !path.parent().unwrap().exists(),
            "precondition: the parent directory does not exist yet"
        );

        let token = ApiToken::load_or_create(&path)
            .expect("a missing parent directory is created, not an error");

        assert!(
            path.exists(),
            "the token file was written at {}",
            path.display()
        );
        let reloaded = ApiToken::load_or_create(&path).expect("the second load reads it back");
        assert!(
            reloaded.matches(token.expose()),
            "the second load returns the same token, not a fresh one"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the file stays owner-only: {mode:o}");
        }
    }

    #[test]
    fn a_generated_token_matches_only_itself() {
        let token = ApiToken::generate();
        let value = token.expose().to_owned();
        assert!(token.matches(&value), "a token matches its own value");
        assert!(!token.matches(""), "an empty presentation never matches");
        assert!(
            !token.matches(&value[..value.len() - 1]),
            "a prefix of the token does not match"
        );
        assert_eq!(value.len(), 64, "32 random bytes render as 64 hex chars");
    }

    #[test]
    fn the_token_never_renders_itself() {
        let token = ApiToken::from_string("super-secret-token-value");
        let rendered = format!("{token:?}");
        assert!(
            !rendered.contains("super-secret-token-value"),
            "Debug must not render the token: {rendered}"
        );
    }
}
