//! The wire format every tinyBird addon export shares.
//!
//! Hosts, overlays, and external tools agree on these types and nothing else.
//! A game-specific addon keeps whatever typed internals it likes in the `data`
//! field, but must also describe itself through `sections` so a consumer that
//! knows nothing about the game can still render something useful.
//!
//! The vocabulary is deliberately small, and every part of it earns its place
//! by being drawable without game knowledge:
//!
//! - [`AddonField`] is a label and a value, optionally carrying an [`AddonMeter`]
//!   (a bounded quantity, so a consumer can draw a bar) and an [`AddonTone`]
//!   (how the value reads — healthy, worth noticing, critical).
//! - [`AddonCard`] groups fields under a heading with its own headline stat and
//!   [`AddonBadge`]s. A party member, a clan unit, a battle opponent: anything
//!   the game has several of and each of which has more to say than one row.
//! - [`AddonSectionContent`] is the four shapes a section can take.
//!
//! A consumer that only understands `KeyValue`, `List`, and `Table` still
//! works: tone and meter are optional decorations on a field that always
//! carries a plain string value, and a card degrades to a heading with rows
//! under it.

use serde::Serialize;

use crate::registry::RomIdentity;

/// Bumped to 2 when fields gained meters and tones and sections gained cards.
pub const SNAPSHOT_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StreamSnapshot<T = serde_json::Value> {
    pub schema_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rom: Option<RomIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addon: Option<AddonSnapshot<T>>,
}

impl<T> Default for StreamSnapshot<T> {
    fn default() -> Self {
        Self {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            rom: None,
            addon: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonSnapshot<T = serde_json::Value> {
    pub addon_id: &'static str,
    pub display_name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sections: Vec<AddonSection>,
    pub overlay_lines: Vec<String>,
    pub data: T,
}

/// How a value reads at a glance.
///
/// This is the addon's judgement, not the consumer's: only the addon knows
/// that 4 HP out of 50 is critical while 4 PP out of 5 is fine. Consumers map
/// the tone onto whatever their own palette calls those states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AddonTone {
    /// Ordinary information, styled like everything else.
    #[default]
    Neutral,
    /// Healthy, complete, or otherwise going well.
    Good,
    /// Worth noticing before it becomes a problem.
    Warn,
    /// Actively bad: fainted, poisoned, out of a resource.
    Bad,
}

impl AddonTone {
    /// Neutral is the default, so it is left out of the JSON entirely.
    fn is_neutral(&self) -> bool {
        matches!(self, AddonTone::Neutral)
    }

    /// The usual three-band reading of "how much of this is left".
    ///
    /// Shared here rather than written out in each addon so that a bar means
    /// the same thing in every game: empty is bad, a quarter or less is worth
    /// warning about, anything above that is fine.
    pub fn from_fraction(value: u32, max: u32) -> Self {
        if max == 0 {
            return AddonTone::Neutral;
        }
        match value.min(max) * 100 / max {
            0 => AddonTone::Bad,
            1..=25 => AddonTone::Warn,
            _ => AddonTone::Good,
        }
    }
}

/// A bounded quantity, so a consumer can draw a bar without parsing "12/34".
///
/// Integral because everything it describes in practice is — hit points, PP,
/// percentages, item counts — and because it keeps the whole schema `Eq`,
/// which is what lets a host skip re-rendering an unchanged snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AddonMeter {
    pub value: u32,
    pub max: u32,
}

impl AddonMeter {
    pub fn new(value: u32, max: u32) -> Self {
        Self { value, max }
    }

