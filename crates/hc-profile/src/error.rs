//! Typed errors for profile parsing.
//!
//! There is no "malformed entity" error: a single bad entity is recorded on
//! [`crate::Profile::skipped`] instead of failing the whole profile.

use core::fmt;

use thiserror::Error;

/// Redacts the identifier-bearing prefix of a profile member filename, keeping the structural
/// suffix that makes an error actionable.
///
/// Members are named `<serial>_DeviceDescription.xml` or `<haId>_DeviceDescription.xml`, and the
/// prefix before the first `_` is a device identifier. The prefix becomes `<redacted>`; a name with
/// no `_` keeps just its extension; a name with neither is redacted whole. Used by both
/// [`ProfileError`]'s `Display` and its `Debug`.
fn redact_member(name: &str) -> String {
    if let Some((_identifier, suffix)) = name.split_once('_') {
        format!("<redacted>_{suffix}")
    } else if let Some((_stem, extension)) = name.rsplit_once('.') {
        format!("<redacted>.{extension}")
    } else {
        "<redacted>".to_owned()
    }
}

/// Everything that can stop a profile ZIP from becoming a [`crate::Profile`].
///
/// Every variant is a whole-archive failure: the ZIP is unreadable, a declared member is
/// missing, or the manifest is not a profile manifest.
///
/// `Debug` is hand-written so member filenames are redacted; a derived `Debug`
/// would leak the identifier in logs and panic messages.
#[derive(Error)]
#[non_exhaustive]
pub enum ProfileError {
    /// The bytes are not a readable ZIP archive, or a member could not be inflated.
    #[error("profile archive is not readable: {0}")]
    Archive(#[from] zip::result::ZipError),

    /// Reading a ZIP member's bytes failed.
    #[error("could not read profile archive member: {0}")]
    Io(#[from] std::io::Error),

    /// No `*.json` member, so nothing declares the two XML filenames.
    #[error("profile archive contains no `*.json` manifest member")]
    NoManifest,

    /// The manifest is not valid JSON, or a required field is missing or the wrong type.
    ///
    /// Carries the `serde_path_to_error` path so the message names the offending field.
    #[error("profile manifest is invalid at `{path}`: {source}")]
    Manifest {
        /// Dotted path to the field that failed, e.g. `connectionType`.
        path: String,
        /// The underlying serde error.
        source: serde_json::Error,
    },

    /// The manifest declares a filename that the archive does not contain.
    ///
    /// Members are looked up by what the JSON declares, never by naming convention.
    #[error(
        "profile manifest declares member `{}` in field `{field}`, which the archive does not contain",
        redact_member(.declared)
    )]
    DeclaredMemberMissing {
        /// The manifest field that made the declaration.
        field: &'static str,
        /// The filename it declared.
        declared: String,
    },

    /// A ZIP member was not valid UTF-8.
    #[error("profile archive member `{}` is not valid UTF-8", redact_member(.member))]
    NotUtf8 {
        /// The offending member name.
        member: String,
    },

    /// A ZIP member inflates to more than [`crate::MAX_MEMBER_BYTES`].
    ///
    /// The profile ZIP is untrusted input and its declared sizes are attacker-controlled, so
    /// members are read with a hard bound to defeat zip bombs.
    #[error(
        "profile archive member `{}` inflates to more than {limit} bytes",
        redact_member(.member)
    )]
    MemberTooLarge {
        /// The offending member name.
        member: String,
        /// The ceiling that was exceeded, in bytes.
        limit: u64,
    },

    /// One of the two XML documents is not well-formed.
    ///
    /// Carries the byte offset of the fault; descriptions can run to hundreds of kilobytes.
    #[error("profile XML `{}` is malformed at byte {position}: {source}", redact_member(.member))]
    Xml {
        /// The offending member name.
        member: String,
        /// Byte offset into the member at which `quick-xml` reports the fault.
        position: u64,
        /// The underlying `quick-xml` error.
        source: quick_xml::Error,
    },

    /// `connectionType == "AES"` but the manifest carries no `iv`.
    ///
    /// Without an `iv` the CBC chain cannot be initialised, so this is a hard error.
    #[error("AES profile carries no `iv`; the CBC chain cannot be initialised without one")]
    AesWithoutIv,
}

/// Hand-written so member filenames are redacted. Every other field is shown.
impl fmt::Debug for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Archive(source) => f.debug_tuple("Archive").field(source).finish(),
            Self::Io(source) => f.debug_tuple("Io").field(source).finish(),
            Self::NoManifest => f.write_str("NoManifest"),
            Self::Manifest { path, source } => f
                .debug_struct("Manifest")
                .field("path", path)
                .field("source", source)
                .finish(),
            Self::DeclaredMemberMissing { field, declared } => f
                .debug_struct("DeclaredMemberMissing")
                .field("field", field)
                .field("declared", &redact_member(declared))
                .finish(),
            Self::NotUtf8 { member } => f
                .debug_struct("NotUtf8")
                .field("member", &redact_member(member))
                .finish(),
            Self::MemberTooLarge { member, limit } => f
                .debug_struct("MemberTooLarge")
                .field("member", &redact_member(member))
                .field("limit", limit)
                .finish(),
            Self::Xml {
                member,
                position,
                source,
            } => f
                .debug_struct("Xml")
                .field("member", &redact_member(member))
                .field("position", position)
                .field("source", source)
                .finish(),
            Self::AesWithoutIv => f.write_str("AesWithoutIv"),
        }
    }
}

/// `Result` specialised to [`ProfileError`].
pub type Result<T> = core::result::Result<T, ProfileError>;
