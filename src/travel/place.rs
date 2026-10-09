//! Places from Wikidata. The model only names a place: the names are looked up, and an item
//! with coordinates is kept only when the post itself names it, either by one of the item's
//! names in any language or by the name that found it. The destination is the nearest place
//! up the administrative chain with an English Wikivoyage guide, so a shrine resolves to its
//! city and a park to its borough; the country, which always has one, is the last resort. A big
//! city's district, which Wikivoyage writes as "Tokyo/Asakusa", resolves to the city.

use crate::{
    error::{AppError, Result},
    state::AppState,
};
use serde::{Deserialize, de::IgnoredAny};
use serde_json::Value;
use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, PoisonError},
};
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use url::Url;

const API: &str = "https://www.wikidata.org/w/api.php";
const SEARCH_LIMIT: &str = "7";
/// Steps up the administrative chain before the country is taken.
const MAX_DEPTH: usize = 6;
/// Matching items tried for coordinates, best first.
const MAX_TRIES: usize = 3;
/// A Latin name may begin a longer word, as in "#kyototrip", once it is this long.
const MIN_PREFIX: usize = 5;
const CHINESE: [&str; 6] = ["zh-cn", "zh-hans", "zh-sg", "zh", "zh-hant", "zh-tw"];
const ENGLISH: [&str; 3] = ["en", "en-gb", "mul"];
/// Taiwan is shown as part of China: its destinations are listed under the People's Republic
/// of China, and Taiwan itself is called 中国台湾.
const TAIWAN: &str = "Q865";
const CHINA: &str = "Q148";
/// What a lookup reads of an item: candidates are matched by their names, places walked by
/// their statements, countries need only names and guides, and `NAMES` are display names.
const CANDIDATE: &str = "labels|aliases|sitelinks";
const PLACE: &str = "labels|sitelinks|claims";
const COUNTRY: &str = "labels|sitelinks";
const NAMES: &str = "names";

/// One run's Wikidata answers. Moments at the same place ask the same questions, so answers
/// are kept for the run; failures are not kept, and no lock is held across a request.
#[derive(Default)]
pub(super) struct Wikidata {
    searches: Mutex<HashMap<(String, String), Vec<String>>>,
    entities: Mutex<HashMap<(&'static str, String), Arc<Entity>>>,
    /// Items by the title of their English Wikivoyage guide.
    articles: Mutex<HashMap<String, Option<Arc<Entity>>>>,
}

/// A Wikidata item with its Chinese and English names.
#[derive(Debug, Clone)]
pub(crate) struct Named {
    pub id: String,
    pub name: String,
    pub name_en: String,
}

/// A destination as `travel_places` keeps it.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct Destination {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub name_en: String,
    pub country_id: String,
    pub country: String,
    pub country_en: String,
    pub latitude: f64,
    pub longitude: f64,
    pub voyage: String,
    pub wiki_zh: Option<String>,
    pub wiki_en: Option<String>,
}

pub(crate) struct Located {
    pub spot: Named,
    pub destination: Destination,
}

#[derive(Debug, Deserialize)]
struct Response {
    #[serde(default)]
    entities: HashMap<String, Entity>,
    #[serde(default)]
    search: Vec<Hit>,
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct Hit {
    id: String,
}

#[derive(Debug, Deserialize)]
struct Entity {
    /// Absent for missing items.
    #[serde(default)]
    id: String,
    #[serde(default)]
    labels: HashMap<String, Text>,
    #[serde(default)]
    aliases: HashMap<String, Vec<Text>>,
    #[serde(default)]
    sitelinks: HashMap<String, Sitelink>,
    #[serde(default)]
    claims: HashMap<String, Vec<Statement>>,
}

#[derive(Debug, Deserialize)]
struct Text {
    value: String,
}

#[derive(Debug, Deserialize)]
struct Sitelink {
    title: String,
}

#[derive(Debug, Deserialize)]
struct Statement {
    mainsnak: Snak,
    #[serde(default)]
    rank: String,
    #[serde(default)]
    qualifiers: HashMap<String, IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct Snak {
    datavalue: Option<DataValue>,
}

#[derive(Debug, Deserialize)]
struct DataValue {
    value: Value,
}

impl Entity {
    fn label(&self, languages: &[&str]) -> Option<&str> {
        languages
            .iter()
            .find_map(|language| self.labels.get(*language))
            .map(|text| text.value.as_str())
    }

    fn names(&self) -> impl Iterator<Item = &str> {
        self.labels
            .values()
            .chain(self.aliases.values().flatten())
            .map(|text| text.value.as_str())
    }

