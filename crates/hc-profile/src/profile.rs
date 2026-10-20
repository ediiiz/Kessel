//! The ZIP → [`Profile`] pipeline.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use crate::description::{self, Description};
use crate::error::{ProfileError, Result};
use crate::feature_mapping::{self, FeatureMapping};
use crate::manifest::{Credentials, DeviceIdentity, Manifest};
use crate::model::{EntityModel, EnumType, SkippedEntity};

/// The domain separator [`Profile::fingerprint`] hashes ahead of the `haId`.
///
/// Versioned in the string itself: the digest is a persisted lookup key, so changing the hash or this
/// prefix orphans every record the store holds. A future change takes `v2` and a migration.
const FINGERPRINT_DOMAIN: &[u8] = b"kessel-profile-fingerprint-v1:";

/// A parsed appliance profile: who the device is, how to authenticate to it, and what it can do.
#[derive(Debug, Clone)]
pub struct Profile {
    identity: DeviceIdentity,
    credentials: Credentials,
    model: EntityModel,
    skipped: Vec<SkippedEntity>,
    manual_host: Option<String>,
}

impl Profile {
    /// Parses a profile ZIP from memory.
    ///
    /// # Errors
    ///
    /// See [`ProfileError`]. A malformed entity is not an error; it lands in [`Self::skipped`].
    pub fn from_zip_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_zip_reader(Cursor::new(bytes))
    }

    /// Parses a profile ZIP from disk.
    ///
    /// # Errors
    ///
    /// See [`ProfileError`].
    pub fn from_zip_path(path: &Path) -> Result<Self> {
        Self::from_zip_reader(std::fs::File::open(path)?)
    }

    /// Parses a profile ZIP from any seekable reader.
    ///
    /// # Errors
    ///
    /// See [`ProfileError`].
    pub fn from_zip_reader<R: Read + Seek>(reader: R) -> Result<Self> {
        let mut archive = zip::ZipArchive::new(reader)?;
        let manifest = Manifest::parse(&read_manifest_member(&mut archive)?)?;

        let description = parse_declared(
            &mut archive,
            "deviceDescriptionFileName",
            &manifest.files.device_description,
            description::parse,
        )?;
        let mapping = parse_declared(
            &mut archive,
            "featureMappingFileName",
            &manifest.files.feature_mapping,
            feature_mapping::parse,
        )?;

        let (model, skipped) = assemble(description, mapping);
        Ok(Self {
            identity: manifest.identity,
            credentials: manifest.credentials,
            model,
            skipped,
            manual_host: None,
        })
    }

    /// Who the appliance is.
    #[must_use]
    pub const fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    /// The psk, and the iv on the AES path, both already wrapped.
    #[must_use]
    pub const fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    /// The appliance's capability set.
    #[must_use]
    pub const fn model(&self) -> &EntityModel {
        &self.model
    }

    /// Entities the description declared but the parser could not address.
    #[must_use]
    pub fn skipped(&self) -> &[SkippedEntity] {
        &self.skipped
    }

    /// A stable, non-identifying digest of who this appliance is.
    ///
    /// Hex-encoded SHA-256 over a domain-separated `haId`; the profile store's upsert key, so a
    /// re-uploaded profile updates its existing record. The `haId` identifies the appliance on both
    /// transports, and hashing lets the stored record carry an identity without the identifier.
    ///
    /// **This is not a privacy mechanism.** An AES `haId` is `BRAND-VIB-MAC`, which is small enough
    /// to enumerate. The domain separator only stops the digest matching a bare `sha256(haId)`.
    ///
    /// Invariant under archive compression, member order and naming, and
    /// [`Self::set_manual_host`]. The separator carries a version tag: changing it or the hash
    /// would orphan every stored record.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        use sha2::Digest as _;

        let mut hasher = sha2::Sha256::new();
        hasher.update(FINGERPRINT_DOMAIN);
        // The exposure ends at the hasher: only the digest leaves this function.
        hasher.update(self.identity.ha_id.expose_secret().as_bytes());
        hex::encode(hasher.finalize())
    }

    /// The host derived from the manifest, ignoring any manual override.
    #[must_use]
    pub fn derived_host(&self) -> String {
        self.identity.derived_host()
    }

    /// The manual override, if one is set.
    #[must_use]
    pub fn manual_host(&self) -> Option<&str> {
        self.manual_host.as_deref()
    }

    /// Sets (or, with `None`, clears) the manual host override.
    ///
    /// The derived host is only a default; a static IP set here should be the normal path.
    pub fn set_manual_host(&mut self, host: Option<String>) {
        self.manual_host = host;
    }

    /// Builder form of [`Self::set_manual_host`].
    #[must_use]
    pub fn with_manual_host(mut self, host: impl Into<String>) -> Self {
        self.manual_host = Some(host.into());
        self
    }

    /// The host to connect to: the manual override if set, otherwise the derived default.
    #[must_use]
    pub fn host(&self) -> String {
        self.manual_host
            .clone()
            .unwrap_or_else(|| self.derived_host())
    }
}

