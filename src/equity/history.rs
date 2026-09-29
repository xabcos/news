use super::*;
use uuid::Uuid;

const NO_PROGRESS: &str = "暂无可验证进展；现有资料不足以确认或推翻上期判断。";
const NO_PROGRESS_EN: &str =
    "No verifiable progress; the available material cannot confirm or refute the previous thesis.";

/// A fixed, bounded snapshot: do not nest feedback from earlier editions.
#[derive(Serialize, Deserialize)]
pub(super) struct Baseline {
    pub id: Uuid,
    pub day: NaiveDate,
    pub cutoff: DateTime<Utc>,
    pub stocks: Vec<Value>,
    pub source_urls: Vec<String>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct Review {
    pub previous_index: usize,
    pub status: String,
    pub assessment: String,
    pub assessment_en: String,
    pub events: Vec<Event>,
}

pub(super) async fn baseline(state: &AppState, cutoff: DateTime<Utc>) -> Result<Option<Baseline>> {
    // Regenerating an edition keeps its first published comparison, including no baseline.
    let fixed: Option<Value> = sqlx::query_scalar(
        "SELECT payload->'baseline' FROM ai_insights
         WHERE kind='equity' AND status='ok' AND window_end=$1 AND payload ? 'baseline'
         ORDER BY finished_at,id LIMIT 1",
    )
    .bind(cutoff)
    .fetch_optional(&state.pool)
    .await?;
    if let Some(fixed) = fixed {
        return Ok(serde_json::from_value(fixed)?);
    }
    let previous: Option<(Uuid, NaiveDate, DateTime<Utc>, Value)> = sqlx::query_as(
        "SELECT id,briefing_day,window_end,payload FROM ai_insights
         WHERE kind='equity' AND status='ok' AND window_end < $1
         ORDER BY window_end DESC,finished_at DESC,id DESC LIMIT 1",
    )
    .bind(cutoff)
    .fetch_optional(&state.pool)
    .await?;
    previous
        .map(|(id, day, cutoff, payload)| {
            let mut stocks: Vec<Value> = serde_json::from_value(payload["note"]["stocks"].clone())?;
            for stock in &mut stocks {
                if let Some(fields) = stock.as_object_mut() {
                    fields.remove("feedback");
                }
            }
            let source_urls = payload["sources"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|source| source["url"].as_str().map(str::to_owned))
                .collect();
            Ok(Baseline {
                id,
                day,
                cutoff,
                stocks,
                source_urls,
            })
        })
        .transpose()
}

pub(super) fn is_new(source: &Source, baseline: &Baseline, end: DateTime<Utc>) -> bool {
    source.published_at >= baseline.cutoff
        && source.published_at < end
        && !baseline.source_urls.contains(&source.url)
}

pub(super) fn no_progress(baseline: Option<&Baseline>) -> Vec<Review> {
    (0..baseline.map_or(0, |b| b.stocks.len()))
        .map(|previous_index| Review {
            previous_index,
            status: "insufficient".into(),
            assessment: NO_PROGRESS.into(),
            assessment_en: NO_PROGRESS_EN.into(),
            events: vec![],
        })
        .collect()
}

pub(super) fn validate(
    reviews: &[Review],
    baseline: Option<&Baseline>,
    sources: &[Source],
    end: DateTime<Utc>,
) -> Result<()> {
    let invalid =
        || AppError::Internal("历史反馈必须完整对应上期股票，并引用上期截止后的新证据".into());
    let count = baseline.map_or(0, |b| b.stocks.len());
    if reviews.len() != count {
        return Err(invalid());
    }
    let mut seen = HashSet::new();
    for review in reviews {
        let baseline = baseline.ok_or_else(invalid)?;
        let previous = baseline
            .stocks
            .get(review.previous_index)
            .ok_or_else(invalid)?;
        if !seen.insert(review.previous_index)
            || !matches!(
                review.status.as_str(),
                "supported" | "partial" | "contradicted" | "pending" | "insufficient"
            )
            || review.assessment.trim().is_empty()
            || review.assessment_en.trim().is_empty()
            || review.events.len() > 3
        {
            return Err(invalid());
        }
        if review.events.is_empty() {
            if !matches!(review.status.as_str(), "pending" | "insufficient") {
                return Err(invalid());
            }
            continue;
        }
        validate_events(
            &review.events,
            sources,
            previous["symbol"].as_str().ok_or_else(invalid)?,
        )?;
        for index in review.events.iter().flat_map(|event| &event.source_indexes) {
            if !is_new(&sources[*index], baseline, end) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}

pub(super) fn attach(
    stocks: &mut [Value],
    reviews: &[Review],
    baseline: Option<&Baseline>,
    sources: &[Source],
    timezone: chrono_tz::Tz,
) -> Vec<Value> {
    let mut followups = vec![];
    let Some(baseline) = baseline else {
        return followups;
    };
    // Follow the prior edition's order, regardless of the model's review order.
    for (index, previous) in baseline.stocks.iter().enumerate() {
        let review = reviews
            .iter()
            .find(|review| review.previous_index == index)
            .expect("validated review");
        let (assessment, assessment_en) = if review.events.is_empty() {
            (NO_PROGRESS, NO_PROGRESS_EN)
        } else {
            (review.assessment.as_str(), review.assessment_en.as_str())
        };
        let feedback = json!({
            "previous_id":baseline.id,"previous_day":baseline.day,"previous":previous,
            "previous_edition":edition(baseline.cutoff),"previous_cutoff":baseline.cutoff,
            "status":review.status,"assessment":assessment,"assessment_en":assessment_en,
            "events":public_events(&review.events,sources,timezone),
        });
        if let Some(stock) = stocks.iter_mut().find(|stock| {
            same_security(
                stock["market"].as_str().expect("validated market"),
                stock["symbol"].as_str().expect("validated symbol"),
                previous,
            )
        }) {
            stock["change"] = json!(if stock["signal"] == previous["signal"] {
                "continuing"
            } else {
                "changed"
            });
            stock["previous_signal"] = previous["signal"].clone();
            stock["feedback"] = feedback;
        } else {
            followups.push(json!({
                "name":previous["name"],"name_en":previous["name_en"],
                "market":previous["market"],"symbol":previous["symbol"],"feedback":feedback,
            }));
        }
    }
    followups
}
