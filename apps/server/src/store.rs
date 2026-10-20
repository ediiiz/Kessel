//! The profile store: what lets an onboarded device survive a restart.
//!
//! Each device is two files, `<slug>.zip` and `<slug>.json`. The profile is kept as the original
//! archive because `hc_profile::Credentials` is `#[non_exhaustive]` and cannot be rebuilt outside its
//! crate; [`Profile::from_zip_bytes`] stays the single parse path, and everything derived from it is
//! recomputed at load. The record holds only what the export cannot know: the operator's host and
//! port overrides and the appliance's [`StoredDevice::fingerprint`].
//!
//! The ZIP carries the `haId`, serial, MAC and psk, so both files are written owner-only (`0600`).
//! The record stores a digest instead of the `haId` because it is the file an operator pastes into a
//! bug report.
//!
//! The slug is operator-supplied and becomes a filename, so [`valid_slug`] gates both write paths.
//! This module never dials: starting a connection is `crate::live_transport::spawn_device_tasks`.

use std::path::{Path, PathBuf};

use hc_profile::Profile;
use serde::{Deserialize, Serialize};

/// The environment variable naming the profile directory, following the `KESSEL_*` prefix.
pub const PROFILE_DIR_ENV: &str = "KESSEL_PROFILE_DIR";

/// Where profiles live when [`PROFILE_DIR_ENV`] is unset.
///
/// Under `secrets/` beside the API token's and the stored config's defaults, because the contents are
/// equally sensitive and a developer running from a checkout finds all three in one gitignored place.
///
/// This default is not a persisted path in the add-on image: the image sets [`PROFILE_DIR_ENV`] to
/// `/data/profiles` explicitly, as it does `KESSEL_TOKEN_FILE` and `KESSEL_CONFIG_FILE`.
pub const DEFAULT_PROFILE_DIR: &str = "secrets/profiles";

/// The longest slug the store accepts.
///
/// Generous against what the daemon generates (`cooktop`, `oven-2`) and far inside any filesystem's
/// name limit once `.json` is appended.
pub const MAX_SLUG_LEN: usize = 64;

/// Whether `slug` is safe as both a filename component and an MQTT topic segment.
///
/// ASCII alphanumerics, `-` and `_`; non-empty; at most [`MAX_SLUG_LEN`]. This is an allowlist rather
/// than a scan for `..` and separators. A `.` is refused too because the slug is also a topic segment.
#[must_use]
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= MAX_SLUG_LEN
        && slug.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
}

/// A refused slug, as an IO error — so callers handle one failure kind from the store, not two.
fn bad_slug(slug: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!(
            "{slug:?} is not a usable device name: use letters, digits, `-` and `_`, at most \
             {MAX_SLUG_LEN} characters"
        ),
    )
}

/// What the daemon stores about one device beyond its profile ZIP.
///
/// Anything derivable from the profile (role, entity model, mapping) is recomputed at load instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDevice {
    /// [`Profile::fingerprint`] — which appliance this is, as a digest rather than as its `haId`.
    ///
    /// Stored so a record whose ZIP was swapped underneath it is detectable.
    pub fingerprint: String,
    /// The operator's host override, applied to the profile at load.
    ///
    /// `None` means the host derived from the profile, which often does not resolve on a home LAN.
    pub manual_host: Option<String>,
    /// The operator's port override; `None` is the transport's default (80 AES / 443 TLS-PSK).
    pub port: Option<u16>,
}

/// One device read back off disk, ready to be registered.
#[derive(Debug, Clone)]
pub struct LoadedDevice {
    /// Its slug, taken from the filename — which is why [`valid_slug`] gates what may become one.
    pub slug: String,
    /// Its stored record.
    pub record: StoredDevice,
    /// Its re-parsed profile, **with [`StoredDevice::manual_host`] already applied**.
    ///
    /// Applied here so a loaded profile never reports the derived host to
    /// `crate::onboarding::target_for`.
    pub profile: Profile,
}

/// The directory holding `<slug>.zip` + `<slug>.json` per device.
#[derive(Debug, Clone)]
pub struct ProfileStore {
    dir: PathBuf,
}

impl ProfileStore {
    /// A store over `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// A store over [`PROFILE_DIR_ENV`] if set, else [`DEFAULT_PROFILE_DIR`].
    #[must_use]
    pub fn from_env() -> Self {
        Self::new(
            std::env::var(PROFILE_DIR_ENV)
                .map_or_else(|_unset| PathBuf::from(DEFAULT_PROFILE_DIR), PathBuf::from),
        )
    }

    /// The directory this store reads and writes.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `<dir>/<slug>.<extension>`, or an error if the slug is not usable as a filename.
    fn path(&self, slug: &str, extension: &str) -> std::io::Result<PathBuf> {
        if !valid_slug(slug) {
            return Err(bad_slug(slug));
        }
        Ok(self.dir.join(format!("{slug}.{extension}")))
    }

    /// Writes a device's profile ZIP and its record, replacing any previous pair for that slug.
    ///
    /// The ZIP is written first: an interruption leaves a ZIP with no record, which
    /// [`Self::load_all`] ignores.
    ///
    /// # Errors
    ///
    /// [`std::io::ErrorKind::InvalidInput`] if `slug` is not a usable device name (checked before
    /// anything is written), otherwise any IO error from creating the directory or writing a file.
    pub fn save(&self, slug: &str, zip: &[u8], record: &StoredDevice) -> std::io::Result<()> {
        let zip_path = self.path(slug, "zip")?;
        let record_path = self.path(slug, "json")?;
        std::fs::create_dir_all(&self.dir)?;
        write_private(&zip_path, zip)?;
        write_private(&record_path, &encode(record)?)
    }

