//! Validated, read-only game readers expressed as JSON data.
//!
//! Browser hosts keep `Manifest` values in their emulator session and request
//! owned sections. `ManifestAddon` adapts the same format to the desktop's
//! startup registry. Numeric zero is valid; use `when` for readiness checks.
//! Memory reads, pointers, strings, repeats, and manifest size are bounded.

use serde::{Deserialize, Serialize};

use crate::memory::MemoryView;
use crate::registry::{AddonInfo, GameAddon, RomIdentity};
use crate::schema::{
    AddonBadge, AddonCard, AddonField, AddonImage, AddonMeter, AddonSection, AddonSnapshot,
    AddonTone,
};

/// How many repeats a manifest may ask for.
///
/// A manifest is data, and data can be wrong: a stride of 1 and a count of a
/// million is a plausible typo, and without a cap it is a hang.
const MAX_REPEAT: u32 = 64;
/// Longest string a text read may pull out of memory.
const MAX_TEXT_LEN: u32 = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default, rename = "$comment", skip_serializing_if = "Option::is_none")]
    pub comment: Option<serde_json::Value>,
    #[serde(default = "manifest_version")]
    pub manifest_version: u32,
    pub addon_id: String,
    pub display_name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub matches: Matcher,
    #[serde(default)]
    pub sections: Vec<SectionSpec>,
    /// Optional readiness check; numeric zero is valid data in version 2.
    #[serde(default)]
    pub when: Option<Condition>,
}

fn manifest_version() -> u32 {
    1
}

pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_INSTALLED_MANIFESTS: usize = 16;

