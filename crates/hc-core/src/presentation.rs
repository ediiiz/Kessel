//! How a feature is shown, shared by both surfaces.
//!
//! Kessel presents each appliance twice: as Home Assistant discovery payloads
//! ([`crate::discovery`]) and as REST descriptors for the dashboard. Both take their names,
//! descriptions, enum member labels and fallbacks from here, so they cannot disagree.
//!
//! Classification (HA platform, `device_class`, unit, enabled-by-default) belongs to
//! [`crate::mapping`] and is read off [`EntityMapping`]. The naming catalogue itself lives in the
//! server; this module asks it through [`FeatureNaming`].
//!
//! Everything here is display text. `unique_id`s, `object_id`s, topics and write payloads are
//! built from protocol identifiers, so a contributed name can change without HA losing an entity.

use std::collections::BTreeMap;

use crate::discovery::FeatureNaming;
use crate::mapping::{EntityCategory, EntityMapping};

/// A catalogue's say in which section of Home Assistant's device page an entity is filed.
///
/// Not an `Option<EntityCategory>`: the prominent section is the one with no `entity_category` key
/// at all, so "no override" and "override to no category" must be distinct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum CategoryOverride {
    /// No override: [`crate::mapping`]'s heuristic decides.
    #[default]
    Inherit,
    /// The prominent section: no `entity_category` key.
    Primary,
    /// Home Assistant's Configuration section.
    Config,
    /// Home Assistant's Diagnostic section.
    Diagnostic,
}

/// A catalogue's say in how prominently a feature is shown.
///
/// The mapping's heuristic enables only a small set of well-known leaves; features such as a
/// fridge's `EcoMode` or `VacationMode` need a human judgement, which the catalogue supplies.
///
/// Not `#[non_exhaustive]`, because the server's catalogue constructs it with a struct expression.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Curation {
    /// Whether Home Assistant should enable the entity on discovery. [`None`] keeps the heuristic.
    pub enabled_by_default: Option<bool>,
    /// Which device-page section the entity is filed under.
    pub entity_category: CategoryOverride,
}

/// One feature, as a viewer sees it.
///
/// Built by [`present`] and rendered by both surfaces. Carries only what the catalogue answers;
/// everything else is on [`EntityMapping`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PresentedEntity {
    /// What to call this feature. Never empty.
    pub label: String,
    /// Contributed help text, when someone wrote one. Never derived.
    pub description: Option<String>,
    /// Display names for the enum members, keyed by the raw member name.
    ///
    /// Sparse: a member nobody named has no entry and is shown raw (see [`Self::member_label`]).
    pub option_labels: BTreeMap<String, String>,
    /// Whether Home Assistant should enable this entity on discovery: the catalogue's answer, else
    /// [`EntityMapping::enabled_by_default`]. Renderers read this, not the mapping's field.
    pub enabled_by_default: bool,
    /// Which section of Home Assistant's device page this belongs in, with the catalogue's override
    /// applied. [`None`] is the prominent one.
    pub entity_category: Option<EntityCategory>,
}

impl PresentedEntity {
    /// The display name for one member: the contributed one, or the member itself.
    #[must_use]
    pub fn member_label<'m>(&'m self, member: &'m str) -> &'m str {
        self.option_labels
            .get(member)
            .map_or(member, String::as_str)
    }

    /// The appliance's own member name for something Home Assistant sent back.
    ///
    /// The reverse of [`Self::member_label`], and on the path to an appliance:
    ///
    /// 1. a payload that is already a declared member is that member, so adding a label never
    ///    changes the meaning of a payload that worked before;
    /// 2. otherwise, a label carried by exactly one member is that member;
    /// 3. otherwise [`None`]: the caller passes the payload through, and enumeration resolution
    ///    refuses it unless it is a declared ordinal.
    ///
    /// `declared` is the appliance's member list ([`EntityMapping::options`]); the label map omits
    /// unnamed members.
    #[must_use]
    pub fn raw_member<'m>(&'m self, declared: &'m [String], payload: &str) -> Option<&'m str> {
        if let Some(member) = declared.iter().find(|member| *member == payload) {
            return Some(member.as_str());
        }
        let mut matched = self
            .option_labels
            .iter()
            .filter(|(_, label)| label.as_str() == payload)
            .map(|(member, _)| member.as_str());
        let first = matched.next()?;
        // Two members carry the same label: the payload names neither.
        matched.next().is_none().then_some(first)
    }
}