    /// Filled percentage, clamped, for a consumer that just wants a width.
    ///
    /// A meter with no maximum reads as empty rather than as an error: an
    /// addon reporting 0/0 is saying the quantity does not apply right now.
    pub fn percent(&self) -> u32 {
        (self.value.min(self.max) * 100)
            .checked_div(self.max)
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonSection {
    pub section_id: &'static str,
    pub title: String,
    /// One line of context under the title: what this section is reading, or
    /// why it is thin. Optional, and consumers may drop it when space is tight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// A flag worth seeing from outside the section.
    ///
    /// Notes and fields are only read once the section is open, and a consumer
    /// that shows one section at a time is a consumer where "open" means "the
    /// other ones are not". This is the short word that rides on the tab, for
    /// the times an addon has spotted something the player would want to look
    /// at now rather than next time they happen to click through.
    ///
    /// Use it sparingly. A flag that is always there is a flag nobody sees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub badge: Option<AddonBadge>,
    #[serde(flatten)]
    pub content: AddonSectionContent,
}

impl AddonSection {
    pub fn new(
        section_id: &'static str,
        title: impl Into<String>,
        content: AddonSectionContent,
    ) -> Self {
        Self {
            section_id,
            title: title.into(),
            note: None,
            badge: None,
            content,
        }
    }

    pub fn key_value(
        section_id: &'static str,
        title: impl Into<String>,
        fields: Vec<AddonField>,
    ) -> Self {
        Self::new(section_id, title, AddonSectionContent::KeyValue(fields))
    }

    pub fn list(section_id: &'static str, title: impl Into<String>, items: Vec<String>) -> Self {
        Self::new(section_id, title, AddonSectionContent::List(items))
    }

    pub fn table(section_id: &'static str, title: impl Into<String>, table: AddonTable) -> Self {
        Self::new(section_id, title, AddonSectionContent::Table(table))
    }

    pub fn cards(
        section_id: &'static str,
        title: impl Into<String>,
        cards: Vec<AddonCard>,
    ) -> Self {
        Self::new(section_id, title, AddonSectionContent::Cards(cards))
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    pub fn with_badge(mut self, badge: AddonBadge) -> Self {
        self.badge = Some(badge);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum AddonSectionContent {
    KeyValue(Vec<AddonField>),
    List(Vec<String>),
    Table(AddonTable),
    /// Several things of the same shape, each with more to say than one row.
    Cards(Vec<AddonCard>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonField {
    pub label: String,
    pub value: String,
    /// The same quantity as a bar, when the value is "n of m".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meter: Option<AddonMeter>,
    #[serde(skip_serializing_if = "AddonTone::is_neutral")]
    pub tone: AddonTone,
    /// Secondary detail — a derived number, a source address, a caveat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl AddonField {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            meter: None,
            tone: AddonTone::default(),
            hint: None,
        }
    }

    /// A field whose value is "value/max", carrying the bar and the tone that
    /// reading implies. The one call that covers most live game stats.
    pub fn gauge(label: impl Into<String>, value: u32, max: u32) -> Self {
        Self::new(label, format!("{value}/{max}"))
            .with_meter(AddonMeter::new(value, max))
            .with_tone(AddonTone::from_fraction(value, max))
    }

    pub fn with_meter(mut self, meter: AddonMeter) -> Self {
        self.meter = Some(meter);
        self
    }

    pub fn with_tone(mut self, tone: AddonTone) -> Self {
        self.tone = tone;
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// A picture for a card: a species sprite, a portrait, an item icon.
///
/// `src` is a URL the consumer resolves, and a relative one is relative to
/// whatever is serving the page — which is how an addon can name a picture
/// without knowing whether it is being read by a web page, a stream overlay,
/// or a desktop window. A consumer that cannot fetch pictures ignores this and
/// still has the card title, which is why `alt` says the same thing in words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonImage {
    pub src: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

impl AddonImage {
    pub fn new(src: impl Into<String>) -> Self {
        Self {
            src: src.into(),
            alt: None,
        }
    }

    pub fn with_alt(mut self, alt: impl Into<String>) -> Self {
        self.alt = Some(alt.into());
        self
    }
}

/// A short status word, drawn as a chip rather than as a labelled row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonBadge {
    pub text: String,
    #[serde(skip_serializing_if = "AddonTone::is_neutral")]
    pub tone: AddonTone,
}

impl AddonBadge {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: AddonTone::default(),
        }
    }

    pub fn toned(text: impl Into<String>, tone: AddonTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// One member of a repeated set: a party slot, a clan unit, an opponent.
///
/// A `Table` row flattens all of this into strings of equal weight. A card
/// says which part is the headline (`lead`), which parts are flags (`badges`),
/// and which are supporting detail (`fields`), so a consumer can lay it out
/// well without knowing what game it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonCard {
    pub title: String,
    /// What the title actually is, when the title is a name — a species, a job.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// A picture of whatever this card is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<AddonImage>,
    /// The one number that matters most for this card, usually a gauge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead: Option<AddonField>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub badges: Vec<AddonBadge>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<AddonField>,
}

impl AddonCard {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            image: None,
            lead: None,
            badges: Vec::new(),
            fields: Vec::new(),
        }
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn with_image(mut self, image: AddonImage) -> Self {
        self.image = Some(image);
        self
    }

    pub fn with_lead(mut self, lead: AddonField) -> Self {
        self.lead = Some(lead);
        self
    }

    pub fn with_badges(mut self, badges: Vec<AddonBadge>) -> Self {
        self.badges = badges;
        self
    }

    pub fn with_fields(mut self, fields: Vec<AddonField>) -> Self {
        self.fields = fields;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl AddonTable {
    pub fn new(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self { columns, rows }
    }
}

impl<T> AddonSnapshot<T> {
    pub fn new(
        addon_id: &'static str,
        display_name: &'static str,
        overlay_lines: Vec<String>,
        data: T,
    ) -> Self {
        Self {
            addon_id,
            display_name,
            version: None,
            capabilities: Vec::new(),
            sections: Vec::new(),
            overlay_lines,
            data,
        }
    }

    pub fn with_version(mut self, version: &'static str) -> Self {
        self.version = Some(version);
        self
    }

    pub fn with_capabilities(mut self, capabilities: Vec<&'static str>) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_sections(mut self, sections: Vec<AddonSection>) -> Self {
        self.sections = sections;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_snapshot_exports_schema_only() {
        let snapshot = StreamSnapshot::<serde_json::Value>::default();
        let json = serde_json::to_value(snapshot).expect("serialize snapshot");

        assert_eq!(json, json!({ "schema_version": SNAPSHOT_SCHEMA_VERSION }));
    }

    #[test]
    fn addon_snapshot_exports_optional_metadata_when_present() {
        let addon = AddonSnapshot::new(
            "example.addon",
            "Example Addon",
            vec!["Ready".to_string()],
            json!({ "type": "example", "payload": { "ok": true } }),
        )
        .with_version("1.2.3")
        .with_capabilities(vec!["party", "battle"]);

        let json = serde_json::to_value(addon).expect("serialize addon");

        assert_eq!(json["addon_id"], "example.addon");
        assert_eq!(json["version"], "1.2.3");
        assert_eq!(json["capabilities"], json!(["party", "battle"]));
        assert_eq!(json["data"]["payload"]["ok"], true);
    }

    #[test]
    fn addon_sections_serialize_as_generic_blocks() {
        let section =
            AddonSection::key_value("summary", "Summary", vec![AddonField::new("Mode", "Live")]);
        let json = serde_json::to_value(section).expect("serialize section");

        assert_eq!(json["section_id"], "summary");
        assert_eq!(json["kind"], "key_value");
        assert_eq!(json["payload"][0]["label"], "Mode");
        assert_eq!(json["payload"][0]["value"], "Live");
    }

    /// A plain field must stay exactly as small on the wire as it was before
    /// meters and tones existed, or every consumer pays for a feature that
    /// most fields do not use.
    #[test]
    fn a_plain_field_carries_no_decoration_on_the_wire() {
        let json = serde_json::to_value(AddonField::new("Mode", "Live")).expect("serialize");
        assert_eq!(json, json!({ "label": "Mode", "value": "Live" }));
    }

    #[test]
    fn a_gauge_field_reads_as_text_and_as_a_bar() {
        let json = serde_json::to_value(AddonField::gauge("HP", 12, 48)).expect("serialize");

        // The string value is what a text-only consumer draws...
        assert_eq!(json["value"], "12/48");
        // ...and the meter is what a graphical one draws instead.
        assert_eq!(json["meter"], json!({ "value": 12, "max": 48 }));
        assert_eq!(json["tone"], "warn");
    }

    #[test]
    fn tone_bands_a_fraction_the_same_way_for_every_game() {
        assert_eq!(AddonTone::from_fraction(0, 50), AddonTone::Bad);
        assert_eq!(AddonTone::from_fraction(12, 50), AddonTone::Warn);
        assert_eq!(AddonTone::from_fraction(40, 50), AddonTone::Good);
        // Nothing sensible to say about a bar with no maximum.
        assert_eq!(AddonTone::from_fraction(0, 0), AddonTone::Neutral);
    }

    #[test]
    fn a_meter_clamps_rather_than_overflowing_its_bar() {
        assert_eq!(AddonMeter::new(60, 50).percent(), 100);
        assert_eq!(AddonMeter::new(0, 0).percent(), 0);
        assert_eq!(AddonMeter::new(25, 50).percent(), 50);
    }

    #[test]
    fn cards_name_their_headline_flags_and_detail_separately() {
        let section = AddonSection::cards(
            "party",
            "Party",
            vec![AddonCard::new("Pidgey")
                .with_subtitle("Slot 1")
                .with_lead(AddonField::gauge("HP", 0, 20))
                .with_badges(vec![AddonBadge::toned("Fainted", AddonTone::Bad)])
                .with_fields(vec![AddonField::new("Nature", "Brave")])],
        )
        .with_note("Read from the live party block");

        let json = serde_json::to_value(section).expect("serialize section");

        assert_eq!(json["kind"], "cards");
        assert_eq!(json["note"], "Read from the live party block");
        let card = &json["payload"][0];
        assert_eq!(card["title"], "Pidgey");
        assert_eq!(card["lead"]["meter"], json!({ "value": 0, "max": 20 }));
        assert_eq!(card["badges"][0]["tone"], "bad");
        assert_eq!(card["fields"][0]["label"], "Nature");
    }

    /// The tab flag is for the times a section has something worth leaving
    /// another section to look at.
    #[test]
    fn a_section_flag_rides_outside_the_section() {
        let section = AddonSection::list("battle", "In battle", vec!["Pidgey".to_string()])
            .with_badge(AddonBadge::toned("Great IVs", AddonTone::Good));

        let json = serde_json::to_value(&section).expect("serialize");
        assert_eq!(json["badge"]["text"], "Great IVs");
        assert_eq!(json["badge"]["tone"], "good");

        // And a section with nothing to flag says nothing, so a consumer can
        // draw the flag whenever it is there and never have to test for blank.
        let plain = AddonSection::list("battle", "In battle", Vec::new());
        assert!(serde_json::to_value(&plain).expect("serialize").get("badge").is_none());
    }

    /// An empty card is mostly absent from the JSON, so a consumer can tell
    /// "nothing to report" apart from "reported nothing".
    #[test]
    fn an_undecorated_card_omits_every_optional_part() {
        let json = serde_json::to_value(AddonCard::new("Empty")).expect("serialize");
        assert_eq!(json, json!({ "title": "Empty" }));
    }

    /// A picture is an addition to a card, never a replacement for its words:
    /// a consumer that cannot show images must still have something to read.
    #[test]
    fn a_card_with_a_picture_still_says_what_it_is_in_words() {
        let card = AddonCard::new("ZKTLS")
            .with_subtitle("Nidoran M")
            .with_image(AddonImage::new("/sprites/32").with_alt("Nidoran M"));

        let json = serde_json::to_value(&card).expect("serialize");
        assert_eq!(json["image"]["src"], "/sprites/32");
        assert_eq!(json["image"]["alt"], "Nidoran M");
        assert_eq!(json["title"], "ZKTLS");
        assert_eq!(json["subtitle"], "Nidoran M");
    }
}