impl Manifest {
    pub fn parse(json: &str) -> Result<Self, String> {
        if json.len() > MAX_MANIFEST_BYTES {
            return Err("Add-ons must be at most 64 KiB.".into());
        }
        let manifest: Self = serde_json::from_str(json).map_err(|err| err.to_string())?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), String> {
        let valid_id = |s: &str| {
            !s.is_empty()
                && s.len() <= 80
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        };
        if !matches!(self.manifest_version, 1 | 2) {
            return Err("Unsupported manifest_version; use 1 or 2.".into());
        }
        if !valid_id(&self.addon_id)
            || self.display_name.trim().is_empty()
            || self.display_name.len() > 120
        {
            return Err("Use a short add-on ID and a display name of at most 120 bytes.".into());
        }
        if self.sections.is_empty() || self.sections.len() > 8 {
            return Err("Use between 1 and 8 sections.".into());
        }
        if self.matches.game_code_prefix.len()
            + self.matches.game_code.len()
            + self.matches.title.len()
            > 32
            || self.matches.revision.len() > 16
        {
            return Err("Too many compatibility rules.".into());
        }
        if self
            .matches
            .game_code_prefix
            .iter()
            .any(|s| s.len() != 3 || !s.is_ascii())
            || self
                .matches
                .game_code
                .iter()
                .any(|s| s.len() != 4 || !s.is_ascii())
            || self
                .matches
                .title
                .iter()
                .any(|s| s.is_empty() || s.len() > 12)
        {
            return Err("Game prefixes need 3 ASCII characters; full codes need 4; titles at most 12 bytes.".into());
        }
        if self.matches.game_code_prefix.is_empty()
            && self.matches.game_code.is_empty()
            && self.matches.title.is_empty()
        {
            return Err("Choose at least one compatible game.".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut work = 0usize;
        for section in &self.sections {
            if !valid_id(&section.id) || !ids.insert(&section.id) {
                return Err("Section IDs must be valid and unique.".into());
            }
            if section.title.is_empty()
                || section.title.len() > 120
                || section.note.as_ref().is_some_and(|s| s.len() > 512)
            {
                return Err("Section titles need 1–120 bytes; notes at most 512.".into());
            }
            match &section.body {
                SectionBody::KeyValue { fields } => {
                    // Naming the section matters: a reader under construction
                    // has several, and "use 1-32 fields" sends the author
                    // hunting through all of them for the one that is empty.
                    if fields.is_empty() {
                        return Err(format!("\"{}\" has no fields yet.", section.title));
                    }
                    if fields.len() > 32 {
                        return Err(format!("\"{}\" has more than 32 fields.", section.title));
                    }
                    work += fields.len() * 2;
                    for field in fields {
                        validate_field(field)?;
                    }
                }
                SectionBody::Cards { repeat, card } => {
                    if repeat.count == 0
                        || repeat.count > MAX_REPEAT
                        || card.fields.len() > 32
                        || repeat.count.checked_mul(repeat.stride).is_none()
                    {
                        return Err("Invalid card count or stride.".into());
                    }
                    work += repeat.count as usize * (card.fields.len() * 2 + 4);
                    validate_value(&card.title)?;
                    if let Some(value) = &card.subtitle {
                        validate_value(value)?;
                    }
                    if let Some(image) = &card.image {
                        // A sprite is another whole-record decrypt per card,
                        // so it costs what a species read costs.
                        work += repeat.count as usize * 2;
                        validate_value(&Value::Gen3Species(image.address().clone()))?;
                    }
                    for field in card.fields.iter().chain(card.lead.iter()) {
                        validate_field(field)?;
                    }
                }
            }
        }
        if work > 2048 {
            return Err("This add-on requests too much work per update.".into());
        }
        if let Some(condition) = &self.when {
            if !matches!(condition.read, Value::U8(_) | Value::U16(_) | Value::U32(_)) {
                return Err("Readiness conditions must compare a numeric memory read.".into());
            }
            validate_value(&condition.read)?;
        }
        let json = serde_json::to_value(self).map_err(|err| err.to_string())?;
        fn strings_bounded(value: &serde_json::Value) -> bool {
            match value {
                serde_json::Value::String(s) => s.len() <= 2048,
                serde_json::Value::Array(a) => a.iter().all(strings_bounded),
                serde_json::Value::Object(o) => o.values().all(strings_bounded),
                _ => true,
            }
        }
        if !strings_bounded(&json) || json.to_string().len() > MAX_MANIFEST_BYTES {
            return Err("Manifest text is too large.".into());
        }
        Ok(())
    }

    pub fn supports(&self, rom: &RomIdentity) -> bool {
        self.matches.matches(rom)
    }

    /// Whether anything here needs the cartridge's own name tables.
    ///
    /// Finding those is one pass over the whole ROM, which is far outside the
    /// per-update read budget [`evaluate`](Self::evaluate) enforces. So the
    /// host does it once, before evaluating, and only when a manifest actually
    /// asks for a name — see [`prepare`](Self::prepare).
    pub fn needs_cartridge_names(&self) -> bool {
        fn named(value: &Value) -> bool {
            match value {
                Value::Gen3Species(_) => true,
                Value::Gen3 { field, .. } => field.needs_names(),
                _ => false,
            }
        }
        self.sections.iter().any(|section| match &section.body {
            SectionBody::KeyValue { fields } => fields.iter().any(|field| named(&field.read)),
            SectionBody::Cards { card, .. } => {
                card.image.is_some()
                    || named(&card.title)
                    || card.subtitle.as_ref().is_some_and(named)
                    || card
                        .fields
                        .iter()
                        .chain(card.lead.iter())
                        .any(|field| named(&field.read))
            }
        })
    }

    /// Read whatever this manifest needs from the cartridge before evaluating.
    ///
    /// Cheap after the first call for a given ROM, and a no-op for the manifest
    /// that never asks for a species. Give it *unbounded* memory: the read
    /// budget exists to stop a manifest scanning RAM every frame, and this is
    /// the one read that is allowed to be large because it happens once.
    pub fn prepare(&self, memory: &dyn MemoryView, rom: &RomIdentity) {
        if self.needs_cartridge_names() {
            crate::gen3_names::ensure(memory, rom);
        }
    }

    /// Owned sections for reloadable browser add-ons. No leaked metadata.
    pub fn sections(&self, memory: &dyn MemoryView) -> Vec<AddonSection> {
        self.evaluate(memory).unwrap_or_default()
    }

    pub fn evaluate(&self, memory: &dyn MemoryView) -> Result<Vec<AddonSection>, &'static str> {
        let memory = BoundedMemory {
            inner: memory,
            remaining: std::cell::Cell::new(16384),
            decrypted: std::cell::RefCell::new(None),
        };
        if self.when.as_ref().is_some_and(|condition| {
            condition.read.read(&memory, 0, 0).number != Some(condition.equals)
        }) {
            return Ok(Vec::new());
        }
        let sections: Vec<_> = self
            .sections
            .iter()
            .filter_map(|spec| build_section(spec, &memory))
            .collect();
        if memory.remaining.get() == 0 {
            return Err("Memory-read budget exhausted.");
        }
        if serde_json::to_vec(&sections).map_or(true, |bytes| bytes.len() > 32768) {
            return Err("Output exceeds 32 KiB. Reduce fields or repeated cards.");
        }
        Ok(sections)
    }
}

fn readable(addr: u32, len: u32) -> bool {
    [
        (0x02000000u32, 0x02040000u32),
        (0x03000000, 0x03008000),
        (0x08000000, 0x0e000000),
    ]
    .iter()
    .any(|&(start, end)| addr >= start && addr.checked_add(len).is_some_and(|last| last <= end))
}

/// The read budget, plus a memo for the one read that is expensive.
///
/// Every field of a card names the same record address, and decrypting that
/// record costs twelve word reads and a checksum. A party card showing four
/// moves, six effort values and six individual values would pay for that
/// sixteen times if each field decrypted for itself — which is most of the
/// budget spent re-deriving a value that cannot have changed. Cards are built
/// one at a time and every field in a card shares an address, so a single slot
/// keyed by address catches all of it.
struct BoundedMemory<'a> {
    inner: &'a dyn MemoryView,
    remaining: std::cell::Cell<usize>,
    decrypted: std::cell::RefCell<Option<(u32, Option<std::rc::Rc<crate::gen3::Boxed>>)>>,
}

impl BoundedMemory<'_> {
    /// The decrypted record at `at`, decrypting only if this is not the record
    /// the last field asked about. A miss caches too, so six empty party slots
    /// cost one failed decrypt each rather than one per field.
    fn boxed(&self, at: u32) -> Option<std::rc::Rc<crate::gen3::Boxed>> {
        if let Some((cached, value)) = self.decrypted.borrow().as_ref() {
            if *cached == at {
                return value.clone();
            }
        }
        let value = crate::gen3::Boxed::read(self, at).map(std::rc::Rc::new);
        *self.decrypted.borrow_mut() = Some((at, value.clone()));
        value
    }
}
impl MemoryView for BoundedMemory<'_> {
    fn read_u8(&self, addr: u32) -> u8 {
        let left = self.remaining.get();
        if left == 0 || !readable(addr, 1) {
            return 0;
        }
        self.remaining.set(left - 1);
        self.inner.read_u8(addr)
    }
}

fn validate_field(field: &FieldSpec) -> Result<(), String> {
    if field.label.is_empty()
        || field.label.len() > 120
        || field.hint.as_ref().is_some_and(|s| s.len() > 256)
    {
        return Err("Field labels need 1–120 bytes; hints at most 256.".into());
    }
    validate_value(&field.read)?;
    if let Some(max) = &field.max {
        validate_value(max)?;
    }
    Ok(())
}

