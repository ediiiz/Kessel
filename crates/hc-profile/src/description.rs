//! `DeviceDescription.xml` → entities, enumerations, programs.
//!
//! One streaming pass. Every element that carries a `uid` becomes an [`Entity`]; `<program>`
//! becomes an entity *and* a [`Program`]; `<option refUID=…>` inside a program becomes a
//! [`ProgramOption`] overlay rather than a new uid.
//!
//! An element whose `uid` cannot be read is recorded in [`Description::skipped`] and the walk
//! continues; one bad element never fails the device.

use std::collections::BTreeMap;

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

use crate::error::{ProfileError, Result};
use crate::model::{
    DeviceMeta, Entity, EntityKind, NumericRange, Program, ProgramOption, SkipReason,
    SkippedEntity, ValueType,
};
use crate::xml::{attr, attr_bool, attr_hex_u16, attr_i64, has_attr, local_name, strip_prefix};

/// Everything the description file yields.
pub(crate) struct Description {
    pub(crate) meta: DeviceMeta,
    pub(crate) entities: BTreeMap<u16, Entity>,
    /// enid → the values this appliance declares. Names come from the `FeatureMapping`.
    pub(crate) enum_values: BTreeMap<u16, Vec<i64>>,
    pub(crate) programs: BTreeMap<u16, Program>,
    pub(crate) skipped: Vec<SkippedEntity>,
}

/// Which `<description>` child the next text event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetaField {
    Type,
    Brand,
    Model,
    Version,
    Revision,
}

impl MetaField {
    const fn from_element(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"type" => Self::Type,
            b"brand" => Self::Brand,
            b"model" => Self::Model,
            b"version" => Self::Version,
            b"revision" => Self::Revision,
            _ => return None,
        })
    }

    /// The element this field is read from, for a skip record.
    const fn element(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Brand => "brand",
            Self::Model => "model",
            Self::Version => "version",
            Self::Revision => "revision",
        }
    }
}

/// The streaming walk's state.
#[derive(Default)]
struct Walker {
    meta: DeviceMeta,
    entities: BTreeMap<u16, Entity>,
    enum_values: BTreeMap<u16, Vec<i64>>,
    programs: BTreeMap<u16, Program>,
    skipped: Vec<SkippedEntity>,
    /// The currently open container elements, innermost last; the last one is an entity's
    /// parent. `None` marks a skipped container with no uid, kept so the matching close pops
    /// the right thing.
    containers: Vec<Option<u16>>,
    open_program: Option<Program>,
    open_enum: Option<(u16, Vec<i64>)>,
    in_description: bool,
    text_target: Option<MetaField>,
}

