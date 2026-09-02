//! Addons written as data rather than as Rust.
//!
//! **Proof of concept.** Everything here works and is tested, but it covers a
//! deliberately small slice of what a compiled [`GameAddon`](crate::GameAddon)
//! can do. See the bottom of this file for what it cannot express yet.
//!
//! # Why
//!
//! Adding a game currently means writing a Rust module and rebuilding. That is
//! fine for the games this ships with and hopeless for the long tail — someone
//! who has found where their game keeps its gold has to become a contributor
//! to see it on screen.
//!
//! A manifest is a JSON file describing *where to read* and *what to call it*:
//!
//! ```json
//! {
//!   "addon_id": "custom.firered_money",
//!   "display_name": "Money",
//!   "matches": { "game_code_prefix": ["BPR", "BPG"] },
//!   "sections": [{
//!     "id": "wallet", "title": "Wallet", "kind": "key_value",
//!     "fields": [{ "label": "Money", "read": { "u32": "0x0300500C" } }]
//!   }]
//! }
//! ```
//!
//! # What this is really for
//!
//! The two halves of writing an addon are not equally hard. Deciding that a
//! number is worth a row, and what to call it, is easy and mechanical — it is
//! exactly the part a language model does well, and this format is small
//! enough to be a reliable generation target. Working out that `0x02024284` is
//! the party block and not a buffer that happens to look like one is the hard
//! part, and no amount of schema helps: it comes from `tinybird-probe`,
//! diffing memory across two savestates and watching what changed when it
//! should have.
//!
//! So the split this is built around is: **a human or a probe finds the
//! addresses, and the manifest is the cheap part anyone — or anything — can
//! write.** A manifest that names a wrong address produces confident nonsense,
//! which is why [`ManifestAddon::snapshot`] refuses to report a section whose
//! reads all came back zero: unmapped memory reads as zero, and a panel full
//! of zeroes is indistinguishable from a panel that is simply wrong.

use serde::Deserialize;

use crate::memory::MemoryView;
use crate::registry::{AddonInfo, GameAddon, RomIdentity};
use crate::schema::{
    AddonBadge, AddonCard, AddonField, AddonMeter, AddonSection, AddonSnapshot, AddonTone,
};

/// How many repeats a manifest may ask for.
///
/// A manifest is data, and data can be wrong: a stride of 1 and a count of a
/// million is a plausible typo, and without a cap it is a hang.
const MAX_REPEAT: u32 = 64;
/// Longest string a text read may pull out of memory.
const MAX_TEXT_LEN: u32 = 64;

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub addon_id: String,
    pub display_name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub matches: Matcher,
    #[serde(default)]
    pub sections: Vec<SectionSpec>,
}

/// Which ROMs this manifest claims.
///
/// Empty matches nothing rather than everything. A manifest that forgot to say
/// what it is for should be inert, not attached to every game someone loads.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Matcher {
    #[serde(default)]
    pub game_code_prefix: Vec<String>,
    #[serde(default)]
    pub title: Vec<String>,
}

