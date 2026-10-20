//! The response-`code` table.
//!
//! An unrecognised code is not a parse failure: it is [`ResponseCode::Other`], which keeps the
//! numeric value so an unknown code stays diagnosable. Names are bare `PascalCase`. Both 412 and
//! 414 map to `RequestUriTooLong`; either way it is an error.

use core::fmt;

/// A response `code`, classified. `Other` keeps the numeric value of an unknown code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseCode {
    // HTTP-derived range.
    Ok,
    Accepted,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    RequestEntityTooLong,
    /// Both 412 and 414 land here.
    RequestUriTooLong,
    TooManyRequests,
    InternalServerError,
    NotImplemented,
    BadGateway,
    ServiceUnavailable,
    GatewayTimeout,
    InsufficientMemory,
    // Home Connect application range 512-544.
    UnknownUid,
    WriteRequestUnknownUid,
    ReadRequestUnknownUid,
    Busy,
    WriteRequestBusy,
    ReadRequestBusy,
    NoAccess,
    WriteRequestNoAccess,
    ReadRequestNoAccess,
    NoAccessByList,
    WriteRequestNoAccessByList,
    ReadRequestNoAccessByList,
    NotAvailable,
    WriteRequestNotAvailable,
    ReadRequestNotAvailable,
    NotAvailableByList,
    WriteRequestNotAvailableByList,
    ReadRequestNotAvailableByList,
    NoExecution,
    ValueOutOfRange,
    InvalidUidValue,
    Incomplete,
    Inconsistent,
    CmdViolation,
    InvalidFormat,
    RemoteControlNotActive,
    RemoteStartNotActive,
    LockedByLocalControl,
    DeviceStateNotCompliant,
    ProcessStateNotCompliant,
    BackendNotConnected,
    EnergyManagementNotConnected,
    NotInLocalWifi,
    /// A code not in the table; its numeric value is retained.
    Other(i64),
}

impl ResponseCode {
    /// Classifies a numeric response code.
    #[must_use]
    pub const fn from_code(code: i64) -> Self {
        match code {
            200 => Self::Ok,
            202 => Self::Accepted,
            400 => Self::BadRequest,
            403 => Self::Forbidden,
            404 => Self::NotFound,
            405 => Self::MethodNotAllowed,
            413 => Self::RequestEntityTooLong,
            412 | 414 => Self::RequestUriTooLong,
            429 => Self::TooManyRequests,
            500 => Self::InternalServerError,
            501 => Self::NotImplemented,
            502 => Self::BadGateway,
            503 => Self::ServiceUnavailable,
            504 => Self::GatewayTimeout,
            507 => Self::InsufficientMemory,
            512 => Self::UnknownUid,
            513 => Self::WriteRequestUnknownUid,
            514 => Self::ReadRequestUnknownUid,
            515 => Self::Busy,
            516 => Self::WriteRequestBusy,
            517 => Self::ReadRequestBusy,
            518 => Self::NoAccess,
            519 => Self::WriteRequestNoAccess,
            520 => Self::ReadRequestNoAccess,
            521 => Self::NoAccessByList,
            522 => Self::WriteRequestNoAccessByList,
            523 => Self::ReadRequestNoAccessByList,
            524 => Self::NotAvailable,
            525 => Self::WriteRequestNotAvailable,
            526 => Self::ReadRequestNotAvailable,
            527 => Self::NotAvailableByList,
            528 => Self::WriteRequestNotAvailableByList,
            529 => Self::ReadRequestNotAvailableByList,
            530 => Self::NoExecution,
            531 => Self::ValueOutOfRange,
            532 => Self::InvalidUidValue,
            533 => Self::Incomplete,
            534 => Self::Inconsistent,
            535 => Self::CmdViolation,
            536 => Self::InvalidFormat,
            537 => Self::RemoteControlNotActive,
            538 => Self::RemoteStartNotActive,
            539 => Self::LockedByLocalControl,
            540 => Self::DeviceStateNotCompliant,
            541 => Self::ProcessStateNotCompliant,
            542 => Self::BackendNotConnected,
            543 => Self::EnergyManagementNotConnected,
            544 => Self::NotInLocalWifi,
            other => Self::Other(other),
        }
    }

    /// The `PascalCase` name, or `"Unknown"` for an unrecognised code.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Accepted => "Accepted",
            Self::BadRequest => "BadRequest",
            Self::Forbidden => "Forbidden",
            Self::NotFound => "NotFound",
            Self::MethodNotAllowed => "MethodNotAllowed",
            Self::RequestEntityTooLong => "RequestEntityTooLong",
            Self::RequestUriTooLong => "RequestUriTooLong",
            Self::TooManyRequests => "TooManyRequests",
            Self::InternalServerError => "InternalServerError",
            Self::NotImplemented => "NotImplemented",
            Self::BadGateway => "BadGateway",
            Self::ServiceUnavailable => "ServiceUnavailable",
            Self::GatewayTimeout => "GatewayTimeout",
            Self::InsufficientMemory => "InsufficientMemory",
            Self::UnknownUid => "UnknownUID",
            Self::WriteRequestUnknownUid => "WriteRequestUnknownUID",
            Self::ReadRequestUnknownUid => "ReadRequestUnknownUID",
            Self::Busy => "Busy",
            Self::WriteRequestBusy => "WriteRequestBusy",
            Self::ReadRequestBusy => "ReadRequestBusy",
            Self::NoAccess => "NoAccess",
            Self::WriteRequestNoAccess => "WriteRequestNoAccess",
            Self::ReadRequestNoAccess => "ReadRequestNoAccess",
            Self::NoAccessByList => "NoAccessByList",
            Self::WriteRequestNoAccessByList => "WriteRequestNoAccessByList",
            Self::ReadRequestNoAccessByList => "ReadRequestNoAccessByList",
            Self::NotAvailable => "NotAvailable",
            Self::WriteRequestNotAvailable => "WriteRequestNotAvailable",
            Self::ReadRequestNotAvailable => "ReadRequestNotAvailable",
            Self::NotAvailableByList => "NotAvailableByList",
            Self::WriteRequestNotAvailableByList => "WriteRequestNotAvailableByList",
            Self::ReadRequestNotAvailableByList => "ReadRequestNotAvailableByList",
            Self::NoExecution => "NoExecution",
            Self::ValueOutOfRange => "ValueOutOfRange",
            Self::InvalidUidValue => "InvalidUIDValue",
            Self::Incomplete => "Incomplete",
            Self::Inconsistent => "Inconsistent",
            Self::CmdViolation => "CmdViolation",
            Self::InvalidFormat => "InvalidFormat",
            Self::RemoteControlNotActive => "RemoteControlNotActive",
            Self::RemoteStartNotActive => "RemoteStartNotActive",
            Self::LockedByLocalControl => "LockedByLocalControl",
            Self::DeviceStateNotCompliant => "DeviceStateNotCompliant",
            Self::ProcessStateNotCompliant => "ProcessStateNotCompliant",
            Self::BackendNotConnected => "BackendNotConnected",
            Self::EnergyManagementNotConnected => "EnergyManagementNotConnected",
            Self::NotInLocalWifi => "NotInLocalWiFi",
            Self::Other(_) => "Unknown",
        }
    }
}

impl fmt::Display for ResponseCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}
