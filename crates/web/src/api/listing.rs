// La page publique d'un bot, réglée par son créateur : texte, catégorie, images, commandes, formules.
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use platform_core::ProductId;
use platform_store::{ImageKind, ListedCommand, Plan};
use serde::{Deserialize, Serialize};

use super::auth::{User, Visitor};
use super::bots::owned;
use super::error::ApiError;
use crate::html;
use crate::image::{checked, ImageError};
use crate::state::WebState;

pub(crate) const CATEGORIES: [&str; 8] = [
    "moderation",
    "music",
    "games",
    "utility",
    "fun",
    "economy",
    "social",
    "other",
];
const MAX_DESCRIPTION: usize = 500;
const MAX_COMMANDS: usize = 25;
const MAX_PLANS: usize = 3;
const MAX_PRICE_CENTS: u32 = 100_000;

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct CommandDto {
    pub name: String,
    pub description: String,
}

#[derive(Deserialize, Serialize, Clone, PartialEq, Eq, Debug)]
pub(crate) struct PlanDto {
    #[serde(default)]
    pub id: u64,
    pub name: String,
    pub price_cents: u32,
    pub days: u32,
}

#[derive(Serialize)]
pub(crate) struct ListingView {
    pub(crate) description: String,
    pub(crate) category: String,
    pub(crate) has_avatar: bool,
    pub(crate) has_banner: bool,
    // Change à chaque modification : l'adresse d'une image change, le navigateur la recharge.
    pub(crate) version: i64,
    commands: Vec<CommandDto>,
    plans: Vec<PlanDto>,
    categories: [&'static str; 8],
}

// Champ sans valeur « par défaut » à restaurer : vide reste vide plutôt que `None`.
fn clean_text(raw: &str, max: usize, newlines: bool) -> Option<String> {
    html::clean_text(raw, max, newlines).map(Option::unwrap_or_default)
}

// Les noms de commandes de Discord : 1 à 32 caractères, minuscules, chiffres, - et _.
pub(crate) fn valid_command_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub(crate) fn clean_commands(raw: &[CommandDto]) -> Result<Vec<ListedCommand>, ApiError> {
    if raw.len() > MAX_COMMANDS {
        return Err(ApiError::bad_request(
            "too_many_commands",
            format!("At most {MAX_COMMANDS} commands."),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    raw.iter()
        .map(|c| {
            let name = c.name.trim().trim_start_matches('/').to_string();
            if !valid_command_name(&name) {
                return Err(ApiError::bad_request(
                    "invalid_command",
                    "A command name has 1 to 32 characters: lowercase letters, digits, - and _.",
                ));
            }
            if !seen.insert(name.clone()) {
                return Err(ApiError::bad_request(
                    "duplicate_command",
                    format!("/{name} is listed twice."),
                ));
            }
            let description = clean_text(&c.description, 100, false)
                .filter(|d| !d.is_empty())
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_command",
                        "Each command needs a description of up to 100 characters.",
                    )
                })?;
            Ok(ListedCommand { name, description })
        })
        .collect()
}

pub(crate) fn clean_plans(raw: &[PlanDto]) -> Result<Vec<Plan>, ApiError> {
    if raw.len() > MAX_PLANS {
        return Err(ApiError::bad_request(
            "too_many_plans",
            format!("At most {MAX_PLANS} plans."),
        ));
    }
    raw.iter()
        .map(|p| {
            let name = clean_text(&p.name, 40, false).filter(|n| !n.is_empty());
            let (Some(name), true, true) = (name, p.price_cents <= MAX_PRICE_CENTS, (1..=366).contains(&p.days)) else {
                return Err(ApiError::bad_request(
                    "invalid_plan",
                    "A plan has a name (up to 40 characters), a price up to 1000.00 and a length of 1 to 366 days.",
                ));
            };
            Ok(Plan { id: 0, name, price_cents: p.price_cents, days: p.days })
        })
        .collect()
}

pub(crate) fn view(state: &WebState, id: u64) -> Result<ListingView, ApiError> {
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let product = ProductId(id);
    let listing = store.listing(product).map_err(|_| ApiError::internal())?;
    let commands = store
        .commands_of(product)
        .map_err(|_| ApiError::internal())?;
    let plans = store.plans_of(product).map_err(|_| ApiError::internal())?;
    Ok(ListingView {
        description: listing
            .as_ref()
            .map(|l| l.description.clone())
            .unwrap_or_default(),
        category: listing
            .as_ref()
            .map_or_else(|| "other".to_string(), |l| l.category.clone()),
        has_avatar: listing.as_ref().is_some_and(|l| l.has_avatar),
        has_banner: listing.as_ref().is_some_and(|l| l.has_banner),
        version: listing.as_ref().map_or(0, |l| l.updated_at),
        commands: commands
            .into_iter()
            .map(|c| CommandDto {
                name: c.name,
                description: c.description,
            })
            .collect(),
        plans: plans
            .into_iter()
            .map(|p| PlanDto {
                id: p.id,
                name: p.name,
                price_cents: p.price_cents,
                days: p.days,
            })
            .collect(),
        categories: CATEGORIES,
    })
}

pub(crate) async fn read(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
) -> Result<Json<ListingView>, ApiError> {
    owned(&state, &auth, id)?;
    Ok(Json(view(&state, id)?))
}

#[derive(Deserialize)]
pub(crate) struct ListingForm {
    description: String,
    category: String,
    commands: Vec<CommandDto>,
    plans: Vec<PlanDto>,
}

pub(crate) async fn save(
    State(state): State<WebState>,
    User(auth): User,
    Path(id): Path<u64>,
    Json(form): Json<ListingForm>,
) -> Result<Json<ListingView>, ApiError> {
    owned(&state, &auth, id)?;
    let description = clean_text(&form.description, MAX_DESCRIPTION, true).ok_or_else(|| {
        ApiError::bad_request(
            "invalid_description",
            format!("The description has up to {MAX_DESCRIPTION} characters."),
        )
    })?;
    if !CATEGORIES.contains(&form.category.as_str()) {
        return Err(ApiError::bad_request(
            "invalid_category",
            "Choose a category from the list.",
        ));
    }
    let commands = clean_commands(&form.commands)?;
    let plans = clean_plans(&form.plans)?;
    {
        let store = state.store.lock().map_err(|_| ApiError::internal())?;
        let product = ProductId(id);
        let now = (state.clock)();
        store
            .save_listing_text(product, &description, &form.category, now)
            .map_err(|_| ApiError::internal())?;
        store
            .replace_commands(product, &commands)
            .map_err(|_| ApiError::internal())?;
        store
            .replace_plans(product, &plans)
            .map_err(|_| ApiError::internal())?;
    }
    Ok(Json(view(&state, id)?))
}

fn kind_of(raw: &str) -> Result<ImageKind, ApiError> {
    match raw {
        "avatar" => Ok(ImageKind::Avatar),
        "banner" => Ok(ImageKind::Banner),
        _ => Err(ApiError::not_found()),
    }
}

fn image_error(error: ImageError, max: usize) -> ApiError {
    match error {
        ImageError::Empty => ApiError::bad_request("empty_image", "The file is empty."),
        ImageError::TooLarge => ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "image_too_large",
            format!(
                "The image is larger than {} MB.",
                (max / (1024 * 1024)).max(1)
            ),
        ),
        ImageError::UnsupportedType => ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "image_type",
            "Use a PNG, JPEG or GIF image.",
        ),
    }
}