impl Matcher {
    fn matches(&self, rom: &RomIdentity) -> bool {
        self.game_code_prefix
            .iter()
            .any(|prefix| rom.code_prefix().eq_ignore_ascii_case(prefix))
            || self
                .title
                .iter()
                .any(|title| rom.title.eq_ignore_ascii_case(title))
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SectionSpec {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(flatten)]
    pub body: SectionBody,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SectionBody {
    KeyValue {
        fields: Vec<FieldSpec>,
    },
    /// One card per repeat: a party, a squad, an inventory row.
    Cards {
        repeat: Repeat,
        card: CardSpec,
    },
}

/// A base address stepped `count` times, `stride` bytes apart.
#[derive(Clone, Debug, Deserialize)]
pub struct Repeat {
    pub count: u32,
    pub stride: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CardSpec {
    /// The heading. Usually a text read; a literal works for numbered slots.
    pub title: Value,
    #[serde(default)]
    pub subtitle: Option<Value>,
    #[serde(default)]
    pub lead: Option<FieldSpec>,
    #[serde(default)]
    pub fields: Vec<FieldSpec>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FieldSpec {
    pub label: String,
    pub read: Value,
    /// The other half of a gauge. With it, the field gets a bar and a tone.
    #[serde(default)]
    pub max: Option<Value>,
    #[serde(default)]
    pub hint: Option<String>,
}

/// Something to read out of memory, or a constant.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    U8(Address),
    U16(Address),
    U32(Address),
    Text {
        at: Address,
        len: u32,
    },
    /// A fixed string. Useful as a card title when the game stores no name.
    Literal(String),
    /// Which repeat this is, counting from one. Cards need slot numbers.
    Index,
}

/// Where to read.
///
/// A bare number is an absolute address. A `deref` chain follows pointers,
/// which most games need — Generation 3 keeps its save blocks behind one, and
/// their addresses move every time the game reloads them.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum Address {
    Direct(String),
    Chain {
        at: String,
        /// Offsets applied after each pointer read, in order.
        deref: Vec<i64>,
    },
}

impl Address {
    /// Resolve to an address, or `None` if the manifest is malformed or a
    /// pointer in the chain was null.
    fn resolve(&self, memory: &dyn MemoryView, step: u32) -> Option<u32> {
        match self {
            Address::Direct(text) => parse_address(text).map(|base| base.wrapping_add(step)),
            Address::Chain { at, deref } => {
                let mut cursor = parse_address(at)?;
                for (index, offset) in deref.iter().enumerate() {
                    let pointer = memory.read_u32(cursor);
                    // A null pointer means the game has not built that
                    // structure yet, which is a normal state and not an error.
                    if pointer == 0 {
                        return None;
                    }
                    cursor = apply_offset(pointer, *offset)?;
                    // The step belongs on the final address, not on every
                    // pointer along the way.
                    if index + 1 == deref.len() {
                        cursor = cursor.wrapping_add(step);
                    }
                }
                Some(cursor)
            }
        }
    }
}

fn apply_offset(base: u32, offset: i64) -> Option<u32> {
    let sum = i64::from(base) + offset;
    u32::try_from(sum).ok()
}

/// `"0x02024284"`, `"0x2024284"` or `"33702532"`.
fn parse_address(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    match trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => trimmed.parse().ok(),
    }
}

/// What a read produced, and whether it read anything at all.
struct Read {
    text: String,
    number: Option<u32>,
    /// False when every byte behind it was zero. Unmapped memory reads as
    /// zero, so this is how a wrong address is told from an empty one.
    live: bool,
}

impl Value {
    fn read(&self, memory: &dyn MemoryView, step: u32, index: u32) -> Read {
        match self {
            Value::Literal(text) => Read {
                text: text.clone(),
                number: None,
                live: true,
            },
            Value::Index => Read {
                text: (index + 1).to_string(),
                number: Some(index + 1),
                live: true,
            },
            Value::U8(at) | Value::U16(at) | Value::U32(at) => {
                let Some(address) = at.resolve(memory, step) else {
                    return Read::dead();
                };
                let number = match self {
                    Value::U8(_) => u32::from(memory.read_u8(address)),
                    Value::U16(_) => u32::from(memory.read_u16(address)),
                    _ => memory.read_u32(address),
                };
                Read {
                    text: number.to_string(),
                    number: Some(number),
                    live: number != 0,
                }
            }
            Value::Text { at, len } => {
                let Some(address) = at.resolve(memory, step) else {
                    return Read::dead();
                };
                let text =
                    crate::memory::read_ascii(memory, address, (*len).min(MAX_TEXT_LEN) as usize);
                let live = !text.trim().is_empty();
                Read {
                    text,
                    number: None,
                    live,
                }
            }
        }
    }
}

impl Read {
    fn dead() -> Self {
        Read {
            text: String::new(),
            number: None,
            live: false,
        }
    }
}

/// A manifest, wearing the same trait a compiled addon does.
///
/// Nothing downstream knows the difference: the registry, the export envelope,
/// the web read-out and the stream overlay all see an `AddonSnapshot` and have
/// no way to tell whether a person or a JSON file described it.
pub struct ManifestAddon {
    manifest: Manifest,
    info: AddonInfo,
}

impl ManifestAddon {
    /// Parse a manifest, leaking the few strings that [`AddonInfo`] needs for
    /// the lifetime of the process.
    ///
    /// The leak is deliberate and bounded: `AddonInfo` is `&'static str`
    /// because compiled addons have string literals, and manifests are loaded
    /// once at startup and live until exit. A few dozen bytes per addon, never
    /// in a loop.
    pub fn parse(json: &str) -> Result<Self, String> {
        let manifest: Manifest = serde_json::from_str(json).map_err(|err| err.to_string())?;
        Self::new(manifest)
    }