    fn named(&self) -> Named {
        let any = self.labels.values().next().map(|text| text.value.as_str());
        let name_en = self.label(&ENGLISH).or(any).unwrap_or(self.id.as_str());
        let name = if self.id == TAIWAN {
            "中国台湾"
        } else {
            self.label(&CHINESE).unwrap_or(name_en)
        };
        Named {
            id: self.id.clone(),
            name: clip(name),
            name_en: clip(name_en),
        }
    }

    fn sitelink(&self, site: &str) -> Option<&str> {
        self.sitelinks.get(site).map(|link| link.title.as_str())
    }

    /// The values to rely on: preferred statements when there are any, otherwise the
    /// normal ones that have not ended.
    fn values(&self, property: &str) -> Vec<&Value> {
        let statements = self.claims.get(property).map_or(&[][..], Vec::as_slice);
        let preferred = statements
            .iter()
            .filter(|statement| statement.rank == "preferred")
            .collect::<Vec<_>>();
        let chosen = if preferred.is_empty() {
            statements
                .iter()
                .filter(|statement| {
                    statement.rank == "normal" && !statement.qualifiers.contains_key("P582")
                })
                .collect()
        } else {
            preferred
        };
        chosen
            .into_iter()
            .filter_map(|statement| statement.mainsnak.datavalue.as_ref())
            .map(|data| &data.value)
            .collect()
    }

    fn item(&self, property: &str) -> Option<String> {
        self.values(property)
            .into_iter()
            .find_map(|value| value["id"].as_str().filter(|id| is_item(id)))
            .map(str::to_owned)
    }

