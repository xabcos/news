//! AI-authored identities are stored once in the draft, then frozen at adoption.
use crate::{
    error::Result,
    state::AppState,
    travel_creative::{
        CREATIVE_BUDGET_SECONDS, choose_candidate, near_duplicate, request_json, text_schema,
        unavailable, valid_text,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
use uuid::Uuid;

pub(crate) const IDENTITY_VERSION: &str = "travel-girl-ai-v4";

#[derive(Debug, Serialize)]
pub(crate) struct IdentityPreview {
    pub(crate) identity_version: String,
    pub(crate) identity: Value,
    pub(crate) initial_accessories: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Persona {
    name: String,
    age: u8,
    face: Face,
    hair: String,
    hair_color: String,
    skin_tone: String,
    silhouette: String,
    style: String,
    personality: String,
    appearance_summary: String,
    accessory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Face {
    shape: String,
    eyes: String,
    brows: String,
    nose: String,
    lips: String,
    chin: String,
    detail: String,
}

impl Persona {
    fn valid(&self) -> bool {
        (22..=24).contains(&self.age)
            && valid_text(&self.name, 12)
            && valid_text(&self.appearance_summary, 120)
            && [
                &self.hair,
                &self.hair_color,
                &self.skin_tone,
                &self.silhouette,
                &self.style,
                &self.personality,
                &self.accessory,
                &self.face.shape,
                &self.face.eyes,
                &self.face.brows,
                &self.face.nose,
                &self.face.lips,
                &self.face.chin,
                &self.face.detail,
            ]
            .iter()
            .all(|value| valid_text(value, 300))
    }

    fn preview(&self, seed: &[u8]) -> IdentityPreview {
        let mut identity = serde_json::to_value(self).expect("persona is serialisable");
        identity.as_object_mut().unwrap().remove("name");
        identity.as_object_mut().unwrap().remove("accessory");
        identity["kind"] = json!("travel_girl");
        identity["heritage"] = json!("East Asian");
        identity["name_seed"] = json!(self.name);
        identity["appearance_key"] = json!(appearance_key(&identity));
        identity["signature"] = json!(hex::encode(&Sha256::digest(seed)[..6]));
        identity["identity_seed_version"] = json!(IDENTITY_VERSION);
        IdentityPreview {
            identity_version: IDENTITY_VERSION.into(),
            identity,
            initial_accessories: json!([{"id": "personal_accessory", "name": self.accessory, "slot": "optional", "fixed": false}]),
        }
    }
}

fn appearance_key(identity: &Value) -> String {
    let mut appearance = BTreeMap::new();
    for key in ["face", "hair", "hair_color", "skin_tone", "silhouette"] {
        appearance.insert(key, identity[key].clone());
    }
    let canonical = serde_json::to_string(&appearance).expect("appearance is serialisable");
    let normalized = canonical
        .to_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    hex::encode(Sha256::digest(normalized))
}

fn persona_schema() -> Value {
    let mut facial = serde_json::Map::new();
    for field in ["shape", "eyes", "brows", "nose", "lips", "chin", "detail"] {
        facial.insert(field.into(), text_schema(300));
    }
    let mut properties = serde_json::Map::new();
    for field in [
        "hair",
        "hair_color",
        "skin_tone",
        "silhouette",
        "style",
        "personality",
        "accessory",
    ] {
        properties.insert(field.into(), text_schema(300));
    }
    properties.insert("name".into(), text_schema(12));
    properties.insert(
        "age".into(),
        json!({"type": "integer", "minimum": 22, "maximum": 24}),
    );
    properties.insert("appearance_summary".into(), text_schema(120));
    properties.insert("face".into(), json!({"type": "object", "additionalProperties": false, "required": facial.keys().collect::<Vec<_>>(), "properties": facial}));
    json!({"type": "object", "additionalProperties": false, "required": properties.keys().collect::<Vec<_>>(), "properties": properties})
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersonaCandidates {
    candidates: Vec<Persona>,
}

pub(crate) async fn generate_identity(state: &AppState, seed: &[u8]) -> Result<IdentityPreview> {
    tokio::time::timeout(Duration::from_secs(CREATIVE_BUDGET_SECONDS), async {
        // A bounded recent sample gives the author visual vocabulary to avoid.
        // The database additionally rejects exact complete appearance duplicates.
        let recent = sqlx::query_scalar::<_, Value>(
            "SELECT jsonb_build_object('face', identity->'face', 'hair', identity->'hair', 'silhouette', identity->'silhouette') FROM travel_girls ORDER BY adopted_at DESC LIMIT 24",
        ).fetch_all(&state.pool).await?;
        let mut rejected = Vec::<Persona>::new();
        for _ in 0..2 {
            let batch: PersonaCandidates = request_json(state, "travel_identity_candidates", json!({
                "type": "object", "additionalProperties": false, "required": ["candidates"],
                "properties": {"candidates": {"type": "array", "minItems": 3, "maxItems": 3, "items": persona_schema()}},
            }), r#"Create THREE independently imagined, original fictional adult East Asian women for individual travel companions. Invent each complete character freely; do not use a named archetype roster, a preset face list or a celebrity likeness. The three must differ substantially in facial geometry and relationships between features, not just clothes or hair. Read recent identities and avoid repeating their face designs. The nonce supplies fresh creative entropy; it does not encode a fixed character.
Shared art direction: clearly adult age 22–24, beautiful fresh early-twenties East Asian face, expressive eyes, light dewy natural makeup, natural pink lips, lively youthful charm. Attractive healthy feminine curves, a naturally defined waist and balanced bust, hip and leg proportions; each physique has its own coherent character. Avoid mature office styling, severe sculpted features, heavy makeup, plastic perfection, minor-coded styling, school uniforms and explicit sexual descriptions.
Design coherent specific face shape, eye spacing and shape, brows, nose, lips, chin and one subtle identifying detail; invent harmonious hair, color, complexion and physique. Create her own personality with personal tastes, curiosity and small idiosyncrasies that can inspire surprising future moments. Style describes a flexible preference, never a mandatory uniform or a fixed outfit. An accessory is a personal possession she can wear or leave at home; invent it freely.
Use Chinese for name, appearance_summary and accessory. appearance_summary is a short friendly description for the owner. Write all other descriptive fields in concise natural English for the image model. Respect all schema limits; return JSON only."#,
                json!({"random_nonce": Uuid::new_v4(), "recent_identities_to_differ_from": recent, "rejected_drafts": rejected})).await?;
            if batch.candidates.len() != 3 || batch.candidates.iter().any(|persona| !persona.valid()) { return Err(unavailable()); }
            let candidates = serde_json::to_value(&batch.candidates)?;
            if let Ok(index) = choose_candidate(state, "new independent adult identity", &candidates, &json!({"recent_identities": recent})).await {
                let selected = &batch.candidates[index];
                let face = serde_json::to_string(&selected.face)?;
                let repeated = recent.iter().any(|previous| near_duplicate(&face, &previous["face"].to_string()));
                let preview = selected.preview(seed);
                let exists = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM travel_girls WHERE identity ? 'appearance_key' AND identity->>'appearance_key' = $1)",
                ).bind(preview.identity["appearance_key"].as_str()).fetch_one(&state.pool).await?;
                if !repeated && !exists { return Ok(preview); }
            }
            rejected.extend(batch.candidates);
        }
        Err(unavailable())
    }).await.map_err(|_| unavailable())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_dedup_ignores_names_and_random_signatures() {
        let mut one = json!({"face":{"shape":"soft oval"},"hair":"dark waves","hair_color":"brown","skin_tone":"warm ivory","silhouette":"balanced curves","name_seed":"one","signature":"123"});
        let key = appearance_key(&one);
        one["name_seed"] = json!("another");
        one["signature"] = json!("456");
        assert_eq!(key, appearance_key(&one));
        one["face"]["shape"] = json!("a softly rounded face");
        assert_ne!(key, appearance_key(&one));
    }
}