    pub fn new(manifest: Manifest) -> Result<Self, String> {
        if manifest.addon_id.trim().is_empty() {
            return Err("a manifest needs an addon_id".to_string());
        }
        if manifest.sections.is_empty() {
            return Err(format!(
                "{} describes no sections, so it would never show anything",
                manifest.addon_id
            ));
        }

        let info = AddonInfo {
            addon_id: Box::leak(manifest.addon_id.clone().into_boxed_str()),
            display_name: Box::leak(manifest.display_name.clone().into_boxed_str()),
            version: Box::leak(
                manifest
                    .version
                    .clone()
                    .unwrap_or_else(|| "0.0.0".to_string())
                    .into_boxed_str(),
            ),
            capabilities: Box::leak(
                manifest
                    .sections
                    .iter()
                    .map(|section| &*Box::leak(section.id.clone().into_boxed_str()))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            ),
            supported_games: Box::leak(describe_matcher(&manifest.matches).into_boxed_str()),
        };

        Ok(Self { manifest, info })
    }
}

fn describe_matcher(matcher: &Matcher) -> String {
    let mut parts = Vec::new();
    if !matcher.game_code_prefix.is_empty() {
        parts.push(matcher.game_code_prefix.join(", "));
    }
    if !matcher.title.is_empty() {
        parts.push(matcher.title.join(", "));
    }
    if parts.is_empty() {
        "nothing (no matcher)".to_string()
    } else {
        parts.join(" / ")
    }
}

impl<T: Default> GameAddon<T> for ManifestAddon {
    fn info(&self) -> AddonInfo {
        self.info
    }

    fn supports(&self, rom: &RomIdentity) -> bool {
        self.manifest.matches.matches(rom)
    }

