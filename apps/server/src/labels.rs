//! Display labels for feature keys, enum members and programs.
//!
//! The appliance archive carries no human-readable names: `FeatureMapping.xml` has only keys and
//! enum members, so anything friendlier than `Dishcare.Dishwasher.Setting.TimeLight` comes from the
//! contributed tables under `data/labels/`.
//!
//! Lookup happens server-side because it is keyed by brand and model number, which never leave the
//! daemon; the wire carries only the resolved string.
//!
//! Labels are presentation only. The raw key and member are still sent beside them, and a write
//! always carries the member verbatim. An unmapped key falls back to [`humanize`], a pure spelling
//! transform of the identifier.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use rust_embed::Embed;
use serde::Deserialize;

/// The contributed tables under `data/labels/`, embedded at compile time. A new file is picked up
/// without any index or code change.
#[derive(Embed)]
#[folder = "data/labels"]
struct LabelFiles;

/// Which layer a name came from, so the UI can tell a contributed name from a derived one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSource {
    /// From `models/<BRAND>/<VIB>.toml` — contributed for this exact appliance.
    Model,
    /// From `prefixes/<Vendor.Section>.toml` — contributed for every appliance sharing the prefix.
    Prefix,
    /// From [`humanize`] — nobody named this; it is the identifier, re-spelled.
    Derived,
}

/// A resolved name, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    /// What to show.
    pub text: String,
    /// Which layer produced [`Self::text`].
    pub source: LabelSource,
    /// Contributed help text, when a layer supplied one. Never derived.
    pub description: Option<String>,
}

// The doc comments here and on the fields are what an editor shows from `schema.json`, so they are
// written for a contributor.
/// How one feature or programme is named and shown. Every field is optional, and each one left out
/// comes from the next layer (the prefix file under a model file).
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Entry {
    /// What the web UI and Home Assistant call it. Leave out to keep the next layer's name, or the
    /// identifier re-spelled (`TimeLight` → `Time light`).
    #[serde(default)]
    label: Option<String>,
    /// Help text shown beside the name. Describe what you have observed; omit rather than guess.
    #[serde(default)]
    description: Option<String>,
    /// Names for the enumeration's members, keyed by the raw member exactly as the appliance
    /// declares it (`ColdWater = "Cold water"`). A write still sends the raw member.
    #[serde(default)]
    values: BTreeMap<String, String>,
    /// Whether Home Assistant enables the entity when it is discovered. Leave out to let the
    /// built-in rule decide, which enables only the handful of features an appliance is for.
    #[serde(default)]
    enabled_by_default: Option<bool>,
    /// Which section of Home Assistant's device page it lands in. `primary` is the prominent one
    /// at the top.
    #[serde(default)]
    entity_category: Option<CategoryWord>,
    /// Home Assistant's icon for the entity: any Material Design Icon, `mdi:<name>`
    /// (<https://pictogrammers.com/library/mdi/>). Leave out of a binary sensor with a
    /// `device_class`, which draws a different icon per state.
    #[serde(default)]
    #[schemars(regex(pattern = r"^mdi:[a-z0-9-]+$"))]
    icon: Option<String>,
    /// Home Assistant's `device_class`, from the vocabulary of the entity's platform; `problem`
    /// makes a binary sensor a fault. A class the platform lacks fails the tests.
    #[serde(default)]
    #[schemars(schema_with = "device_class_schema")]
    device_class: Option<String>,
}

/// The schema for [`Entry::device_class`]: one of Home Assistant's classes.
fn device_class_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "enum": hc_core::discovery::known_device_classes(),
    })
}

/// The word a contributed `entity_category` may use.
///
/// A closed enum so a mistyped value fails the parse instead of silently meaning "no override".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
enum CategoryWord {
    /// The prominent section at the top of the device page.
    Primary,
    /// Home Assistant's Configuration section.
    Config,
    /// Home Assistant's Diagnostic section.
    Diagnostic,
}

