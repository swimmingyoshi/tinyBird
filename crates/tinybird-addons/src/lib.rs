//! Shared addon contracts for tinyBird hosts, overlays, and external tools.
//!
//! Game-specific code can keep its own typed internals, but exported snapshots
//! should use these stable envelope types so web apps and future addon loaders
//! can consume different games through the same contract.

use serde::Serialize;

pub const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

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
pub struct RomIdentity {
    pub title: String,
    pub game_code: String,
    pub maker_code: String,
    pub revision: u8,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonSection {
    pub section_id: &'static str,
    pub title: String,
    #[serde(flatten)]
    pub content: AddonSectionContent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "payload", rename_all = "snake_case")]
pub enum AddonSectionContent {
    KeyValue(Vec<AddonField>),
    List(Vec<String>),
    Table(AddonTable),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonField {
    pub label: String,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AddonTable {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
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
        let section = AddonSection {
            section_id: "summary",
            title: "Summary".to_string(),
            content: AddonSectionContent::KeyValue(vec![AddonField {
                label: "Mode".to_string(),
                value: "Live".to_string(),
            }]),
        };
        let json = serde_json::to_value(section).expect("serialize section");

        assert_eq!(json["section_id"], "summary");
        assert_eq!(json["kind"], "key_value");
        assert_eq!(json["payload"][0]["label"], "Mode");
        assert_eq!(json["payload"][0]["value"], "Live");
    }
}