    fn snapshot(&self, memory: &dyn MemoryView, _rom: &RomIdentity) -> Option<AddonSnapshot<T>> {
        let sections: Vec<AddonSection> = self
            .manifest
            .sections
            .iter()
            .filter_map(|spec| build_section(spec, memory))
            .collect();

        // Every section read nothing but zeroes. The manifest is either
        // pointed at the wrong addresses or the game has not built those
        // structures yet, and reporting a panel of zeroes would make the two
        // look identical.
        if sections.is_empty() {
            return None;
        }

        let overlay_lines = sections
            .iter()
            .map(|section| section.title.clone())
            .collect();

        Some(
            AddonSnapshot::new(
                self.info.addon_id,
                self.info.display_name,
                overlay_lines,
                T::default(),
            )
            .with_version(self.info.version)
            .with_capabilities(self.info.capabilities.to_vec())
            .with_sections(sections),
        )
    }
}

fn build_section(spec: &SectionSpec, memory: &dyn MemoryView) -> Option<AddonSection> {
    let id: &'static str = Box::leak(spec.id.clone().into_boxed_str());

    let section = match &spec.body {
        SectionBody::KeyValue { fields } => {
            let built: Vec<AddonField> = fields
                .iter()
                .filter_map(|field| build_field(field, memory, 0, 0))
                .collect();
            if built.is_empty() {
                return None;
            }
            AddonSection::key_value(id, spec.title.clone(), built)
        }
        SectionBody::Cards { repeat, card } => {
            let cards: Vec<AddonCard> = (0..repeat.count.min(MAX_REPEAT))
                .filter_map(|index| build_card(card, memory, index * repeat.stride, index))
                .collect();
            if cards.is_empty() {
                return None;
            }
            AddonSection::cards(id, spec.title.clone(), cards)
        }
    };

    Some(match &spec.note {
        Some(note) => section.with_note(note.clone()),
        None => section,
    })
}

fn build_card(
    spec: &CardSpec,
    memory: &dyn MemoryView,
    step: u32,
    index: u32,
) -> Option<AddonCard> {
    let title = spec.title.read(memory, step, index);
    // An empty slot is not an error — a party of three has three live slots
    // and three dead ones, and drawing the dead ones would be inventing them.
    if !title.live {
        return None;
    }

    let mut card = AddonCard::new(title.text);
    if let Some(subtitle) = &spec.subtitle {
        let read = subtitle.read(memory, step, index);
        if read.live {
            card = card.with_subtitle(read.text);
        }
    }
    if let Some(lead) = &spec.lead {
        if let Some(field) = build_field(lead, memory, step, index) {
            card = card.with_lead(field);
        }
    }

    let fields: Vec<AddonField> = spec
        .fields
        .iter()
        .filter_map(|field| build_field(field, memory, step, index))
        .collect();
    if !fields.is_empty() {
        card = card.with_fields(fields);
    }
    Some(card)
}

fn build_field(
    spec: &FieldSpec,
    memory: &dyn MemoryView,
    step: u32,
    index: u32,
) -> Option<AddonField> {
    let read = spec.read.read(memory, step, index);
    if !read.live {
        return None;
    }

    // Both halves of a gauge have to be readable for a bar to mean anything.
    let field = match (&spec.max, read.number) {
        (Some(max), Some(value)) => {
            let max_read = max.read(memory, step, index);
            match max_read.number.filter(|max| *max > 0) {
                Some(max) => AddonField::new(spec.label.clone(), format!("{value}/{max}"))
                    .with_meter(AddonMeter::new(value, max))
                    .with_tone(AddonTone::from_fraction(value, max)),
                None => AddonField::new(spec.label.clone(), read.text),
            }
        }
        _ => AddonField::new(spec.label.clone(), read.text),
    };

    Some(match &spec.hint {
        Some(hint) => field.with_hint(hint.clone()),
        None => field,
    })
}

/// A badge a manifest can raise. Not wired to anything yet; here so the shape
/// of a future `"flag_when"` rule is obvious rather than invented later.
#[allow(dead_code)]
fn unused_badge() -> AddonBadge {
    AddonBadge::new("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::SparseMemory;
    use crate::schema::AddonSectionContent;

    /// The typed payload a host would supply. Manifests have none of their own.
    #[derive(Default)]
    struct NoData;

    fn rom(code: &str) -> RomIdentity {
        RomIdentity {
            title: "POKEMON FIRE".to_string(),
            game_code: code.to_string(),
            maker_code: "01".to_string(),
            revision: 0,
            // Not a real dump; nothing here reads the fingerprint.
            fingerprint: 0,
        }
    }

    fn addon(json: &str) -> ManifestAddon {
        ManifestAddon::parse(json).expect("manifest should parse")
    }

    fn read_of(addon: &ManifestAddon, memory: &dyn MemoryView) -> Option<AddonSnapshot<NoData>> {
        GameAddon::<NoData>::snapshot(addon, memory, &rom("BPRE"))
    }

    const WALLET: &str = r#"{
      "addon_id": "custom.wallet",
      "display_name": "Wallet",
      "version": "0.1.0",
      "matches": { "game_code_prefix": ["BPR", "BPG"] },
      "sections": [{
        "id": "wallet",
        "title": "Wallet",
        "note": "Read from the save block",
        "kind": "key_value",
        "fields": [
          { "label": "Money", "read": { "u32": "0x02000100" } },
          { "label": "Badges", "read": { "u8": "0x02000110" } }
        ]
      }]
    }"#;

    #[test]
    fn a_manifest_claims_only_the_games_it_names() {
        let addon = addon(WALLET);
        assert!(GameAddon::<NoData>::supports(&addon, &rom("BPRE")));
        assert!(GameAddon::<NoData>::supports(&addon, &rom("BPGE")));
        assert!(!GameAddon::<NoData>::supports(&addon, &rom("AFXE")));
    }

    /// A manifest with no matcher is inert. The alternative — matching
    /// everything — would attach a half-written file to every game someone
    /// loads, which is the worst possible default for a format people hand-edit.
    #[test]
    fn a_manifest_with_no_matcher_claims_nothing() {
        let addon = addon(
            r#"{
              "addon_id": "custom.empty",
              "display_name": "Empty",
              "sections": [{ "id": "s", "title": "S", "kind": "key_value",
                "fields": [{ "label": "X", "read": { "u8": "0x02000000" } }] }]
            }"#,
        );
        assert!(!GameAddon::<NoData>::supports(&addon, &rom("BPRE")));
    }

    #[test]
    fn reads_become_a_section_of_labelled_rows() {
        let memory = SparseMemory::new()
            .with(0x0200_0100, vec![0xE8, 0x03, 0x00, 0x00])
            .with(0x0200_0110, vec![3]);

        let snapshot = read_of(&addon(WALLET), &memory).expect("a live section");
        assert_eq!(snapshot.addon_id, "custom.wallet");
        assert_eq!(snapshot.sections.len(), 1);
        assert_eq!(
            snapshot.sections[0].note.as_deref(),
            Some("Read from the save block")
        );

        match &snapshot.sections[0].content {
            AddonSectionContent::KeyValue(fields) => {
                assert_eq!(fields[0].label, "Money");
                assert_eq!(fields[0].value, "1000");
                assert_eq!(fields[1].value, "3");
            }
            other => panic!("expected key/value, got {other:?}"),
        }
    }