    /// Coordinates on Earth.
    fn coordinates(&self) -> Option<(f64, f64)> {
        self.values("P625").into_iter().find_map(|value| {
            if value["globe"]
                .as_str()
                .is_some_and(|globe| !globe.ends_with("/Q2"))
            {
                return None;
            }
            let (latitude, longitude) = (value["latitude"].as_f64()?, value["longitude"].as_f64()?);
            ((-90.0..=90.0).contains(&latitude) && (-180.0..=180.0).contains(&longitude))
                .then_some((latitude, longitude))
        })
    }
}

/// Finds the place the post names, trying the model's whole names before their parts
/// ("Nonomiya Shrine, Kyoto"). Names that come earlier win, then longer matches, then items
/// whose own names match, then better-known items. A post's short form of a name, such as
/// "กรุงเทพ" for "กรุงเทพมหานคร", counts when the search for that form found the item.
pub(super) async fn locate(
    state: &AppState,
    wikidata: &Wikidata,
    names: &[String],
    text: &str,
    lang: &str,
) -> Result<Option<Located>> {
    let text = fold(text);
    let languages = search_languages(lang);
    // A name is searched once, however often the model repeats it.
    let mut seen = HashSet::new();
    let mut distinct = |names: &mut dyn Iterator<Item = &str>| {
        names
            .map(str::trim)
            .filter(|name| !name.is_empty() && seen.insert(fold(name)))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let wholes = distinct(&mut names.iter().map(String::as_str));
    let parts = distinct(
        &mut names
            .iter()
            .flat_map(|name| name.split([',', '，', '、', '/', '·'])),
    );
    for round in [wholes, parts] {
        // Each item keeps the first name that found it, and how much of the post that name covers.
        let mut found = HashMap::<String, (usize, usize)>::new();
        for (index, name) in round.iter().enumerate() {
            let named = matched(&text, name).unwrap_or_default();
            for language in &languages {
                for id in wikidata.search(state, name, language).await? {
                    found.entry(id).or_insert((index, named));
                }
            }
        }
        if found.is_empty() {
            continue;
        }
        let ids = found.keys().cloned().collect::<Vec<_>>();
        let candidates = wikidata.entities(state, &ids, CANDIDATE).await?;
        let mut ranked = candidates
            .values()
            .filter_map(|entity| {
                let (index, named) = *found.get(&entity.id)?;
                let own = entity
                    .names()
                    .filter_map(|name| matched(&text, name))
                    .max()
                    .unwrap_or_default();
                (own.max(named) > 0).then_some((
                    index,
                    Reverse(own.max(named)),
                    Reverse(own),
                    Reverse(entity.sitelinks.len()),
                    &entity.id,
                ))
            })
            .collect::<Vec<_>>();
        ranked.sort();
        for (.., id) in ranked.into_iter().take(MAX_TRIES) {
            let spot = wikidata.entity(state, id, PLACE).await?;
            if spot.coordinates().is_none() {
                continue;
            }
            // An item that leads to no destination leaves the next one its chance.
            let Some(destination) = destination(state, wikidata, &spot).await? else {
                continue;
            };
            let spot = wikidata
                .names(state, &[&spot.id])
                .await?
                .remove(&spot.id)
                .unwrap_or_else(|| spot.named());
            return Ok(Some(Located { spot, destination }));
        }
    }
    Ok(None)
}

/// The nearest place at or above the spot with an English Wikivoyage guide. Destinations
/// already stored, or reached before from the same spot, are taken from the database.
async fn destination(
    state: &AppState,
    wikidata: &Wikidata,
    spot: &Entity,
) -> Result<Option<Destination>> {
    if let Some(known) = sqlx::query_as::<_, Destination>(
        "SELECT p.id, p.slug, p.name, p.name_en, p.country_id, p.country, p.country_en, p.latitude,
                p.longitude, p.voyage, p.wiki_zh, p.wiki_en
         FROM travel_places p
         WHERE p.id = $1 OR p.id = (SELECT place_id FROM travel_moments WHERE spot_id = $1 LIMIT 1)
         ORDER BY p.id = $1 DESC LIMIT 1",
    )
    .bind(&spot.id)
    .fetch_optional(&state.pool)
    .await?
    {
        return Ok(Some(known));
    }
    let mut current = None::<Arc<Entity>>;
    for _ in 0..=MAX_DEPTH {
        let place = current.as_deref().unwrap_or(spot);
        if place.sitelink("enwikivoyage").is_some() {
            return guided(state, wikidata, place, spot).await;
        }
        let Some(parent) = place.item("P131") else {
            break;
        };
        if let Some(known) = stored(state, &parent).await? {
            return Ok(Some(known));
        }
        current = Some(wikidata.entity(state, &parent, PLACE).await?);
    }
    // A country's statements run to megabytes; its labels and guide are all that is needed.
    let Some(country) = spot.item("P17") else {
        return Ok(None);
    };
    if let Some(known) = stored(state, &country).await? {
        return Ok(Some(known));
    }
    let country = wikidata.entity(state, &country, COUNTRY).await?;
    if country.sitelink("enwikivoyage").is_none() {
        return Ok(None);
    }
    guided(state, wikidata, &country, spot).await
}

/// The destination a place with an English Wikivoyage guide stands for: the place itself, or,
/// for a district guide of a big city ("Tokyo/Asakusa", also Vatican City's "Rome/Vatican"),
/// the city ("Tokyo").
async fn guided(
    state: &AppState,
    wikidata: &Wikidata,
    place: &Entity,
    spot: &Entity,
) -> Result<Option<Destination>> {
    let title = place.sitelink("enwikivoyage").unwrap_or_default();
    let Some((city, _)) = title.split_once('/') else {
        return describe(state, wikidata, place, spot).await.map(Some);
    };
    let Some(city) = wikidata.article(state, city).await? else {
        return Ok(None);
    };
    if let Some(known) = stored(state, &city.id).await? {
        return Ok(Some(known));
    }
    describe(state, wikidata, &city, spot).await.map(Some)
}

async fn stored(state: &AppState, id: &str) -> Result<Option<Destination>> {
    Ok(sqlx::query_as::<_, Destination>(
        "SELECT id, slug, name, name_en, country_id, country, country_en, latitude, longitude,
                voyage, wiki_zh, wiki_en
         FROM travel_places WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?)
}

async fn describe(
    state: &AppState,
    wikidata: &Wikidata,
    place: &Entity,
    spot: &Entity,
) -> Result<Destination> {
    let voyage = place
        .sitelink("enwikivoyage")
        .unwrap_or_default()
        .to_owned();
    // Regions and countries without statements are placed at the spot.
    let (latitude, longitude) = place
        .coordinates()
        .or_else(|| spot.coordinates())
        .ok_or_else(|| AppError::Internal("目的地没有坐标".into()))?;
    let country_id = place
        .item("P17")
        .or_else(|| spot.item("P17"))
        .unwrap_or_else(|| place.id.clone());
    let country_id = if country_id == TAIWAN {
        CHINA.to_owned()
    } else {
        country_id
    };
    let mut names = wikidata.names(state, &[&place.id, &country_id]).await?;
    let named = names.remove(&place.id).unwrap_or_else(|| place.named());
    let country = names.remove(&country_id).unwrap_or_else(|| named.clone());
    Ok(Destination {
        slug: slug(&voyage).unwrap_or_else(|| place.id.to_ascii_lowercase()),
        id: place.id.clone(),
        name: named.name,
        name_en: named.name_en,
        country_id,
        country: country.name,
        country_en: country.name_en,
        latitude,
        longitude,
        voyage,
        wiki_zh: place.sitelink("zhwiki").map(str::to_owned),
        wiki_en: place.sitelink("enwiki").map(str::to_owned),
    })
}

/// Saves a new destination. A slug another destination already uses gives way to the id.
pub(super) async fn store(state: &AppState, destination: &Destination) -> Result<()> {
    for slug in [
        destination.slug.clone(),
        destination.id.to_ascii_lowercase(),
    ] {
        sqlx::query(
            "INSERT INTO travel_places (id, slug, name, name_en, country_id, country, country_en,
                 latitude, longitude, voyage, wiki_zh, wiki_en)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT DO NOTHING",
        )
        .bind(&destination.id)
        .bind(&slug)
        .bind(&destination.name)
        .bind(&destination.name_en)
        .bind(&destination.country_id)
        .bind(&destination.country)
        .bind(&destination.country_en)
        .bind(destination.latitude)
        .bind(destination.longitude)
        .bind(&destination.voyage)
        .bind(&destination.wiki_zh)
        .bind(&destination.wiki_en)
        .execute(&state.pool)
        .await?;
        if stored(state, &destination.id).await?.is_some() {
            return Ok(());
        }
    }
    Err(AppError::Internal("目的地无法保存".into()))
}

async fn api(state: &AppState, query: &[(&str, &str)]) -> Result<Response> {
    let mut url = Url::parse(API).expect("fixed Wikidata URL");
    url.query_pairs_mut()
        .extend_pairs(query)
        .append_pair("format", "json");
    let bytes = super::wikimedia(state, url).await?;
    let response = serde_json::from_slice::<Response>(&bytes)?;
    if let Some(error) = response.error {
        return Err(AppError::Internal(
            format!("Wikidata: {}", error["info"]).into(),
        ));
    }
    Ok(response)
}

impl Wikidata {
    async fn search(&self, state: &AppState, name: &str, language: &str) -> Result<Vec<String>> {
        let key = (language.to_owned(), name.to_owned());
        if let Some(ids) = lock(&self.searches).get(&key) {
            return Ok(ids.clone());
        }
        let response = api(
            state,
            &[
                ("action", "wbsearchentities"),
                ("search", name),
                ("language", language),
                ("uselang", language),
                ("type", "item"),
                ("limit", SEARCH_LIMIT),
            ],
        )
        .await?;
        let ids = response
            .search
            .into_iter()
            .map(|hit| hit.id)
            .filter(|id| is_item(id))
            .collect::<Vec<_>>();
        lock(&self.searches).insert(key, ids.clone());
        Ok(ids)
    }

    /// The items with the given parts read, from this run's answers where possible.
    async fn entities(
        &self,
        state: &AppState,
        ids: &[String],
        props: &'static str,
    ) -> Result<HashMap<String, Arc<Entity>>> {
        let mut found = HashMap::new();
        let mut missing = Vec::new();
        {
            let known = lock(&self.entities);
            for id in ids {
                match known.get(&(props, id.clone())) {
                    Some(entity) => {
                        found.insert(id.clone(), Arc::clone(entity));
                    }
                    None => missing.push(id.as_str()),
                }
            }
        }
        for chunk in missing.chunks(50) {
            let ids = chunk.join("|");
            let mut query = vec![("action", "wbgetentities"), ("ids", ids.as_str())];
            query.extend(if props == NAMES {
                // Wikidata converts traditional Chinese labels when no simplified one exists.
                [
                    ("props", "labels"),
                    ("languages", "zh-cn|en"),
                    ("languagefallback", "1"),
                ]
                .to_vec()
            } else {
                vec![("props", props)]
            });
            let response = api(state, &query).await?;
            // Redirected ids come back under their target, which carries the id.
            let mut known = lock(&self.entities);
            for entity in response.entities.into_values() {
                if entity.id.is_empty() {
                    continue;
                }
                let entity = Arc::new(entity);
                known.insert((props, entity.id.clone()), Arc::clone(&entity));
                found.insert(entity.id.clone(), entity);
            }
        }
        Ok(found)
    }

    /// The item an English Wikivoyage guide belongs to, read as a place.
    async fn article(&self, state: &AppState, title: &str) -> Result<Option<Arc<Entity>>> {
        if let Some(found) = lock(&self.articles).get(title) {
            return Ok(found.clone());
        }
        let response = api(
            state,
            &[
                ("action", "wbgetentities"),
                ("sites", "enwikivoyage"),
                ("titles", title),
                ("props", PLACE),
            ],
        )
        .await?;
        // A title without an item comes back as a missing entity, which has no id.
        let found = response
            .entities
            .into_values()
            .find(|entity| is_item(&entity.id))
            .map(Arc::new);
        lock(&self.articles).insert(title.to_owned(), found.clone());
        Ok(found)
    }

    async fn entity(&self, state: &AppState, id: &str, props: &'static str) -> Result<Arc<Entity>> {
        self.entities(state, &[id.to_owned()], props)
            .await?
            .remove(id)
            .ok_or_else(|| AppError::Internal(format!("Wikidata 没有 {id}").into()))
    }

    /// Display names in simplified Chinese and English.
    async fn names(&self, state: &AppState, ids: &[&str]) -> Result<HashMap<String, Named>> {
        let ids = ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
        Ok(self
            .entities(state, &ids, NAMES)
            .await?
            .into_iter()
            .map(|(id, entity)| (id, entity.named()))
            .collect())
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Wikidata's language for the post's: X writes Indonesian as `in` and Hebrew as `iw`, and
/// marks posts without words as `und`, `zxx` or `q…`. English names are searched as well.
fn search_languages(lang: &str) -> Vec<&str> {
    let lang = match lang {
        "in" => "id",
        "iw" => "he",
        lang if lang.len() >= 2
            && lang.len() <= 8
            && !lang.starts_with('q')
            && !matches!(lang, "und" | "zxx")
            && lang.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') =>
        {
            lang
        }
        _ => "en",
    };
    if lang == "en" {
        vec!["en"]
    } else {
        vec![lang, "en"]
    }
}

pub(super) fn is_item(id: &str) -> bool {
    id.strip_prefix('Q').is_some_and(|digits| {
        (1..=12).contains(&digits.len())
            && !digits.starts_with('0')
            && digits.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Lower case with the accents of Latin letters removed; other scripts keep their marks.
pub(super) fn fold(text: &str) -> String {
    let mut folded = String::with_capacity(text.len());
    let mut latin = false;
    for c in text.nfkd() {
        if is_combining_mark(c) {
            if !latin {
                folded.push(c);
            }
            continue;
        }
        latin = c.is_ascii_alphabetic();
        folded.extend(c.to_lowercase());
    }
    folded.nfc().collect()
}

/// How much of the folded post a name covers, if it appears there. Latin names match whole
/// words (a long one may also begin a word, as hashtags run words together); names in other
/// scripts, which need not separate words, match anywhere and count double per character.
pub(super) fn matched(text: &str, name: &str) -> Option<usize> {
    let name = fold(name.trim());
    if name.is_ascii() {
        let words = |text: &str| {
            text.split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let wanted = words(&name);
        let joined = wanted.concat();
        if joined.len() < 2 {
            return None;
        }
        let found = words(text);
        let whole = found.windows(wanted.len()).any(|window| window == wanted);
        let begins =
            joined.len() >= MIN_PREFIX && found.iter().any(|word| word.starts_with(&joined));
        (whole || begins).then_some(joined.len())
    } else {
        let length = name.chars().filter(|c| !c.is_whitespace()).count();
        (length >= 2 && text.contains(&name)).then_some(length * 2)
    }
}

/// An address segment from an English Wikivoyage title: "São Paulo" → `sao-paulo`,
/// "Santiago (Chile)" → `santiago-chile`.
pub(super) fn slug(title: &str) -> Option<String> {
    let mut slug = String::new();
    for c in fold(title).chars() {
        let mapped = match c {
            'ł' => "l",
            'ø' => "o",
            'æ' => "ae",
            'œ' => "oe",
            'ß' => "ss",
            'đ' | 'ð' => "d",
            'þ' => "th",
            'ı' => "i",
            c if c.is_ascii_alphanumeric() => {
                slug.push(c);
                continue;
            }
            c if c.is_alphanumeric() => return None,
            _ => "-",
        };
        if mapped == "-" {
            if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
        } else {
            slug.push_str(mapped);
        }
    }
    let slug = slug.trim_end_matches('-');
    (!slug.is_empty() && slug.len() <= 80).then(|| slug.to_owned())
}

fn clip(value: &str) -> String {
    value.trim().chars().take(120).collect()
}