// --- archive access -------------------------------------------------------------------------

/// The most any single ZIP member is allowed to inflate to.
///
/// The profile ZIP is untrusted, so both its compression ratio and declared sizes are
/// attacker-controlled. Real descriptions are a few hundred kilobytes; 16 MiB leaves ample
/// headroom while keeping a zip bomb survivable.
pub const MAX_MEMBER_BYTES: u64 = 16 * 1024 * 1024;

/// The manifest is located by its `.json` extension, and only the manifest is.
fn read_manifest_member<R: Read + Seek>(archive: &mut zip::ZipArchive<R>) -> Result<Vec<u8>> {
    let name = archive
        .file_names()
        .find(|name| is_json_member(name))
        .ok_or(ProfileError::NoManifest)?
        .to_owned();
    read_member(archive, &name)
}

/// Case-insensitively: does this member name end in `.json`?
fn is_json_member(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

/// Inflates one member, refusing to allocate more than [`MAX_MEMBER_BYTES`] for it.
///
/// The read is bounded rather than pre-checked against the header's declared size: that field
/// is written by whoever built the archive and is not evidence of anything.
fn read_member<R: Read + Seek>(archive: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut member = archive.by_name(name)?;
    let mut bytes = Vec::new();
    // One byte past the cap, so "exactly at the cap" is allowed and "over it" is detectable.
    Read::take(&mut member, MAX_MEMBER_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MEMBER_BYTES {
        return Err(ProfileError::MemberTooLarge {
            member: name.to_owned(),
            limit: MAX_MEMBER_BYTES,
        });
    }
    Ok(bytes)
}

/// Reads a member the manifest *declared* and hands its text to `parse`.
///
/// The lookup is by declared name only, with no fallback to the
/// `<serial>_DeviceDescription.xml` convention, so a manifest that lies fails loudly.
fn parse_declared<R, T, F>(
    archive: &mut zip::ZipArchive<R>,
    field: &'static str,
    declared: &str,
    parse: F,
) -> Result<T>
where
    R: Read + Seek,
    F: FnOnce(&str, &str) -> Result<T>,
{
    if !archive.file_names().any(|name| name == declared) {
        return Err(ProfileError::DeclaredMemberMissing {
            field,
            declared: declared.to_owned(),
        });
    }
    let bytes = read_member(archive, declared)?;
    let text = String::from_utf8(bytes).map_err(|_| ProfileError::NotUtf8 {
        member: declared.to_owned(),
    })?;
    parse(declared, &text)
}

// --- assembly -------------------------------------------------------------------------------

/// Merges the two documents: the description supplies structure, the mapping supplies names.
fn assemble(
    description: Description,
    mapping: FeatureMapping,
) -> (EntityModel, Vec<SkippedEntity>) {
    let Description {
        meta,
        mut entities,
        enum_values,
        mut programs,
        mut skipped,
    } = description;
    // Both documents can skip entries; merge them so `Profile::skipped` is the one place to look.
    skipped.extend(mapping.skipped);

    for (uid, entity) in &mut entities {
        entity.key = mapping.features.get(uid).cloned();
    }
    for (uid, program) in &mut programs {
        program.key = mapping.features.get(uid).cloned();
    }

    let enums = enum_values
        .into_iter()
        .map(|(enid, declared_values)| {
            let names = mapping.enums.get(&enid);
            let resolved = EnumType {
                enid,
                key: names.and_then(|names| names.key.clone()),
                members: names.map(|names| names.members.clone()).unwrap_or_default(),
                declared_values,
            };
            (enid, resolved)
        })
        .collect::<BTreeMap<_, _>>();

    (EntityModel::new(meta, entities, enums, programs), skipped)
}