    /// The failure mode that matters. Unmapped memory reads as zero, so a
    /// manifest pointed at the wrong address produces a panel of zeroes that
    /// looks exactly like a panel that is merely idle.
    #[test]
    fn a_manifest_pointed_at_nothing_reports_nothing() {
        assert!(read_of(&addon(WALLET), &SparseMemory::new()).is_none());
    }

    #[test]
    fn a_gauge_needs_both_halves_to_draw_a_bar() {
        let json = r#"{
          "addon_id": "custom.hp", "display_name": "HP",
          "matches": { "game_code_prefix": ["BPR"] },
          "sections": [{ "id": "hp", "title": "HP", "kind": "key_value",
            "fields": [{ "label": "HP",
              "read": { "u16": "0x02000000" }, "max": { "u16": "0x02000002" } }] }]
        }"#;

        let both = SparseMemory::new().with(0x0200_0000, vec![0x0A, 0x00, 0x28, 0x00]);
        let snapshot = read_of(&addon(json), &both).expect("a live section");
        match &snapshot.sections[0].content {
            AddonSectionContent::KeyValue(fields) => {
                assert_eq!(fields[0].value, "10/40");
                assert_eq!(fields[0].meter, Some(AddonMeter::new(10, 40)));
                assert_eq!(fields[0].tone, AddonTone::Warn);
            }
            other => panic!("expected key/value, got {other:?}"),
        }