fn validate_value(value: &Value) -> Result<(), String> {
    let address = match value {
        Value::U8(at) | Value::U16(at) | Value::U32(at) => at,
        Value::Text { at, len } | Value::Gen3Text { at, len } => {
            if *len == 0 || *len > MAX_TEXT_LEN {
                return Err("Text reads need 1–64 bytes.".into());
            }
            at
        }
        Value::Gen3Species(at) | Value::Gen3 { at, .. } => at,
        // A constant reads nothing, so there is no address to bound.
        Value::Const(_) => return Ok(()),
        Value::Literal(text) => {
            return if text.len() <= 256 {
                Ok(())
            } else {
                Err("Literal values need at most 256 bytes.".into())
            }
        }
        Value::Index => return Ok(()),
    };
    let at = match address {
        Address::Direct(at) => at,
        Address::Chain { at, deref } => {
            if deref.is_empty()
                || deref.len() > 4
                || deref.iter().any(|n| !(-33554432..=33554432).contains(n))
            {
                return Err("Pointer chains need 1–4 bounded offsets.".into());
            }
            at
        }
    };
    let width = if matches!(address, Address::Chain { .. }) { 4 } else {
        match value {
            Value::U8(_) => 1,
            Value::U16(_) => 2,
            Value::Text { len, .. } | Value::Gen3Text { len, .. } => *len,
            // Decrypting a record means reading all of the boxed part of it.
            Value::Gen3Species(_) | Value::Gen3 { .. } => crate::gen3::BOXED_BYTES,
            _ => 4,
        }
    };
    if !parse_address(at).is_some_and(|addr| readable(addr, width)) {
        return Err("Read addresses must be in GBA RAM or cartridge ROM.".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub read: Value,
    pub equals: u32,
}

/// Which ROMs this manifest claims.
///
/// Empty matches nothing rather than everything. A manifest that forgot to say
/// what it is for should be inert, not attached to every game someone loads.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Matcher {
    #[serde(default)]
    pub game_code: Vec<String>,
    #[serde(default)]
    pub revision: Vec<u8>,
    #[serde(default)]
    pub game_code_prefix: Vec<String>,
    #[serde(default)]
    pub title: Vec<String>,
}

impl Matcher {
    fn matches(&self, rom: &RomIdentity) -> bool {
        (self
            .game_code
            .iter()
            .any(|code| rom.game_code.eq_ignore_ascii_case(code))
            || self
                .game_code_prefix
                .iter()
                .any(|prefix| rom.code_prefix().eq_ignore_ascii_case(prefix))
            || self
                .title
                .iter()
                .any(|title| rom.title.eq_ignore_ascii_case(title)))
            && (self.revision.is_empty() || self.revision.contains(&rom.revision))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SectionSpec {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(flatten)]
    pub body: SectionBody,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repeat {
    pub count: u32,
    pub stride: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardSpec {
    /// The heading. Usually a text read; a literal works for numbered slots.
    pub title: Value,
    #[serde(default)]
    pub subtitle: Option<Value>,
    /// A picture of whatever this card is about. Optional in the strong sense:
    /// a card that cannot resolve one keeps its title, badges and numbers.
    #[serde(default)]
    pub image: Option<ImageSpec>,
    #[serde(default)]
    pub lead: Option<FieldSpec>,
    #[serde(default)]
    pub fields: Vec<FieldSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
    /// Text in the Generation 3 character set rather than in ASCII.
    ///
    /// Nicknames and trainer names are stored in an alphabet of the game's
    /// own, so `text` on one of them produces punctuation soup. This is the
    /// same read with the right table applied.
    Gen3Text {
        at: Address,
        len: u32,
    },
    /// A fixed number, for the other half of a gauge whose maximum is a rule
    /// rather than a memory location — 31 for an individual value, 252 for an
    /// effort value, 255 for friendship.
    Const(u32),
    /// One named field out of the encrypted part of a Generation 3 record.
    ///
    /// `at` is the start of the record, the same as [`Value::Gen3Species`],
    /// because everything in here needs the whole record decrypted before any
    /// of it can be read. The field is named rather than offset, so nobody
    /// writing a manifest has to know that Attacks might be the third
    /// substructure this time and the first one next time.
    Gen3 {
        at: Address,
        field: Gen3Field,
    },
    /// The species of the Pokémon whose record starts here.
    ///
    /// Reads as the species *name* when the cartridge's name table has been
    /// found, and as `#21` when it has not — a manifest gets the number either
    /// way, so a `max`-less gauge or a card title still works on a ROM hack
    /// that moved its tables. `at` is the start of the 100-byte record, not
    /// the species field: the species is encrypted and permuted, and undoing
    /// that needs the whole record.
    Gen3Species(Address),
}

/// What [`Value::Gen3`] can pull out of a decrypted record.
///
/// Deliberately a closed list of *named* fields rather than an offset into the
/// decrypted block. The whole difficulty of this format is that the block is
/// permuted per Pokémon, so an offset is not a stable way to name anything —
/// and a person building a reader in the browser should never have to learn
/// that in the first place.
///
/// Fields that index a table read as the name and carry the index as their
/// number, so a manifest gets "Leer" on a cartridge whose tables were found
/// and `#43` on one where they were not.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gen3Field {
    Species,
    HeldItem,
    Experience,
    Friendship,
    Move1,
    Move2,
    Move3,
    Move4,
    Pp1,
    Pp2,
    Pp3,
    Pp4,
    EvHp,
    EvAttack,
    EvDefense,
    EvSpeed,
    EvSpAttack,
    EvSpDefense,
    EvTotal,
    IvHp,
    IvAttack,
    IvDefense,
    IvSpeed,
    IvSpAttack,
    IvSpDefense,
    IvTotal,
    Nature,
    AbilitySlot,
    Pokerus,
    MetLocation,
    IsEgg,
    IsShiny,
}

impl Gen3Field {
    /// Whether this field reads as a name out of the cartridge's own tables.
    fn needs_names(self) -> bool {
        matches!(self, Gen3Field::Species | Gen3Field::HeldItem)
            || self.move_slot().is_some()
    }

    fn move_slot(self) -> Option<usize> {
        Some(match self {
            Gen3Field::Move1 => 0,
            Gen3Field::Move2 => 1,
            Gen3Field::Move3 => 2,
            Gen3Field::Move4 => 3,
            _ => return None,
        })
    }

    /// The value, and the words for it when it has any of its own.
    fn read(self, boxed: &crate::gen3::Boxed) -> (u32, Option<String>) {
        use crate::gen3::Stat;
        if let Some(slot) = self.move_slot() {
            let id = boxed.move_id(slot);
            // An empty move slot is a real state — most Pokémon have fewer
            // than four — so it says so rather than reading as "#0".
            let name = if id == 0 {
                "—".to_string()
            } else {
                crate::gen3_names::move_name(id).unwrap_or_else(|| format!("#{id}"))
            };
            return (u32::from(id), Some(name));
        }
        match self {
            Gen3Field::Species => {
                let id = boxed.species();
                (
                    u32::from(id),
                    Some(crate::gen3_names::species(id).unwrap_or_else(|| format!("#{id}"))),
                )
            }
            Gen3Field::HeldItem => {
                let id = boxed.held_item();
                let name = if id == 0 {
                    "—".to_string()
                } else {
                    crate::gen3_names::item(id).unwrap_or_else(|| format!("#{id}"))
                };
                (u32::from(id), Some(name))
            }
            Gen3Field::Experience => (boxed.experience(), None),
            Gen3Field::Friendship => (boxed.friendship(), None),
            Gen3Field::Pp1 => (boxed.pp(0), None),
            Gen3Field::Pp2 => (boxed.pp(1), None),
            Gen3Field::Pp3 => (boxed.pp(2), None),
            Gen3Field::Pp4 => (boxed.pp(3), None),
            Gen3Field::EvHp => (boxed.ev(Stat::Hp), None),
            Gen3Field::EvAttack => (boxed.ev(Stat::Attack), None),
            Gen3Field::EvDefense => (boxed.ev(Stat::Defense), None),
            Gen3Field::EvSpeed => (boxed.ev(Stat::Speed), None),
            Gen3Field::EvSpAttack => (boxed.ev(Stat::SpAttack), None),
            Gen3Field::EvSpDefense => (boxed.ev(Stat::SpDefense), None),
            Gen3Field::EvTotal => (boxed.ev_total(), None),
            Gen3Field::IvHp => (boxed.iv(Stat::Hp), None),
            Gen3Field::IvAttack => (boxed.iv(Stat::Attack), None),
            Gen3Field::IvDefense => (boxed.iv(Stat::Defense), None),
            Gen3Field::IvSpeed => (boxed.iv(Stat::Speed), None),
            Gen3Field::IvSpAttack => (boxed.iv(Stat::SpAttack), None),
            Gen3Field::IvSpDefense => (boxed.iv(Stat::SpDefense), None),
            Gen3Field::IvTotal => (boxed.iv_total(), None),
            Gen3Field::Nature => {
                let index = boxed.nature();
                (index, Some(crate::gen3::NATURES[index as usize].to_string()))
            }
            Gen3Field::AbilitySlot => (boxed.ability_slot(), None),
            Gen3Field::Pokerus => (boxed.pokerus(), None),
            Gen3Field::MetLocation => (boxed.met_location(), None),
            Gen3Field::IsEgg => yes_no(boxed.is_egg()),
            Gen3Field::IsShiny => yes_no(boxed.is_shiny()),
            // Handled above, but the compiler cannot see that.
            Gen3Field::Move1 | Gen3Field::Move2 | Gen3Field::Move3 | Gen3Field::Move4 => (0, None),
        }
    }
}

/// A flag reads as a word, so a panel does not show a bare 1 and leave the
/// reader to guess what it was true about.
fn yes_no(value: bool) -> (u32, Option<String>) {
    (
        u32::from(value),
        Some(if value { "Yes" } else { "No" }.to_string()),
    )
}

/// A picture for a card. One variant today; a tagged enum so adding the next
/// kind of picture does not change what an existing manifest means.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSpec {
    /// The sprite of the species whose record starts here.
    Gen3SpeciesSprite(Address),
}

/// Where to read.
///
/// A bare number is an absolute address. A `deref` chain follows pointers,
/// which most games need — Generation 3 keeps its save blocks behind one, and
/// their addresses move every time the game reloads them.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
            Address::Direct(text) => parse_address(text)
                .and_then(|base| base.checked_add(step))
                .filter(|addr| readable(*addr, 1)),
            Address::Chain { at, deref } => {
                let mut cursor = parse_address(at)?;
                for (index, offset) in deref.iter().enumerate() {
                    if !readable(cursor, 4) {
                        return None;
                    }
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
                        cursor = cursor.checked_add(step)?;
                    }
                }
                readable(cursor, 1).then_some(cursor)
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
    /// Whether a read resolved; numeric zero remains a valid reading.
    live: bool,
}

impl Value {
    fn read(&self, memory: &BoundedMemory<'_>, step: u32, index: u32) -> Read {
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
                let width = match self {
                    Value::U8(_) => 1,
                    Value::U16(_) => 2,
                    _ => 4,
                };
                if !readable(address, width) {
                    return Read::dead();
                }
                let number = match self {
                    Value::U8(_) => u32::from(memory.read_u8(address)),
                    Value::U16(_) => u32::from(memory.read_u16(address)),
                    _ => memory.read_u32(address),
                };
                Read {
                    text: number.to_string(),
                    number: Some(number),
                    live: true,
                }
            }
            Value::Text { at, len } => {
                let Some(address) = at.resolve(memory, step) else {
                    return Read::dead();
                };
                if !readable(address, *len) {
                    return Read::dead();
                }
                let text =
                    crate::memory::read_ascii(memory, address, (*len).min(MAX_TEXT_LEN) as usize);
                let live = !text.trim().is_empty();
                Read {
                    text,
                    number: None,
                    live,
                }
            }
            Value::Gen3Text { at, len } => {
                let Some(address) = at.resolve(memory, step) else {
                    return Read::dead();
                };
                if !readable(address, *len) {
                    return Read::dead();
                }
                let text = crate::gen3::decode_text(
                    &memory.read_bytes(address, (*len).min(MAX_TEXT_LEN) as usize),
                );
                let live = !text.is_empty();
                Read {
                    text,
                    number: None,
                    live,
                }
            }
            Value::Const(number) => Read {
                text: number.to_string(),
                number: Some(*number),
                live: true,
            },
            Value::Gen3 { at, field } => {
                let Some(address) = at.resolve(memory, step) else {
                    return Read::dead();
                };
                if !readable(address, crate::gen3::BOXED_BYTES) {
                    return Read::dead();
                }
                // An empty party slot is the normal state of slots two to six,
                // so it reads as dead and the card for it is simply not drawn.
                let Some(boxed) = memory.boxed(address) else {
                    return Read::dead();
                };
                let (number, name) = field.read(&boxed);
                Read {
                    text: name.unwrap_or_else(|| number.to_string()),
                    number: Some(number),
                    live: true,
                }
            }
            Value::Gen3Species(at) => {
                let Some(address) = at.resolve(memory, step) else {
                    return Read::dead();
                };
                if !readable(address, crate::gen3::BOXED_BYTES) {
                    return Read::dead();
                }
                // An empty party slot is the normal state of slots two to six,
                // so it reads as dead and the card for it is simply not drawn.
                let Some(species) = memory.boxed(address).map(|boxed| boxed.species()) else {
                    return Read::dead();
                };
                Read {
                    // The number is always available; the name only once the
                    // cartridge's table has been found. `#21` is honest about
                    // which of those happened, and still identifies the row.
                    text: crate::gen3_names::species(species)
                        .unwrap_or_else(|| format!("#{species}")),
                    number: Some(u32::from(species)),
                    live: true,
                }
            }
        }
    }
}