/// Parses a `DeviceDescription` document.
///
/// # Errors
///
/// [`ProfileError::Xml`] only if the document is not well-formed XML. A well-formed document
/// with unreadable *elements* parses successfully, with those elements in `skipped`.
pub(crate) fn parse(member: &str, xml: &str) -> Result<Description> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut walker = Walker::default();
    loop {
        let event = reader.read_event().map_err(|source| ProfileError::Xml {
            member: member.to_owned(),
            position: reader.error_position(),
            source,
        })?;
        match event {
            Event::Start(element) => walker.on_open(&element, false),
            Event::Empty(element) => walker.on_open(&element, true),
            Event::End(element) => {
                let name = element.name();
                walker.on_close(strip_prefix(name.into_inner()));
            }
            Event::Text(text) => match text.xml10_content() {
                Ok(content) => walker.on_text(content.as_ref()),
                Err(_) => walker.record_undecodable_text(),
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(walker.finish())
}

impl Walker {
    fn finish(self) -> Description {
        Description {
            meta: self.meta,
            entities: self.entities,
            enum_values: self.enum_values,
            programs: self.programs,
            skipped: self.skipped,
        }
    }

    fn on_open(&mut self, element: &BytesStart<'_>, self_closing: bool) {
        let name = local_name(element);
        match name {
            // `Event::Empty` never produces a matching `End`, so a self-closing `<description/>`
            // must not set the flag, or it would swallow the rest of the document.
            b"description" => self.in_description = !self_closing,
            b"enumerationType" => self.open_enumeration(element, self_closing),
            b"enumeration" => self.push_enumeration_value(element),
            _ if self.in_description => self.text_target = MetaField::from_element(name),
            _ => self.open_entity_element(element, name, self_closing),
        }
    }

    fn open_entity_element(&mut self, element: &BytesStart<'_>, name: &[u8], self_closing: bool) {
        let Some(kind) = EntityKind::from_element(name) else {
            return;
        };
        self.on_entity_element(element, kind, self_closing);
    }

    fn on_close(&mut self, name: &[u8]) {
        self.text_target = None;
        match name {
            b"description" => self.in_description = false,
            b"enumerationType" => self.close_enumeration(),
            _ => self.close_entity_element(name),
        }
    }

    fn close_entity_element(&mut self, name: &[u8]) {
        // Opens and closes must balance. A container nested inside a `<description>` block never
        // pushed onto `containers`, so its close must not pop either, or every later entity would
        // be silently reparented.
        if self.in_description {
            return;
        }
        let Some(kind) = EntityKind::from_element(name) else {
            return;
        };
        if kind == EntityKind::Program {
            self.close_program();
        }
        if kind.is_container() {
            self.containers.pop();
        }
    }

    fn on_text(&mut self, text: &str) {
        let Some(field) = self.text_target else {
            return;
        };
        let slot = match field {
            MetaField::Type => &mut self.meta.device_type,
            MetaField::Brand => &mut self.meta.brand,
            MetaField::Model => &mut self.meta.model,
            MetaField::Version => &mut self.meta.version,
            MetaField::Revision => &mut self.meta.revision,
        };
        *slot = Some(text.to_owned());
    }

    /// A text run that could not be decoded, where a `<description>` field was expected.
    ///
    /// Recorded rather than swallowed as an empty string. Defence in depth:
    /// [`crate::Profile::from_zip_reader`] validates each member as UTF-8 first, so this is
    /// currently unreachable through the public API.
    fn record_undecodable_text(&mut self) {
        let Some(field) = self.text_target.take() else {
            return;
        };
        self.skipped.push(SkippedEntity {
            element: field.element().to_owned(),
            raw_uid: None,
            reason: SkipReason::NotUtf8,
        });
    }

    // --- entities ---------------------------------------------------------------------------

    fn on_entity_element(
        &mut self,
        element: &BytesStart<'_>,
        kind: EntityKind,
        self_closing: bool,
    ) {
        // An `<option refUID=…>` inside a `<program>` constrains an existing uid rather than
        // introducing one.
        if kind == EntityKind::Option && self.open_program.is_some() && !has_attr(element, b"uid") {
            self.push_program_option(element);
            return;
        }

        let uid = match uid_of(element) {
            Ok(uid) => {
                let entity = build_entity(element, kind, uid, self.parent());
                self.entities.insert(uid, entity);
                if kind == EntityKind::Program {
                    self.open_program(element, uid, self_closing);
                }
                Some(uid)
            }
            Err(reason) => {
                self.record_skip(element, reason);
                None
            }
        };

        // Opens and closes must balance: a container whose `uid` could not be read still takes
        // an anonymous stack slot, because its close will pop one.
        if !self_closing && kind.is_container() {
            self.containers.push(uid);
        }
    }

    /// The uid of the innermost open container, if that container is addressable.
    ///
    /// `None` both at the top level and directly inside a skipped container; the grandparent's
    /// uid is never substituted.
    fn parent(&self) -> Option<u16> {
        self.containers.last().copied().flatten()
    }

    fn record_skip(&mut self, element: &BytesStart<'_>, reason: SkipReason) {
        let raw_uid = attr(element, b"uid").or_else(|| attr(element, b"refUID"));
        let name = String::from_utf8_lossy(local_name(element)).into_owned();
        tracing::warn!(
            element = %name,
            uid = ?raw_uid,
            %reason,
            "skipping malformed entity; the rest of the description still loads"
        );
        self.skipped.push(SkippedEntity {
            element: name,
            raw_uid,
            reason,
        });
    }

    // --- programs ---------------------------------------------------------------------------

    fn open_program(&mut self, element: &BytesStart<'_>, uid: u16, self_closing: bool) {
        self.open_program = Some(Program {
            uid,
            key: None,
            available: attr_bool(element, b"available"),
            execution: attr(element, b"execution"),
            group: self.parent(),
            options: Vec::new(),
        });
        if self_closing {
            self.close_program();
        }
    }

    fn close_program(&mut self) {
        if let Some(program) = self.open_program.take() {
            self.programs.insert(program.uid, program);
        }
    }

    fn push_program_option(&mut self, element: &BytesStart<'_>) {
        let Some(ref_uid) = attr_hex_u16(element, b"refUID") else {
            self.record_skip(element, SkipReason::UnparsableUid);
            return;
        };
        let option = ProgramOption {
            ref_uid,
            access: attr(element, b"access").map(|raw| crate::model::Access::parse(&raw)),
            available: attr_bool(element, b"available"),
            live_update: attr_bool(element, b"liveUpdate"),
            range: range_of(element),
            default: attr(element, b"default"),
            init_value: attr(element, b"initValue"),
            enid: attr_hex_u16(element, b"enumerationType"),
        };
        if let Some(program) = self.open_program.as_mut() {
            program.options.push(option);
        }
    }

    // --- enumerations -----------------------------------------------------------------------

    fn open_enumeration(&mut self, element: &BytesStart<'_>, self_closing: bool) {
        let Some(enid) = attr_hex_u16(element, b"enid") else {
            self.record_skip(element, SkipReason::UnparsableUid);
            return;
        };
        self.open_enum = Some((enid, Vec::new()));
        if self_closing {
            self.close_enumeration();
        }
    }

    fn push_enumeration_value(&mut self, element: &BytesStart<'_>) {
        if let (Some(value), Some(open)) = (attr_i64(element, b"value"), self.open_enum.as_mut()) {
            open.1.push(value);
        }
    }

    fn close_enumeration(&mut self) {
        if let Some((enid, values)) = self.open_enum.take() {
            self.enum_values.insert(enid, values);
        }
    }
}

/// The `uid` (or, for a program option that also declares one, `refUID`) as a `u16`.
fn uid_of(element: &BytesStart<'_>) -> core::result::Result<u16, SkipReason> {
    let raw = attr(element, b"uid")
        .or_else(|| attr(element, b"refUID"))
        .ok_or(SkipReason::MissingUid)?;
    u16::from_str_radix(&raw, 16).map_err(|_| SkipReason::UnparsableUid)
}

fn range_of(element: &BytesStart<'_>) -> NumericRange {
    NumericRange {
        min: attr_i64(element, b"min"),
        max: attr_i64(element, b"max"),
        step: attr_i64(element, b"stepSize"),
    }
}

/// An enumeration reference wins over a range: an enum-typed setting can also carry `min`/`max`,
/// and the enumeration is the more specific statement of what the value may be.
fn value_type_of(element: &BytesStart<'_>) -> ValueType {
    if let Some(enid) = attr_hex_u16(element, b"enumerationType") {
        return ValueType::Enumeration { enid };
    }
    let range = range_of(element);
    if range.is_empty() {
        ValueType::Unconstrained
    } else {
        ValueType::Numeric(range)
    }
}

fn build_entity(
    element: &BytesStart<'_>,
    kind: EntityKind,
    uid: u16,
    parent: Option<u16>,
) -> Entity {
    Entity {
        uid,
        key: None,
        kind,
        access: attr(element, b"access").map(|raw| crate::model::Access::parse(&raw)),
        available: attr_bool(element, b"available"),
        value_type: value_type_of(element),
        ref_cid: attr(element, b"refCID"),
        ref_did: attr(element, b"refDID"),
        init_value: attr(element, b"initValue"),
        notify_on_change: attr_bool(element, b"notifyOnChange"),
        live_update: attr_bool(element, b"liveUpdate"),
        parent,
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// A container element nested inside a `<description>` block must not unbalance the parent
    /// stack, which would silently reparent every later entity.
    #[test]
    fn a_container_inside_a_description_block_does_not_unbalance_the_parent_stack() {
        let xml = concat!(
            r#"<statusList uid="0100"><description><settingList uid="0200"></settingList></description>"#,
            "\n",
            r#"<status uid="0001"/></statusList>"#,
        );

        let description = parse("probe_DeviceDescription.xml", xml)
            .expect("the probe is well-formed XML and must parse");

        // The enclosing list itself is a root-level container.
        assert_eq!(
            description
                .entities
                .get(&0x0100)
                .map(|entity| entity.parent),
            Some(None),
            "the enclosing statusList is an entity with no parent of its own"
        );
        // Everything inside `<description>` is metadata, not entities.
        assert!(
            !description.entities.contains_key(&0x0200),
            "a uid inside a <description> block is not an entity"
        );

        assert_eq!(
            description
                .entities
                .get(&0x0001)
                .map(|entity| entity.parent),
            Some(Some(0x0100)),
            "the entity after </description> is still parented by the enclosing statusList"
        );
        // Reparenting would be silent, so the parent assertion above is the real check.
        assert!(
            description.skipped.is_empty(),
            "nothing here is a malformed entity, so nothing is skipped: {:?}",
            description.skipped
        );
    }
}