        // A maximum of zero is not a bar, it is a division by zero waiting to
        // happen. The value still reports.
        let no_max = SparseMemory::new().with(0x0200_0000, vec![0x0A, 0x00, 0x00, 0x00]);
        let snapshot = read_of(&addon(json), &no_max).expect("a live section");
        match &snapshot.sections[0].content {
            AddonSectionContent::KeyValue(fields) => {
                assert_eq!(fields[0].value, "10");
                assert_eq!(fields[0].meter, None);
            }
            other => panic!("expected key/value, got {other:?}"),
        }
    }

    const PARTY: &str = r#"{
      "addon_id": "custom.party",
      "display_name": "Party",
      "matches": { "game_code_prefix": ["BPR"] },
      "sections": [{
        "id": "party", "title": "Party", "kind": "cards",
        "repeat": { "count": 6, "stride": 8 },
        "card": {
          "title": { "text": { "at": "0x02000000", "len": 4 } },
          "subtitle": { "index": null },
          "lead": { "label": "HP",
            "read": { "u16": "0x02000004" }, "max": { "u16": "0x02000006" } }
        }
      }]
    }"#;

    #[test]
    fn a_repeat_becomes_one_card_per_slot() {
        // Two live slots, then nothing: a party of two.
        let memory = SparseMemory::new()
            .with(
                0x0200_0000,
                vec![b'A', b'B', b'C', 0, 0x0A, 0x00, 0x28, 0x00],
            )
            .with(
                0x0200_0008,
                vec![b'X', b'Y', b'Z', 0, 0x28, 0x00, 0x28, 0x00],
            );

        let snapshot = read_of(&addon(PARTY), &memory).expect("a live section");
        match &snapshot.sections[0].content {
            AddonSectionContent::Cards(cards) => {
                // The four empty slots are absent, not drawn as blanks.
                assert_eq!(cards.len(), 2);
                assert_eq!(cards[0].title, "ABC");
                assert_eq!(cards[0].subtitle.as_deref(), Some("1"));
                assert_eq!(cards[0].lead.as_ref().unwrap().value, "10/40");
                assert_eq!(cards[1].title, "XYZ");
                assert_eq!(cards[1].subtitle.as_deref(), Some("2"));
            }
            other => panic!("expected cards, got {other:?}"),
        }
    }

    /// Generation 3 keeps its save blocks behind a pointer that moves whenever
    /// the game reloads them, so a manifest that cannot follow one cannot read
    /// anything durable.
    #[test]
    fn an_address_can_follow_a_pointer() {
        let json = r#"{
          "addon_id": "custom.deref", "display_name": "Deref",
          "matches": { "game_code_prefix": ["BPR"] },
          "sections": [{ "id": "s", "title": "S", "kind": "key_value",
            "fields": [{ "label": "Money",
              "read": { "u32": { "at": "0x03005008", "deref": [4] } } }] }]
        }"#;

        let memory = SparseMemory::new()
            // The pointer...
            .with(0x0300_5008, vec![0x00, 0x00, 0x02, 0x02])
            // ...and four bytes past where it lands.
            .with(0x0202_0004, vec![0x2C, 0x01, 0x00, 0x00]);

        let snapshot = read_of(&addon(json), &memory).expect("a live section");
        match &snapshot.sections[0].content {
            AddonSectionContent::KeyValue(fields) => assert_eq!(fields[0].value, "300"),
            other => panic!("expected key/value, got {other:?}"),
        }

        // A null pointer is the game not having built the block yet, which is
        // an ordinary state and must not be reported as a reading of zero.
        assert!(read_of(&addon(json), &SparseMemory::new()).is_none());
    }

    #[test]
    fn a_manifest_that_could_never_show_anything_is_refused_at_load() {
        let err = match ManifestAddon::parse(
            r#"{ "addon_id": "custom.x", "display_name": "X", "sections": [] }"#,
        ) {
            Err(err) => err,
            Ok(_) => panic!("a manifest with no sections should be refused"),
        };
        assert!(err.contains("no sections"), "{err}");

        assert!(ManifestAddon::parse("not json").is_err());
    }

    /// A manifest is hand-edited data and a stride of one with a count of a
    /// million is a plausible typo. It has to be a bounded mistake.
    #[test]
    fn an_absurd_repeat_is_capped_rather_than_run() {
        let json = r#"{
          "addon_id": "custom.huge", "display_name": "Huge",
          "matches": { "game_code_prefix": ["BPR"] },
          "sections": [{ "id": "s", "title": "S", "kind": "cards",
            "repeat": { "count": 4000000, "stride": 1 },
            "card": { "title": { "literal": "row" } } }]
        }"#;

        let snapshot = read_of(&addon(json), &SparseMemory::new()).expect("literals are live");
        match &snapshot.sections[0].content {
            AddonSectionContent::Cards(cards) => assert_eq!(cards.len() as u32, MAX_REPEAT),
            other => panic!("expected cards, got {other:?}"),
        }
    }

    /// The example that ships in `addons/` is documentation, and documentation
    /// that no longer parses is worse than none.
    #[test]
    fn the_shipped_example_manifest_still_loads() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("addons/example.firered-trainer.json");
        let json = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));

        let addon = ManifestAddon::parse(&json).expect("the shipped example should parse");
        assert!(GameAddon::<NoData>::supports(&addon, &rom("BPRE")));
        // Two sections, and the capabilities are their ids.
        assert_eq!(
            GameAddon::<NoData>::info(&addon).capabilities,
            ["trainer", "party_lite"]
        );
    }

    #[test]
    fn addresses_parse_as_hex_or_decimal() {
        assert_eq!(parse_address("0x02024284"), Some(0x0202_4284));
        assert_eq!(parse_address("  0X20 "), Some(0x20));
        assert_eq!(parse_address("33702532"), Some(33_702_532));
        assert_eq!(parse_address("nonsense"), None);
    }
}

// ---------------------------------------------------------------------------
// What this proof of concept does not do yet
//
// - No decryption. Generation 3 party slots are XOR-encrypted with a key
//   derived from two other fields and their substructures are reordered by
//   personality value. `pokemon_frlg.rs` does that in about eighty lines, and
//   no declarative format is going to express it. The manifest can read a
//   party's *unencrypted* fields — nickname, level, current and maximum HP —
//   which is most of what a read-out shows.
// - No derived values. There is no arithmetic, no lookup tables, and so no
//   "species 16 is Pidgey" and no "IV total out of 186".
// - No conditions. A section cannot appear only during a battle, which is what
//   the dex tab does.
// - No tone or badge rules, so nothing can flag itself the way the IV check does.
//
// The honest summary is that a manifest can express a *reader* and not an
// *interpreter*. That covers a surprising amount — anything a game stores
// plainly — and it stops exactly where a game starts encoding things.
//
// Conditions and simple arithmetic are the two worth adding next, in that
// order: conditions because a section that is empty half the time is the
// commonest shape, and arithmetic because totals and percentages are what turn
// a number into a bar.