// Le fichier arrive tel quel dans le corps de la requête ; son type vient de son contenu.
pub(crate) async fn upload(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, kind)): Path<(u64, String)>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    let kind = kind_of(&kind)?;
    let max = state.config.max_image_bytes;
    let mime = checked(&body, max).map_err(|e| image_error(e, max))?;
    state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .set_listing_image(ProductId(id), kind, mime, &body, (state.clock)())
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn remove_image(
    State(state): State<WebState>,
    User(auth): User,
    Path((id, kind)): Path<(u64, String)>,
) -> Result<StatusCode, ApiError> {
    owned(&state, &auth, id)?;
    state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .clear_listing_image(ProductId(id), kind_of(&kind)?, (state.clock)())
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

// Public : une image de page n'est montrée que pour un bot publié, ou à son créateur.
pub(crate) async fn image(
    State(state): State<WebState>,
    Visitor(auth): Visitor,
    Path((id, kind)): Path<(u64, String)>,
) -> Result<Response, ApiError> {
    let kind = kind_of(&kind)?;
    let store = state.store.lock().map_err(|_| ApiError::internal())?;
    let product = store
        .product(ProductId(id))
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    let published = ["free", "premium"]
        .iter()
        .any(|t| store.credential(product.id, t).ok().flatten().is_some());
    if !published && auth.is_none_or(|a| a.user_id != product.owner) {
        return Err(ApiError::not_found());
    }
    let (mime, bytes) = store
        .listing_image(product.id, kind)
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(&mime).map_err(|_| ApiError::internal())?,
    );
    // L'adresse porte la version : une image changée a une autre adresse.
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    Ok((headers, bytes).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str, description: &str) -> CommandDto {
        CommandDto {
            name: name.into(),
            description: description.into(),
        }
    }

    #[test]
    fn command_names_follow_discords_rule() {
        for ok in ["ping", "a", "my-cmd_2", &"x".repeat(32)] {
            assert!(valid_command_name(ok), "{ok}");
        }
        for bad in ["", "Ping", "has space", "é", "a/b", &"x".repeat(33)] {
            assert!(!valid_command_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_leading_slash_is_dropped_and_duplicates_are_refused() {
        let ok = clean_commands(&[cmd("/ping", "Pong")]).unwrap_or_default();
        assert_eq!(ok[0].name, "ping");
        assert!(clean_commands(&[cmd("a", "x"), cmd("/a", "y")]).is_err());
    }

    #[test]
    fn a_command_needs_a_description_within_100_characters() {
        assert!(clean_commands(&[cmd("a", "")]).is_err());
        assert!(clean_commands(&[cmd("a", &"x".repeat(101))]).is_err());
        assert!(clean_commands(&[cmd("a", &"x".repeat(100))]).is_ok());
        assert!(clean_commands(&[cmd("a", "two\nlines")]).is_err());
    }

    #[test]
    fn the_command_count_is_capped() {
        let many: Vec<_> = (0..=MAX_COMMANDS)
            .map(|i| cmd(&format!("c{i}"), "d"))
            .collect();
        assert!(clean_commands(&many).is_err());
        assert!(clean_commands(&many[..MAX_COMMANDS]).is_ok());
    }

    #[test]
    fn plans_are_bounded_in_price_length_and_count() {
        let plan = |n: &str, c: u32, d: u32| PlanDto {
            id: 0,
            name: n.into(),
            price_cents: c,
            days: d,
        };
        assert!(clean_plans(&[plan("Pro", 500, 30)]).is_ok());
        assert!(clean_plans(&[plan("Free trial", 0, 7)]).is_ok());
        for bad in [
            plan("", 500, 30),
            plan("Pro", MAX_PRICE_CENTS + 1, 30),
            plan("Pro", 500, 0),
            plan("Pro", 500, 367),
        ] {
            assert!(clean_plans(&[bad]).is_err());
        }
        assert!(clean_plans(&vec![plan("a", 1, 1); MAX_PLANS + 1]).is_err());
    }

    #[test]
    fn text_keeps_newlines_only_where_allowed() {
        assert_eq!(
            clean_text("  hi\nthere ", 20, true).as_deref(),
            Some("hi\nthere")
        );
        assert_eq!(clean_text("hi\nthere", 20, false), None);
        assert_eq!(clean_text("a\u{0}b", 20, true), None);
        assert_eq!(
            clean_text(&"é".repeat(5), 5, false).map(|t| t.chars().count()),
            Some(5)
        );
        assert_eq!(clean_text(&"é".repeat(6), 5, false), None);
    }
}