    /// Rewrites a device's record, leaving its ZIP untouched — an operator editing the host or port.
    ///
    /// # Errors
    ///
    /// As [`Self::save`], minus the ZIP.
    pub fn save_record(&self, slug: &str, record: &StoredDevice) -> std::io::Result<()> {
        let record_path = self.path(slug, "json")?;
        std::fs::create_dir_all(&self.dir)?;
        write_private(&record_path, &encode(record)?)
    }

    /// Removes a device's ZIP and its record.
    ///
    /// An already-absent file is not an error. Both removals are attempted even if the first fails.
    ///
    /// # Errors
    ///
    /// The first real IO error from either removal, or an invalid slug.
    pub fn delete(&self, slug: &str) -> std::io::Result<()> {
        let zip = remove_if_present(&self.path(slug, "zip")?);
        let record = remove_if_present(&self.path(slug, "json")?);
        zip.and(record)
    }

    /// Every device on disk, in slug order.
    ///
    /// Never fails: one unreadable profile is logged at `warn` and skipped so the other devices still
    /// load. A missing directory is the ordinary first-run state and logs nothing. Sorted by slug
    /// because `read_dir` order is filesystem-dependent.
    #[must_use]
    pub fn load_all(&self) -> Vec<LoadedDevice> {
        let mut slugs = self.stored_slugs();
        slugs.sort();
        slugs
            .into_iter()
            .filter_map(|slug| self.load_one(&slug))
            .collect()
    }

    /// The slugs with a `<slug>.json` record, from the directory listing.
    ///
    /// Keyed on the record, because a ZIP with no record is an interrupted [`Self::save`].
    fn stored_slugs(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            // A fresh install has no profile directory.
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .filter_map(|path| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_owned)
            })
            .filter(|slug| valid_slug(slug))
            .collect()
    }

    /// One device, or `None` with a `warn` explaining which one was skipped and why.
    ///
    /// The reasons are specific because each calls for a different operator action.
    fn load_one(&self, slug: &str) -> Option<LoadedDevice> {
        let record_path = self.path(slug, "json").ok()?;
        let zip_path = self.path(slug, "zip").ok()?;

        let record: StoredDevice = match std::fs::read(&record_path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(record) => record,
                Err(error) => {
                    tracing::warn!(
                        device = %slug,
                        %error,
                        "this device's stored record is not valid JSON; skipping it. Re-onboard the \
                         appliance to rewrite it"
                    );
                    return None;
                }
            },
            Err(error) => {
                tracing::warn!(device = %slug, %error, "this device's stored record is unreadable; skipping it");
                return None;
            }
        };

        let zip = match std::fs::read(&zip_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(
                    device = %slug,
                    %error,
                    "this device has a stored record but no profile beside it, so there is nothing to \
                     rebuild it from; skipping it"
                );
                return None;
            }
        };

        // `ProfileError`'s Display redacts identifier-bearing member filenames, so it is safe to log.
        let mut profile = match Profile::from_zip_bytes(&zip) {
            Ok(profile) => profile,
            Err(error) => {
                tracing::warn!(
                    device = %slug,
                    %error,
                    "this device's stored profile could not be parsed; skipping it. Re-onboard the \
                     appliance to replace it"
                );
                return None;
            }
        };

        // Not fatal (the ZIP is the authority), but the stored host and port may belong to another
        // appliance.
        let actual = profile.fingerprint();
        if actual != record.fingerprint {
            tracing::warn!(
                device = %slug,
                "this device's stored profile is for a different appliance than its record was written \
                 for; loading the profile, but check its host and port"
            );
        }

        if let Some(host) = record.manual_host.clone() {
            profile.set_manual_host(Some(host));
        }

        Some(LoadedDevice {
            slug: slug.to_owned(),
            record,
            profile,
        })
    }
}

/// A record as pretty JSON, so an operator reading `/data` can see what the daemon believes.
fn encode(record: &StoredDevice) -> std::io::Result<Vec<u8>> {
    serde_json::to_vec_pretty(record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))
}

/// Writes `bytes` to `path` **owner-only**.
///
/// Matches [`crate::config::save`] and [`crate::auth`]'s writer.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Removes `path`, treating "already absent" as success.
fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SLUG_LEN, valid_slug};

    /// The slugs the daemon itself generates are accepted — otherwise onboarding could not persist.
    #[test]
    fn the_daemons_own_slugs_are_valid() {
        for slug in ["cooktop", "oven", "oven-2", "fridge", "dishwasher", "other"] {
            assert!(valid_slug(slug), "{slug} is a slug this daemon generates");
        }
    }

    /// The shapes that would escape the directory, or confuse a topic, are refused.
    #[test]
    fn traversal_and_separators_are_refused() {
        for slug in [
            "", "..", ".", "../x", "a/b", "a\\b", "/abs", "a b", "a.b", "a\0b", "a\nb", "über",
        ] {
            assert!(!valid_slug(slug), "{slug:?} must not become a filename");
        }
        assert!(
            !valid_slug(&"a".repeat(MAX_SLUG_LEN + 1)),
            "over the length cap"
        );
        assert!(
            valid_slug(&"a".repeat(MAX_SLUG_LEN)),
            "exactly at the cap is fine"
        );
    }
}