impl From<CategoryWord> for hc_core::presentation::CategoryOverride {
    fn from(word: CategoryWord) -> Self {
        match word {
            CategoryWord::Primary => Self::Primary,
            CategoryWord::Config => Self::Config,
            CategoryWord::Diagnostic => Self::Diagnostic,
        }
    }
}

/// A contributed catalogue file: names and presentation for an appliance's features and
/// programmes.
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(title = "Kessel label catalogue")]
struct LabelFile {
    /// Keyed by the full dotted feature key, quoted: `[features."Dishcare.Dishwasher.Setting.TimeLight"]`.
    #[serde(default)]
    features: BTreeMap<String, Entry>,
    /// Keyed by the full dotted programme key: `[programs."Dishcare.Dishwasher.Program.Eco50"]`.
    #[serde(default)]
    programs: BTreeMap<String, Entry>,
}

/// The JSON Schema of a catalogue file, as committed at `data/labels/schema.json` for editor
/// completion. Draft 7, which taplo and most editors read.
#[must_use]
pub fn catalogue_schema() -> String {
    let schema = schemars::generate::SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<LabelFile>();
    let mut text = serde_json::to_string_pretty(&schema).unwrap_or_default();
    text.push('\n');
    text
}

/// Every contributed file, keyed by the lookup that selects it.
#[derive(Debug, Default)]
pub struct LabelTable {
    /// Keyed by `"<BRAND>/<VIB>"`, upper-cased.
    models: BTreeMap<String, LabelFile>,
    /// Keyed by the dotted prefix, e.g. `BSH.Common`.
    prefixes: BTreeMap<String, LabelFile>,
}

/// Every dotted key any contributed file names, features and programs together.
///
/// Lets a test check that each contributed key names something an appliance declares; a wrong key
/// would otherwise be silent.
#[must_use]
pub fn contributed_keys() -> Vec<String> {
    let table = embedded();
    table
        .models
        .values()
        .chain(table.prefixes.values())
        .flat_map(|file| file.features.keys().chain(file.programs.keys()))
        .cloned()
        .collect()
}

/// The process-wide table, parsed once. A malformed file yields an empty table here; the
/// `contributed_files_all_parse` test catches it before release.
#[must_use]
pub fn embedded() -> &'static LabelTable {
    static TABLE: OnceLock<LabelTable> = OnceLock::new();
    TABLE.get_or_init(|| LabelTable::from_embedded().unwrap_or_default())
}

impl LabelTable {
    /// Parses every embedded file, or reports the first one that is malformed.
    ///
    /// # Errors
    ///
    /// The path and the TOML error, for a file that does not parse, carries an unknown field, or
    /// sits outside `models/` and `prefixes/`.
    pub fn from_embedded() -> Result<Self, String> {
        let mut table = Self::default();
        for path in LabelFiles::iter() {
            if !path.ends_with(".toml") {
                continue;
            }
            let file = LabelFiles::get(&path).ok_or_else(|| format!("{path}: not readable"))?;
            let text = std::str::from_utf8(&file.data)
                .map_err(|error| format!("{path}: not UTF-8: {error}"))?;
            let parsed: LabelFile =
                toml::from_str(text).map_err(|error| format!("{path}: {error}"))?;
            match classify_path(&path) {
                Some(FileKind::Model(key)) => {
                    table.models.insert(key, parsed);
                }
                Some(FileKind::Prefix(key)) => {
                    table.prefixes.insert(key, parsed);
                }
                None => {
                    return Err(format!(
                        "{path}: not under models/<BRAND>/<VIB>.toml or prefixes/<Prefix>.toml"
                    ));
                }
            }
        }
        Ok(table)
    }

    /// The lookup for one appliance. `brand` and `vib` come from the profile manifest.
    #[must_use]
    pub fn for_device(&self, brand: &str, vib: &str) -> DeviceLabels<'_> {
        DeviceLabels {
            model: self.models.get(&model_key(brand, vib)),
            table: self,
        }
    }
}

/// What a file's path says it covers.
enum FileKind {
    Model(String),
    Prefix(String),
}