impl ImageSpec {
    /// Where a consumer can find this picture, or `None` when there is not one
    /// to name — an empty slot, or a species with no National Dex number.
    fn resolve(&self, memory: &BoundedMemory<'_>, step: u32) -> Option<AddonImage> {
        match self {
            ImageSpec::Gen3SpeciesSprite(at) => {
                let address = at.resolve(memory, step)?;
                if !readable(address, crate::gen3::BOXED_BYTES) {
                    return None;
                }
                let species = memory.boxed(address)?.species();
                let dex = crate::gen3::national_dex_number(species)?;
                let image = AddonImage::new(format!("/sprites/{dex}"));
                Some(match crate::gen3_names::species(species) {
                    Some(name) => image.with_alt(name),
                    None => image,
                })
            }
        }
    }

    fn address(&self) -> &Address {
        match self {
            ImageSpec::Gen3SpeciesSprite(at) => at,
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
        manifest.validate()?;
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
    if !matcher.game_code.is_empty() {
        parts.push(matcher.game_code.join(", "));
    }
    if !matcher.game_code_prefix.is_empty() {
        parts.push(matcher.game_code_prefix.join(", "));
    }
    if !matcher.title.is_empty() {
        parts.push(matcher.title.join(", "));
    }
    if !matcher.revision.is_empty() {
        parts.push(format!("revisions {:?}", matcher.revision));
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

    fn snapshot(&self, memory: &dyn MemoryView, rom: &RomIdentity) -> Option<AddonSnapshot<T>> {
        self.manifest.prepare(memory, rom);
        let sections = self.manifest.sections(memory);

        // A false readiness condition or unresolved pointer leaves the reader idle.
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

fn build_section(spec: &SectionSpec, memory: &BoundedMemory<'_>) -> Option<AddonSection> {
    let id = spec.id.clone();

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
    memory: &BoundedMemory<'_>,
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
    if let Some(image) = &spec.image {
        card = card.with_optional_image(image.resolve(memory, step));
    }
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
    memory: &BoundedMemory<'_>,
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

    /// A Generation 3 party record, built the way the game writes one, so the
    /// manifest is tested against the real encryption rather than a stub.
    fn party_record(personality: u32, ot_id: u32, species: u16, nickname: &[u8]) -> Vec<u8> {
        let mut raw = vec![0u8; 100];
        raw[0..4].copy_from_slice(&personality.to_le_bytes());
        raw[4..8].copy_from_slice(&ot_id.to_le_bytes());
        raw[8..8 + nickname.len()].copy_from_slice(nickname);

        let mut plain = [0u8; 48];
        // Growth is the first substructure in the order personality picks.
        let growth = crate::gen3::growth_offset(personality);
        plain[growth..growth + 2].copy_from_slice(&species.to_le_bytes());
        let checksum = plain
            .chunks_exact(2)
            .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
            .sum::<u32>() as u16;
        raw[28..30].copy_from_slice(&checksum.to_le_bytes());

        let key = personality ^ ot_id;
        for (block, chunk) in plain.chunks_exact(4).enumerate() {
            let word = u32::from_le_bytes(chunk.try_into().unwrap()) ^ key;
            raw[32 + block * 4..36 + block * 4].copy_from_slice(&word.to_le_bytes());
        }
        // Level and HP live in the unencrypted tail, which is the part a
        // manifest could always read.
        raw[84] = 10;
        raw[86..88].copy_from_slice(&53u16.to_le_bytes());
        raw[88..90].copy_from_slice(&53u16.to_le_bytes());
        raw
    }

    /// The encrypted half of a record, which is the half worth having: moves,
    /// effort and individual values, nature, and the flags. None of it can be
    /// reached by an offset, because the block is permuted per Pokémon.
    #[test]
    fn a_card_reads_the_encrypted_half_of_a_record_by_name() {
        const BASE: u32 = 0x0202_4284;
        // personality 7 puts the substructures in one order; 19 in another.
        // Both must answer the same questions with the same numbers.
        for personality in [7u32, 19, 23] {
            let memory = SparseMemory::new().with(
                BASE,
                loaded_record(personality, 0x1234_5678, 21, &[0xCD, 0xFF]),
            );
            let reader = addon(&format!(
                r#"{{
                  "manifest_version": 2,
                  "addon_id": "custom.detail",
                  "display_name": "Detail",
                  "matches": {{ "game_code": ["BPRE"], "revision": [0] }},
                  "sections": [{{
                    "id": "party", "title": "Party", "kind": "cards",
                    "repeat": {{ "count": 1, "stride": 100 }},
                    "card": {{
                      "title": {{ "gen3": {{ "at": "0x02024284", "field": "species" }} }},
                      "fields": [
                        {{ "label": "Move 1", "read": {{ "gen3": {{ "at": "0x02024284", "field": "move1" }} }} }},
                        {{ "label": "Move 2", "read": {{ "gen3": {{ "at": "0x02024284", "field": "move2" }} }} }},
                        {{ "label": "PP 1", "read": {{ "gen3": {{ "at": "0x02024284", "field": "pp1" }} }} }},
                        {{ "label": "HP EV", "read": {{ "gen3": {{ "at": "0x02024284", "field": "ev_hp" }} }},
                          "max": {{ "const": 252 }} }},
                        {{ "label": "EV total", "read": {{ "gen3": {{ "at": "0x02024284", "field": "ev_total" }} }},
                          "max": {{ "const": 510 }} }},
                        {{ "label": "Atk IV", "read": {{ "gen3": {{ "at": "0x02024284", "field": "iv_attack" }} }},
                          "max": {{ "const": 31 }} }},
                        {{ "label": "Nature", "read": {{ "gen3": {{ "at": "0x02024284", "field": "nature" }} }} }},
                        {{ "label": "Friendship", "read": {{ "gen3": {{ "at": "0x02024284", "field": "friendship" }} }} }},
                        {{ "label": "Egg", "read": {{ "gen3": {{ "at": "0x02024284", "field": "is_egg" }} }} }}
                      ]
                    }}
                  }}]
                }}"#
            ));

            let snapshot = read_of(&reader, &memory).expect("detail should report");
            let AddonSectionContent::Cards(cards) = &snapshot.sections[0].content else {
                panic!("expected cards");
            };
            let field = |label: &str| {
                cards[0]
                    .fields
                    .iter()
                    .find(|field| field.label == label)
                    .unwrap_or_else(|| panic!("{label} missing"))
            };

            assert_eq!(field("Move 1").value, "#43", "personality {personality}");
            // An empty move slot is a state, not a zero to be shown as "#0".
            assert_eq!(field("Move 2").value, "—");
            assert_eq!(field("PP 1").value, "35");
            assert_eq!(field("HP EV").value, "4/252");
            assert_eq!(field("EV total").value, "10/510");
            assert_eq!(field("Atk IV").value, "6/31");
            // Nature is not stored; it is the personality, mod 25.
            assert_eq!(
                field("Nature").value,
                crate::gen3::NATURES[(personality % 25) as usize]
            );
            assert_eq!(field("Friendship").value, "70");
            assert_eq!(field("Egg").value, "No");

            // A constant maximum gives a bar to a value whose ceiling is a
            // rule rather than a memory location.
            assert_eq!(field("HP EV").meter, Some(AddonMeter::new(4, 252)));
            assert_eq!(field("Atk IV").meter, Some(AddonMeter::new(6, 31)));
        }
    }

    /// Sixteen decrypted fields on one card must cost one decrypt, not sixteen.
    #[test]
    fn every_field_of_a_card_shares_one_decrypt() {
        const BASE: u32 = 0x0202_4284;
        let memory = SparseMemory::new()
            .with(BASE, loaded_record(7, 0x1234_5678, 21, &[0xCD, 0xFF]))
            .with(BASE + 100, loaded_record(19, 0x1234_5678, 25, &[0xFF]));

        let fields: String = (1..=4)
            .map(|n| {
                format!(
                    r#"{{ "label": "Move {n}", "read": {{ "gen3": {{ "at": "0x02024284", "field": "move{n}" }} }} }},
                       {{ "label": "PP {n}", "read": {{ "gen3": {{ "at": "0x02024284", "field": "pp{n}" }} }} }}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let reader = addon(&format!(
            r#"{{
              "manifest_version": 2, "addon_id": "custom.many", "display_name": "Many",
              "matches": {{ "game_code": ["BPRE"], "revision": [0] }},
              "sections": [{{
                "id": "party", "title": "Party", "kind": "cards",
                "repeat": {{ "count": 6, "stride": 100 }},
                "card": {{ "title": {{ "gen3": {{ "at": "0x02024284", "field": "species" }} }},
                           "fields": [{fields}] }}
              }}]
            }}"#
        ));

        let counted = CountingMemory {
            inner: &memory,
            reads: std::cell::Cell::new(0),
        };
        let snapshot = read_of(&reader, &counted).expect("should report");
        let AddonSectionContent::Cards(cards) = &snapshot.sections[0].content else {
            panic!("expected cards");
        };
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].fields.len(), 8);

        // One decrypt is ~58 byte reads. Nine fields per card over six slots
        // without the memo would be several thousand; with it, well under.
        assert!(
            counted.reads.get() < 700,
            "expected one decrypt per record, saw {} byte reads",
            counted.reads.get()
        );
    }

    /// A record carrying the things the encrypted block is for, so the tests
    /// above are checking a decode rather than a pile of zeroes.
    fn loaded_record(personality: u32, ot_id: u32, species: u16, nickname: &[u8]) -> Vec<u8> {
        let mut raw = party_record(personality, ot_id, species, nickname);

        let mut plain = [0u8; 48];
        let growth = crate::gen3::growth_offset(personality);
        plain[growth..growth + 2].copy_from_slice(&species.to_le_bytes());
        plain[growth + 9] = 70; // friendship

        // The substructure order names where Attacks, EVs and Misc landed.
        let order = substruct_order(personality);
        let attacks = order[1] * 12;
        plain[attacks..attacks + 2].copy_from_slice(&43u16.to_le_bytes()); // move 1
        plain[attacks + 8] = 35; // pp 1

        let evs = order[2] * 12;
        plain[evs] = 4; // HP EV
        plain[evs + 1] = 6; // Attack EV, so the total is not just one number

        let misc = order[3] * 12;
        // IVs: HP 3, Attack 6, the rest zero. Five bits each, from the bottom.
        let iv_word: u32 = 3 | (6 << 5);
        plain[misc + 4..misc + 8].copy_from_slice(&iv_word.to_le_bytes());

        let checksum = plain
            .chunks_exact(2)
            .map(|pair| u32::from(u16::from_le_bytes([pair[0], pair[1]])))
            .sum::<u32>() as u16;
        raw[28..30].copy_from_slice(&checksum.to_le_bytes());

        let key = personality ^ ot_id;
        for (block, chunk) in plain.chunks_exact(4).enumerate() {
            let word = u32::from_le_bytes(chunk.try_into().unwrap()) ^ key;
            raw[32 + block * 4..36 + block * 4].copy_from_slice(&word.to_le_bytes());
        }
        raw
    }

    /// The permutation, rebuilt from the one offset `gen3` exposes, so the
    /// tests do not carry a second copy of the twenty-four orders.
    fn substruct_order(personality: u32) -> [usize; 4] {
        const ORDERS: [[usize; 4]; 24] = [
            [0, 1, 2, 3], [0, 1, 3, 2], [0, 2, 1, 3], [0, 3, 1, 2], [0, 2, 3, 1], [0, 3, 2, 1],
            [1, 0, 2, 3], [1, 0, 3, 2], [2, 0, 1, 3], [3, 0, 1, 2], [2, 0, 3, 1], [3, 0, 2, 1],
            [1, 2, 0, 3], [1, 3, 0, 2], [2, 1, 0, 3], [3, 1, 0, 2], [2, 3, 0, 1], [3, 2, 0, 1],
            [1, 2, 3, 0], [1, 3, 2, 0], [2, 1, 3, 0], [3, 1, 2, 0], [2, 3, 1, 0], [3, 2, 1, 0],
        ];
        ORDERS[(personality % 24) as usize]
    }

    /// Counts byte reads, so "decrypt once per record" is a measurement rather
    /// than a claim.
    struct CountingMemory<'a> {
        inner: &'a dyn MemoryView,
        reads: std::cell::Cell<usize>,
    }
    impl MemoryView for CountingMemory<'_> {
        fn read_u8(&self, addr: u32) -> u8 {
            // Only the party block. The other reads are the one-off pass over
            // the cartridge that finds the name tables, which happens once per
            // ROM and is not what this test is measuring.
            if (0x0202_4284..0x0202_44AC).contains(&addr) {
                self.reads.set(self.reads.get() + 1);
            }
            self.inner.read_u8(addr)
        }
    }

