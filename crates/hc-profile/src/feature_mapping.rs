//! `FeatureMapping.xml` → the dotted names.
//!
//! The description says what exists and how it behaves; the mapping says what it is *called*.
//! Two element families matter:
//!
//! - `<feature refUID="020F">BSH.Common.Status.DoorState</feature>`: uid → dotted key.
//! - `<enumDescription refENID="0201" enumKey="…"><enumMember refValue="0">Open</enumMember>…`:
//!   the value → name table.
//!
//! `<errorDescription>` is not read here; error codes are handled from the wire.

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::{ProfileError, Result};
use crate::model::{SkipReason, SkippedEntity};
use crate::xml::{attr, attr_hex_u16, attr_i64, local_name, strip_prefix};

/// Everything the mapping file yields.
#[derive(Default)]
pub(crate) struct FeatureMapping {
    /// uid → dotted feature key.
    pub(crate) features: BTreeMap<u16, String>,
    /// enid → (`enumKey`, value → member name).
    pub(crate) enums: BTreeMap<u16, EnumNames>,
    /// Names that could not be decoded, merged into [`crate::Profile::skipped`].
    pub(crate) skipped: Vec<SkippedEntity>,
}

/// The naming half of an enumeration.
#[derive(Default)]
pub(crate) struct EnumNames {
    pub(crate) key: Option<String>,
    pub(crate) members: BTreeMap<i64, String>,
}

/// Where the next text event belongs.
enum Sink {
    Feature(u16),
    Member(u16, i64),
}

/// Parses a `FeatureMapping` document.
///
/// # Errors
///
/// [`ProfileError::Xml`] if the document is not well-formed.
pub(crate) fn parse(member: &str, xml: &str) -> Result<FeatureMapping> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut mapping = FeatureMapping::default();
    let mut open_enum: Option<u16> = None;
    let mut sink: Option<Sink> = None;

    loop {
        let event = reader.read_event().map_err(|source| ProfileError::Xml {
            member: member.to_owned(),
            position: reader.error_position(),
            source,
        })?;
        match event {
            Event::Start(element) | Event::Empty(element) => {
                sink = on_open(&mut mapping, &mut open_enum, &element);
            }
            Event::End(element) => {
                if strip_prefix(element.name().into_inner()) == b"enumDescription" {
                    open_enum = None;
                }
                sink = None;
            }
            Event::Text(text) => match text.xml10_content() {
                Ok(body) => store(&mut mapping, sink.as_ref(), body.trim()),
                Err(_) => record_undecodable_text(&mut mapping, sink.as_ref()),
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(mapping)
}

fn on_open(
    mapping: &mut FeatureMapping,
    open_enum: &mut Option<u16>,
    element: &BytesStart<'_>,
) -> Option<Sink> {
    match local_name(element) {
        b"feature" => Some(Sink::Feature(attr_hex_u16(element, b"refUID")?)),
        b"enumDescription" => {
            let enid = attr_hex_u16(element, b"refENID")?;
            mapping.enums.entry(enid).or_default().key = attr(element, b"enumKey");
            *open_enum = Some(enid);
            None
        }
        b"enumMember" => Some(Sink::Member((*open_enum)?, attr_i64(element, b"refValue")?)),
        _ => None,
    }
}

/// A name that could not be decoded, recorded rather than dropped.
///
/// Defence in depth: members are UTF-8-validated before parsing, so this is currently
/// unreachable through the public API, but it must never become a silent loss.
fn record_undecodable_text(mapping: &mut FeatureMapping, sink: Option<&Sink>) {
    let (element, raw_uid) = match sink {
        Some(&Sink::Feature(uid)) => ("feature", uid),
        Some(&Sink::Member(enid, _)) => ("enumMember", enid),
        None => return,
    };
    mapping.skipped.push(SkippedEntity {
        element: element.to_owned(),
        raw_uid: Some(format!("{raw_uid:04X}")),
        reason: SkipReason::NotUtf8,
    });
}

fn store(mapping: &mut FeatureMapping, sink: Option<&Sink>, text: &str) {
    if text.is_empty() {
        return;
    }
    match sink {
        Some(&Sink::Feature(uid)) => {
            mapping.features.insert(uid, text.to_owned());
        }
        Some(&Sink::Member(enid, value)) => {
            mapping
                .enums
                .entry(enid)
                .or_default()
                .members
                .insert(value, text.to_owned());
        }
        None => {}
    }
}
