use super::InsightAnalysis;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StoryStatus {
    Confirmed,
    Reported,
    Unconfirmed,
    Response,
    Feedback,
    Denied,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Story {
    pub title: String,
    pub title_en: String,
    pub summary: String,
    pub summary_en: String,
    /// Why a hotspot matters; gossip has none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub angle: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub angle_en: String,
    pub status: StoryStatus,
    pub source_indexes: Vec<usize>,
}

/// A story's schema properties; hotspots narrow the status and add the angle.
pub(super) fn story_properties(source_indexes: Value) -> Value {
    let text = json!({ "type": "string" });
    json!({
        "title": text, "title_en": text, "summary": text, "summary_en": text,
        "status": { "type": "string", "enum": ["confirmed", "reported", "unconfirmed", "response", "feedback", "denied"] },
        "source_indexes": source_indexes
    })
}

/// The sources a briefing cites, in reading order.
pub(super) fn citation_indexes(analysis: &InsightAnalysis) -> Vec<usize> {
    let mut indexes = analysis.changes_source_indexes.clone();
    for story in &analysis.hotspots {
        indexes.extend(&story.source_indexes);
    }
    for figure in &analysis.figures {
        indexes.extend(&figure.source_indexes);
    }
    for story in &analysis.gossip {
        indexes.extend(&story.source_indexes);
    }
    indexes.extend(analysis.cover_source_index);
    indexes.extend(&analysis.picture_source_indexes);
    for signal in analysis.outlook.iter().chain(&analysis.watch_signals) {
        indexes.extend(&signal.source_indexes);
    }
    let mut seen = HashSet::new();
    indexes.retain(|index| seen.insert(*index));
    indexes
}