    /// The whole point of the Generation 3 reads: a party card that says
    /// *whose* HP it is showing, which no combination of `u8`, `u16` and
    /// `text` could express.
    #[test]
    fn a_party_card_names_the_pokemon_whose_stats_it_shows() {
        const BASE: u32 = 0x0202_4284;
        let mut memory = SparseMemory::new();
        // Two live slots and a third that was never filled.
        memory = memory.with(
            BASE,
            party_record(7, 0x1234_5678, 21, &[0xCD, 0xCA, 0xBF, 0xBB, 0xCC, 0xC9, 0xD1, 0xFF]),
        );
        memory = memory.with(BASE + 100, party_record(19, 0x1234_5678, 25, &[0xFF]));
        memory = memory.with(BASE + 200, vec![0u8; 100]);

        let reader = addon(
            r#"{
              "manifest_version": 2,
              "addon_id": "custom.party",
              "display_name": "Party",
              "matches": { "game_code": ["BPRE"], "revision": [0] },
              "sections": [{
                "id": "party",
                "title": "Party",
                "kind": "cards",
                "repeat": { "count": 6, "stride": 100 },
                "card": {
                  "title": { "gen3_species": "0x02024284" },
                  "subtitle": { "gen3_text": { "at": "0x0202428C", "len": 10 } },
                  "image": { "gen3_species_sprite": "0x02024284" },
                  "lead": {
                    "label": "HP",
                    "read": { "u16": "0x020242DA" },
                    "max": { "u16": "0x020242DC" }
                  },
                  "fields": [{ "label": "Level", "read": { "u8": "0x020242D8" } }]
                }
              }]
            }"#,
        );

