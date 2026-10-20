//! The `<serial>.json` manifest: identity, credentials, and the two declared XML filenames.
//!
//! 1. The XML filenames come from `deviceDescriptionFileName` / `featureMappingFileName`, never
//!    from the `<serial>_DeviceDescription.xml` convention.
//! 2. `key`, `iv`, `haId`, `serialNumber` and `mac` are wrapped in [`Secret`] here, at the parse
//!    boundary, so none of them exists as a bare `String` inside a `Debug`-deriving type.

use hc_crypto::Secret;
use serde::Deserialize;

use crate::error::{ProfileError, Result};

/// How the appliance expects to be talked to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum ConnectionType {
    /// `"AES"`: `ws://host:80`, with the app-layer crypto chains of `hc-crypto`.
    Aes,
    /// `"TLS"`: `wss://host:443` with TLS-PSK and no app-layer crypto.
    Tls,
    /// Anything else. Treated as non-AES.
    Other(String),
}

impl ConnectionType {
    /// Parses the manifest's `connectionType`.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "AES" => Self::Aes,
            "TLS" => Self::Tls,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Whether this is the AES path.
    ///
    /// Host derivation and the presence of an `iv` both follow from this alone.
    #[must_use]
    pub const fn is_aes(&self) -> bool {
        matches!(self, Self::Aes)
    }
}

/// Who the appliance is.
///
/// `ha_id`, `serial_number` and `mac` are wrapped because they must never reach a log, an MQTT
/// topic or an SSE frame, and this struct derives `Debug`. On the AES path `haId` contains the
/// MAC. [`Self::derived_host`] still returns a plain `String` for the transport.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DeviceIdentity {
    /// `haId`. On AES this is already the `BRAND-VIB-MAC` form; on TLS it is the serial.
    pub ha_id: Secret<String>,
    /// `type`, e.g. `Hob`, `Dishwasher`, `Oven`, `FridgeFreezer`.
    pub device_type: String,
    /// `brand`, e.g. `NEFF`.
    pub brand: String,
    /// `vib`: the model designation, e.g. `T58TL6EN2`.
    pub vib: String,
    /// `serialNumber`.
    pub serial_number: Option<Secret<String>>,
    /// `mac`, in `AA-BB-CC-DD-EE-FF` form.
    pub mac: Option<Secret<String>>,
    /// `created`: the profile export timestamp, verbatim.
    pub created: Option<String>,
    /// `connectionType`.
    pub connection_type: ConnectionType,
}

impl DeviceIdentity {
    /// The default host.
    ///
    /// `connectionType == "AES"` → `haId`; otherwise → `"{brand}-{type}-{haId}"`. This works
    /// because an AES `haId` already carries the composed `BRAND-VIB-MAC` value while a TLS
    /// `haId` is the bare serial.
    #[must_use]
    pub fn derived_host(&self) -> String {
        // The one place the haId is unwrapped: a host is a plain string by the time it reaches
        // a URL.
        let ha_id = self.ha_id.expose_secret();
        if self.connection_type.is_aes() {
            ha_id.clone()
        } else {
            format!("{}-{}-{ha_id}", self.brand, self.device_type)
        }
    }
}

/// The appliance's keys, wrapped from the moment they leave the JSON.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Credentials {
    /// `key`: the base64url pre-shared key, for the AES KDF or the TLS-PSK callback.
    pub psk64: Secret<String>,
    /// `iv`: the base64url CBC initialisation vector. `Some` exactly on the AES path.
    pub iv64: Option<Secret<String>>,
}

/// The two XML member names the manifest declares.
///
/// No `Debug`, deliberately: both names start with the serial or `haId`.
#[derive(Clone)]
pub(crate) struct DeclaredFiles {
    pub(crate) device_description: String,
    pub(crate) feature_mapping: String,
}

/// The manifest exactly as it appears on disk.
///
/// No `Debug`, deliberately: this is the one type that holds `serialNumber`, `mac`, `key` and
/// `iv` as bare `String`s. It is private and consumed immediately by [`Manifest::from_raw`].
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawManifest {
    ha_id: String,
    #[serde(rename = "type")]
    device_type: String,
    brand: String,
    vib: String,
    serial_number: Option<String>,
    mac: Option<String>,
    created: Option<String>,
    connection_type: String,
    feature_mapping_file_name: String,
    device_description_file_name: String,
    key: String,
    iv: Option<String>,
}

/// The manifest, split into the three things the rest of the crate needs.
pub(crate) struct Manifest {
    pub(crate) identity: DeviceIdentity,
    pub(crate) credentials: Credentials,
    pub(crate) files: DeclaredFiles,
}

impl Manifest {
    /// Parses `<serial>.json`.
    ///
    /// # Errors
    ///
    /// [`ProfileError::Manifest`] if the JSON is malformed or a required field is missing, with
    /// the failing field's path in the message.
    /// [`ProfileError::AesWithoutIv`] if `connectionType` is `AES` and `iv` is absent.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        let raw: RawManifest =
            serde_path_to_error::deserialize(&mut deserializer).map_err(|err| {
                ProfileError::Manifest {
                    path: err.path().to_string(),
                    source: err.into_inner(),
                }
            })?;
        Self::from_raw(raw)
    }

    fn from_raw(raw: RawManifest) -> Result<Self> {
        let connection_type = ConnectionType::parse(&raw.connection_type);

        // Wrap here, before anything can hold the bare value.
        let iv64 = raw.iv.map(Secret::new);
        if connection_type.is_aes() && iv64.is_none() {
            return Err(ProfileError::AesWithoutIv);
        }

        Ok(Self {
            identity: DeviceIdentity {
                ha_id: Secret::new(raw.ha_id),
                device_type: raw.device_type,
                brand: raw.brand,
                vib: raw.vib,
                serial_number: raw.serial_number.map(Secret::new),
                mac: raw.mac.map(Secret::new),
                created: raw.created,
                connection_type,
            },
            credentials: Credentials {
                psk64: Secret::new(raw.key),
                iv64,
            },
            files: DeclaredFiles {
                device_description: raw.device_description_file_name,
                feature_mapping: raw.feature_mapping_file_name,
            },
        })
    }
}