/// `models/BOSCH/SMD8TCX01E.toml` → model `BOSCH/SMD8TCX01E`; `prefixes/BSH.Common.toml` → prefix
/// `BSH.Common`. Anything else is rejected rather than guessed at.
fn classify_path(path: &str) -> Option<FileKind> {
    let stem = path.strip_suffix(".toml")?;
    if let Some(rest) = stem.strip_prefix("models/") {
        let (brand, vib) = rest.split_once('/')?;
        return Some(FileKind::Model(model_key(brand, vib)));
    }
    stem.strip_prefix("prefixes/")
        .map(|prefix| FileKind::Prefix(prefix.to_owned()))
}

/// The model-layer key, upper-cased so `bosch` and `BOSCH` match.
fn model_key(brand: &str, vib: &str) -> String {
    format!("{}/{}", brand.to_uppercase(), vib.to_uppercase())
}

/// The vendor/section prefix of a dotted key: its **first two** segments.
///
/// Not "everything before the last two", which would give `BSH.Common.Setting.Favorite` for the
/// six-segment favourite keys. A key with fewer than three segments is its own prefix.
#[must_use]
pub fn key_prefix(key: &str) -> &str {
    let mut boundaries = key.match_indices('.').map(|(index, _)| index);
    boundaries.next();
    boundaries.next().map_or(key, |end| {
        key.get(..end).unwrap_or(key) // `end` is a `.` boundary, so this cannot fail.
    })
}

/// The resolved lookup for one appliance: its model file, if any, over the shared prefix files.
#[derive(Debug, Clone, Copy)]
pub struct DeviceLabels<'a> {
    model: Option<&'a LabelFile>,
    table: &'a LabelTable,
}

impl DeviceLabels<'_> {
    /// The name for a feature key: model layer, then prefix layer, then the derived spelling.
    #[must_use]
    pub fn feature(&self, key: &str) -> Label {
        self.resolve(key, |file| &file.features)
    }

    /// The name for a program key. Same three layers, from the `[programs]` tables.
    #[must_use]
    pub fn program(&self, key: &str) -> Label {
        self.resolve(key, |file| &file.programs)
    }

    /// How prominently a feature should be shown, when a contributor said.
    ///
    /// Each field falls through the layers on its own, so a model file that renames a feature does
    /// not erase the prefix file's placement. Absent everywhere leaves `hc_core::mapping`'s
    /// heuristic in charge.
    #[must_use]
    pub fn curation(&self, key: &str) -> hc_core::presentation::Curation {
        let entries = || {
            self.layers(key)
                .into_iter()
                .filter_map(move |(file, _)| file.features.get(key))
        };
        hc_core::presentation::Curation {
            enabled_by_default: entries().find_map(|entry| entry.enabled_by_default),
            // No contributed word anywhere in the layers means no override.
            entity_category: entries()
                .find_map(|entry| entry.entity_category)
                .map_or(hc_core::presentation::CategoryOverride::Inherit, Into::into),
        }
    }

    /// The contributed icon for a feature, from the most specific layer that has one.
    #[must_use]
    pub fn icon(&self, key: &str) -> Option<String> {
        self.feature_field(key, |entry| entry.icon.clone())
    }

    /// The contributed `device_class` for a feature, from the most specific layer that has one.
    #[must_use]
    pub fn device_class(&self, key: &str) -> Option<String> {
        self.feature_field(key, |entry| entry.device_class.clone())
    }

    /// One field of a feature's entry, from the first layer that sets it.
    fn feature_field<T>(&self, key: &str, field: impl Fn(&Entry) -> Option<T>) -> Option<T> {
        self.layers(key)
            .into_iter()
            .filter_map(|(file, _)| file.features.get(key))
            .find_map(field)
    }

    /// The name for one enum member of a feature.
    ///
    /// Falls back to the member's own derived spelling, never to the feature's name.
    #[must_use]
    pub fn member(&self, key: &str, member: &str) -> Label {
        for (file, source) in self.layers(key) {
            if let Some(text) = file
                .features
                .get(key)
                .and_then(|entry| entry.values.get(member))
            {
                return Label {
                    text: text.clone(),
                    source,
                    description: None,
                };
            }
        }
        Label {
            text: humanize(member),
            source: LabelSource::Derived,
            description: None,
        }
    }

    /// Display names for `members`, keyed by the **raw** member name.
    ///
    /// Members with only a derived name are omitted, since the caller already has the raw member.
    /// The map is built by walking `members`, so its keys are always a subset of what the appliance
    /// declared.
    #[must_use]
    pub fn member_labels<'m>(
        &self,
        key: &str,
        members: impl IntoIterator<Item = &'m str>,
    ) -> BTreeMap<String, String> {
        members
            .into_iter()
            .filter_map(|member| {
                let resolved = self.member(key, member);
                (resolved.source != LabelSource::Derived)
                    .then(|| (member.to_owned(), resolved.text))
            })
            .collect()
    }

    /// The layers to consult for `key`, most specific first.
    fn layers(&self, key: &str) -> Vec<(&LabelFile, LabelSource)> {
        let mut layers = Vec::with_capacity(2);
        if let Some(model) = self.model {
            layers.push((model, LabelSource::Model));
        }
        if let Some(prefix) = self.table.prefixes.get(key_prefix(key)) {
            layers.push((prefix, LabelSource::Prefix));
        }
        layers
    }

    /// The shared body of [`Self::feature`] and [`Self::program`].
    fn resolve(&self, key: &str, table: impl Fn(&LabelFile) -> &BTreeMap<String, Entry>) -> Label {
        for (file, source) in self.layers(key) {
            if let Some((entry, text)) = table(file)
                .get(key)
                .and_then(|entry| Some((entry, entry.label.clone()?)))
            {
                return Label {
                    text,
                    source,
                    description: entry.description.clone(),
                };
            }
        }
        Label {
            text: derive(key),
            source: LabelSource::Derived,
            description: None,
        }
    }
}