        let snapshot = read_of(&reader, &memory).expect("party should report");
        let AddonSectionContent::Cards(cards) = &snapshot.sections[0].content else {
            panic!("expected cards");
        };

        // The empty slot is absent rather than drawn as a blank card.
        assert_eq!(cards.len(), 2);
        // Without a cartridge name table the species is still identified.
        assert_eq!(cards[0].title, "#21");
        assert_eq!(cards[0].subtitle.as_deref(), Some("SPEAROW"));
        assert_eq!(
            cards[0].image.as_ref().map(|image| image.src.as_str()),
            Some("/sprites/21")
        );
        // The gauge on the unencrypted tail, beside the name from the
        // encrypted part: the pairing is the feature.
        assert_eq!(cards[0].lead.as_ref().unwrap().value, "53/53");
        assert!(cards[0].lead.as_ref().unwrap().meter.is_some());
        assert_eq!(cards[0].fields[0].value, "10");
        // A record with no nickname keeps its species and drops the subtitle.
        assert_eq!(cards[1].title, "#25");
        assert_eq!(cards[1].subtitle, None);
    }

    #[test]
    fn generation_three_reads_are_bounded_like_every_other_read() {
        let species = |at: &str| {
            format!(
                r#"{{"addon_id":"g","display_name":"g","matches":{{"game_code":["BPRE"]}},
                  "sections":[{{"id":"s","title":"S","kind":"key_value",
                  "fields":[{{"label":"Species","read":{{"gen3_species":"{at}"}}}}]}}]}}"#
            )
        };
        // A record needs 80 readable bytes, so one that would run off the end
        // of EWRAM is refused at validation rather than read short.
        assert!(Manifest::parse(&species("0x0203FFF0")).is_err());
        assert!(Manifest::parse(&species("0x04000000")).is_err());
        assert!(Manifest::parse(&species("0x02024284")).is_ok());

        // Only a manifest that asks for a name pays for finding the tables.
        let plain = Manifest::parse(WALLET).unwrap();
        assert!(!plain.needs_cartridge_names());
        assert!(Manifest::parse(&species("0x02024284"))
            .unwrap()
            .needs_cartridge_names());
    }

    #[test]
    fn strict_validation_and_pointer_bounds_protect_the_host() {
        let mut value: serde_json::Value = serde_json::from_str(WALLET).unwrap();
        value["matches"] = serde_json::json!({"game_code":["BPRE"],"revision":[0]});
        let manifest = Manifest::parse(&value.to_string()).unwrap();
        assert!(manifest.supports(&rom("BPRE")));
        assert!(!manifest.supports(&rom("BPRJ")));
        let mut revision = rom("BPRE"); revision.revision = 1;
        assert!(!manifest.supports(&revision));
        value["sections"][0]["fields"][0]["read"] = serde_json::json!({"u32":{"at":"0x03000000","deref":[0]}});
        value["sections"][0]["fields"].as_array_mut().unwrap().truncate(1);
        let manifest = Manifest::parse(&value.to_string()).unwrap();
        let memory = SparseMemory::new().with(0x03000000, 0x04000000u32.to_le_bytes().to_vec());
        assert!(manifest.sections(&memory).is_empty(), "an indirect pointer cannot expose IO registers");
        value["sections"][0]["fields"][0]["read"] = serde_json::json!({"u8":"0x03007fff"});
        assert!(Manifest::parse(&value.to_string()).is_ok(), "the last RAM byte is valid for u8");
        value["sections"][0]["fields"][0]["read"] = serde_json::json!({"u32":"0x03007fff"});
        assert!(Manifest::parse(&value.to_string()).is_err(), "wide reads must stay in bounds");
        value["sections"][0]["fields"][0]["read"] = serde_json::json!({"u8":"0x02000000"});
        value["matches"]["revisions"] = serde_json::json!([0]);
        assert!(Manifest::parse(&value.to_string()).is_err(), "misspelled compatibility rules must not be ignored");
    }

    #[test]
    fn full_width_meters_do_not_overflow() {
        assert_eq!(AddonMeter::new(u32::MAX, u32::MAX).percent(), 100);
        assert_eq!(AddonTone::from_fraction(u32::MAX, u32::MAX), AddonTone::Good);
    }

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
    fn a_manifest_with_no_matcher_is_rejected() {
        let result = Manifest::parse(
            r#"{
              "addon_id": "custom.empty",
              "display_name": "Empty",
              "sections": [{ "id": "s", "title": "S", "kind": "key_value",
                "fields": [{ "label": "X", "read": { "u8": "0x02000000" } }] }]
            }"#,
        );
        assert!(result.is_err());
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
    fn zero_is_valid_numeric_data() {
        let snapshot = read_of(&addon(WALLET), &SparseMemory::new()).expect("zero is valid");
        match &snapshot.sections[0].content {
            AddonSectionContent::KeyValue(fields) => assert_eq!(fields[0].value, "0"),
            _ => panic!("expected fields"),
        }
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
        assert!(err.contains("sections"), "{err}");

        assert!(ManifestAddon::parse("not json").is_err());
    }

    /// A manifest is hand-edited data and a stride of one with a count of a
    /// million is a plausible typo. It has to be a bounded mistake.
    #[test]
    fn an_absurd_repeat_is_rejected() {
        let json = r#"{
          "addon_id": "custom.huge", "display_name": "Huge",
          "matches": { "game_code_prefix": ["BPR"] },
          "sections": [{ "id": "s", "title": "S", "kind": "cards",
            "repeat": { "count": 4000000, "stride": 1 },
            "card": { "title": { "literal": "row" } } }]
        }"#;

        assert!(Manifest::parse(json).is_err());
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