/// The name for a feature whose uid the `FeatureMapping` does not name.
///
/// Unnamed entities are still mapped, so they need a name; the uid is all there is.
#[must_use]
pub fn unnamed_feature_label(uid: u16) -> String {
    format!("Unnamed feature {uid:#06X}")
}

/// The name for a programme whose uid the `FeatureMapping` does not name.
#[must_use]
pub fn unnamed_program_label(uid: u16) -> String {
    format!("Unnamed programme {uid:#06X}")
}

/// The display name for a dotted feature key, from `names` or derived from the key itself.
///
/// The derived fallback is the key's leaf. The server's catalogue answers every key, so this is
/// reached only through [`crate::discovery::DerivedNames`] or with no catalogue.
fn feature_label(key: &str, names: &dyn FeatureNaming) -> String {
    names
        .feature_name(key)
        .unwrap_or_else(|| key.rsplit('.').next().unwrap_or(key).to_owned())
}

/// The contributed name for one of an entity's `options`, from the table that option lives in.
///
/// Usually the options are this feature's enum members, named per feature. For the programme
/// slots ([`crate::mapping::offers_programs`]) they are programme keys, which the catalogue names
/// once in its programme table. Asking the wrong table silently falls back to the raw string.
fn option_name(
    entry: &EntityMapping,
    key: &str,
    option: &str,
    names: &dyn FeatureNaming,
) -> Option<String> {
    if crate::mapping::offers_programs(entry.kind) {
        names.program_name(option)
    } else {
        names.member_name(key, option)
    }
}

/// Every option's contributed name, less any name that would not identify exactly one option.
///
/// A select's options must be distinct, or neither HA nor [`PresentedEntity::raw_member`] can tell
/// them apart (the oven, for example, has programmes in different families sharing a leaf name).
/// A name is dropped, and the option shows its raw string, when:
///
/// 1. more than one option would carry it; or
/// 2. it equals another option's raw name, which always wins in `raw_member`.
fn unambiguous_option_labels(
    entry: &EntityMapping,
    key: &str,
    names: &dyn FeatureNaming,
) -> BTreeMap<String, String> {
    let candidates: Vec<(&String, String)> = entry
        .options
        .iter()
        .filter_map(|option| option_name(entry, key, option, names).map(|label| (option, label)))
        .collect();
    let mut carriers: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, label) in &candidates {
        *carriers.entry(label.as_str()).or_insert(0) += 1;
    }
    candidates
        .iter()
        .filter(|(option, label)| {
            carriers.get(label.as_str()) == Some(&1)
                && !entry
                    .options
                    .iter()
                    .any(|other| other != *option && other == label)
        })
        .map(|(option, label)| ((*option).clone(), label.clone()))
        .collect()
}

/// Presents one mapped feature through a naming catalogue.
///
/// Total: every entry gets a label, including one with no key at all.
#[must_use]
pub fn present(entry: &EntityMapping, names: &dyn FeatureNaming) -> PresentedEntity {
    let Some(key) = entry.key.as_deref() else {
        // Nothing to look up by: the heuristic's answers stand.
        return PresentedEntity {
            label: unnamed_feature_label(entry.uid),
            description: None,
            option_labels: BTreeMap::new(),
            enabled_by_default: entry.enabled_by_default,
            entity_category: entry.entity_category,
        };
    };
    let curation = names.curation(key);
    PresentedEntity {
        label: feature_label(key, names),
        description: names.feature_description(key),
        // Built from the declared options, so the keys are always a subset of `entry.options`.
        option_labels: unambiguous_option_labels(entry, key, names),
        enabled_by_default: curation
            .enabled_by_default
            .unwrap_or(entry.enabled_by_default),
        entity_category: match curation.entity_category {
            CategoryOverride::Inherit => entry.entity_category,
            CategoryOverride::Primary => None,
            CategoryOverride::Config => Some(EntityCategory::Config),
            CategoryOverride::Diagnostic => Some(EntityCategory::Diagnostic),
        },
    }
}

/// The display name for a programme, from `names` or derived from its key.
///
/// Programmes are a separate namespace in the catalogue. `uid` is the fallback for a programme the
/// `FeatureMapping` does not name.
#[must_use]
pub fn present_program(key: Option<&str>, uid: u16, names: &dyn FeatureNaming) -> String {
    let Some(key) = key else {
        return unnamed_program_label(uid);
    };
    names
        .program_name(key)
        .unwrap_or_else(|| key.rsplit('.').next().unwrap_or(key).to_owned())
}