/// The derived name for a whole key: [`humanize`] over the segment that carries the meaning.
///
/// That is normally the last segment. When the last segment is an all-digit index
/// (`Favorite.001`, `Zone.001`, `Cavity.001`), the preceding segment is prepended so the label
/// reads `Favorite 001` rather than `001`. The index keeps its leading zeros to match the key.
fn derive(key: &str) -> String {
    let mut segments = key.rsplit('.');
    let leaf = segments.next().unwrap_or(key);
    if leaf.is_empty() || !leaf.bytes().all(|byte| byte.is_ascii_digit()) {
        return humanize(leaf);
    }
    // An all-digit leaf with nothing in front of it is all there is to go on.
    segments
        .next()
        .filter(|named| !named.is_empty())
        .map_or_else(
            || humanize(leaf),
            |named| format!("{} {leaf}", humanize(named)),
        )
}

/// A human spelling of one dotted-key segment: `TimeLight` → `Time light`.
///
/// The fallback when no contributed entry covers a key. Every word in the output is a run of
/// characters from the input. Tokens break at:
///
/// - lower→upper, the ordinary camel hump (`TimeLight` → `Time` `Light`);
/// - an acronym run followed by a word, which breaks *before* the last capital
///   (`LEDBrightness` → `LED` `Brightness`) rather than after it;
/// - letter↔digit in both directions (`Eco50` → `Eco` `50`).
///
/// The first token keeps its case and the rest are lower-cased, except acronyms and numbers.
#[must_use]
pub fn humanize(segment: &str) -> String {
    let tokens = tokenize(segment);
    if tokens.is_empty() {
        return segment.to_owned();
    }
    let mut out = String::with_capacity(segment.len() + tokens.len());
    let mut index = 0_usize;
    // `get` rather than `[]`: library code avoids panicking constructs.
    while let Some(rest) = tokens.get(index..).filter(|rest| !rest.is_empty()) {
        if index > 0 {
            out.push(' ');
        }
        // A compound noun is rendered whole, so its own internal capitals survive the sentence-case
        // pass that would otherwise lower-case every token after the first.
        if let Some((rendering, span)) = compound_at(rest) {
            out.push_str(rendering);
            index += span;
            continue;
        }
        let Some(token) = rest.first() else { break };
        let token = respelled(token);
        if index == 0 || is_shouted(&token) {
            out.push_str(&token);
        } else {
            out.extend(token.chars().flat_map(char::to_lowercase));
        }
        index += 1;
    }
    out
}

