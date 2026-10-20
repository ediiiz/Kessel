//! Shared `quick-xml` helpers.
//!
//! Both profile documents are read as an event stream rather than deserialised: `settingList`s
//! nest to arbitrary depth, and a single unreadable element must be skipped, which an event walk
//! can do locally and a `serde` tree cannot.

use quick_xml::events::BytesStart;
use quick_xml::events::attributes::Attribute;

/// Drops any namespace prefix from a qualified name.
pub(crate) fn strip_prefix(raw: &[u8]) -> &[u8] {
    raw.iter()
        .position(|&byte| byte == b':')
        .map_or(raw, |colon| raw.get(colon + 1..).unwrap_or(raw))
}

/// The element name with any namespace prefix stripped.
pub(crate) fn local_name<'a>(element: &'a BytesStart<'a>) -> &'a [u8] {
    strip_prefix(element.name().into_inner())
}

/// An attribute's value as an owned `String`, or `None` if absent or unreadable.
///
/// Unreadable means "not valid UTF-8" or "a broken entity reference". Both are treated as
/// absent here; the caller decides whether that is fatal for the element (it is, for a `uid`)
/// or merely a missing optional (it is, for everything else).
pub(crate) fn attr(element: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    let found: Option<Attribute<'_>> = element
        .attributes()
        .with_checks(false)
        .filter_map(core::result::Result::ok)
        .find(|attribute| attribute.key.as_ref() == name);
    found?
        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .ok()
        .map(std::borrow::Cow::into_owned)
}

/// Whether the element carries the attribute at all.
pub(crate) fn has_attr(element: &BytesStart<'_>, name: &[u8]) -> bool {
    element
        .attributes()
        .with_checks(false)
        .filter_map(core::result::Result::ok)
        .any(|attribute| attribute.key.as_ref() == name)
}

/// `"true"`/`"false"` → `bool`. Anything else is `None` rather than a guess.
pub(crate) fn attr_bool(element: &BytesStart<'_>, name: &[u8]) -> Option<bool> {
    match attr(element, name)?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// A decimal integer attribute (`min`, `max`, `stepSize`).
pub(crate) fn attr_i64(element: &BytesStart<'_>, name: &[u8]) -> Option<i64> {
    attr(element, name)?.parse().ok()
}

/// A 16-bit hexadecimal attribute (`uid`, `refUID`, `enid`, `refENID`).
pub(crate) fn attr_hex_u16(element: &BytesStart<'_>, name: &[u8]) -> Option<u16> {
    u16::from_str_radix(attr(element, name)?.as_str(), 16).ok()
}