/// Words the appliance spells the American way, in the British spelling the catalogue uses.
///
/// Without this a derived name says `Program aborted` beside a contributed `Running programme`.
const RESPELLINGS: [(&str, &str); 2] = [("Program", "Programme"), ("Programs", "Programmes")];

/// `token` in the catalogue's spelling. The first letter keeps its case.
fn respelled(token: &str) -> std::borrow::Cow<'_, str> {
    let capitalised = token.chars().next().is_some_and(char::is_uppercase);
    RESPELLINGS
        .iter()
        .find(|(american, _)| american.eq_ignore_ascii_case(token))
        .map_or(std::borrow::Cow::Borrowed(token), |(_, british)| {
            if capitalised {
                std::borrow::Cow::Borrowed(*british)
            } else {
                std::borrow::Cow::Owned(british.to_lowercase())
            }
        })
}

/// Whether a token must keep its case: an acronym (`LED`, `BSH`) or anything with no lower-case
/// letters (`50`).
fn is_shouted(token: &str) -> bool {
    !token.chars().any(char::is_lowercase)
}

/// Proper nouns that span more than one camel hump, and therefore more than one token.
///
/// `WiFi` tokenizes like `TimeLight` and would otherwise render as `Wi fi`; only a list of names can
/// tell them apart. Kept small on purpose: a general rule would mangle ordinary words such as
/// `BonelessLegOfPork`.
const COMPOUND_NOUNS: [(&[&str], &str); 1] = [
    // `Wi Fi` matches the spelling of Home Assistant's own `home_connect` integration.
    (&["Wi", "Fi"], "Wi Fi"),
];

/// The compound noun starting at `tokens[0]`, if there is one: its rendering and its token count.
fn compound_at(tokens: &[String]) -> Option<(&'static str, usize)> {
    COMPOUND_NOUNS.iter().find_map(|(pattern, rendering)| {
        let matches = tokens.len() >= pattern.len()
            && tokens
                .iter()
                .zip(pattern.iter())
                .all(|(token, expected)| token == expected);
        matches.then_some((*rendering, pattern.len()))
    })
}

/// Splits an identifier into its runs, at the three boundaries documented on [`humanize`].
///
/// Non-alphanumeric characters (`_`, a space) are kept, so the transform is lossless.
fn tokenize(segment: &str) -> Vec<String> {
    let chars: Vec<char> = segment.chars().collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;
    for (index, &ch) in chars.iter().enumerate() {
        if !current.is_empty() && starts_token(previous, ch, chars.get(index + 1).copied()) {
            tokens.push(std::mem::take(&mut current));
        }
        current.push(ch);
        previous = Some(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Whether `ch` begins a new token, given its neighbours.
///
/// Split before `ch` when:
/// - it is upper-case and the previous character was lower-case or a digit — the camel hump;
/// - it is upper-case, the previous was upper-case, and the *next* is lower-case — the acronym's
///   last capital belongs to the following word (`LEDBrightness` → `LED` `Brightness`);
/// - it is a digit and the previous was a letter, or a letter and the previous was a digit.
fn starts_token(previous: Option<char>, ch: char, next: Option<char>) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if ch.is_uppercase() {
        return previous.is_lowercase()
            || previous.is_ascii_digit()
            || (previous.is_uppercase() && next.is_some_and(char::is_lowercase));
    }
    if ch.is_ascii_digit() {
        return previous.is_alphabetic();
    }
    if ch.is_alphabetic() {
        return previous.is_ascii_digit();
    }
    // A separator rides along in the current token, so `humanize("_") == "_"`.
    false
}
