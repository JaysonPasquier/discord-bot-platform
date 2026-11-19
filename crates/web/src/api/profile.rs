// Profil d'un bot dans un serveur abonné (nom, bio, avatar, bannière).
use axum::extract::{Multipart, Path, State};
use axum::http::StatusCode;
use axum::Json;
use platform_core::{GuildId, PremiumStatus, ProductId};
use platform_runner::Tier;
use platform_store::{BotCredential, GuildProfile};
use serde::Serialize;

use super::auth::User;
use super::error::ApiError;
use crate::billing::{load_entitlement, status_of};
use crate::bots::open_token;
use crate::guild_profile::{ProfileError, ProfilePatch};
use crate::html::clean_text;
use crate::image::{data_uri, ImageError};
use crate::state::WebState;

const MAX_NICK_CHARS: usize = 32;
const MAX_BIO_CHARS: usize = 190;

#[derive(Serialize)]
pub(crate) struct ProfileData {
    nick: Option<String>,
    bio: Option<String>,
    avatar_set: bool,
    banner_set: bool,
    bot_name: String,
    client_id: String,
    max_nick_chars: usize,
    max_bio_chars: usize,
    max_image_mb: usize,
}

struct Subscribed {
    credential: BotCredential,
    profile: Option<GuildProfile>,
}

fn check_subscribed(
    state: &WebState,
    user_id: u64,
    product: u64,
    guild: u64,
) -> Result<Subscribed, ApiError> {
    let (product_id, guild_id) = (ProductId(product), GuildId(guild));
    let Ok(store) = state.store.lock() else {
        return Err(ApiError::internal());
    };
    let owns = store
        .subscriptions_of(user_id)
        .map_err(|_| ApiError::internal())?
        .into_iter()
        .any(|s| s.guild == guild_id && s.product == product_id);
    if !owns {
        return Err(ApiError::not_found());
    }
    store
        .product(product_id)
        .map_err(|_| ApiError::internal())?
        .ok_or_else(ApiError::not_found)?;
    let entitlement =
        load_entitlement(&store, guild_id, product_id).map_err(|_| ApiError::internal())?;
    if status_of(state, &entitlement) == PremiumStatus::Expired {
        return Err(ApiError::forbidden(
            "subscription_expired",
            "Subscription not active.",
        ));
    }
    let profile = store
        .guild_profile(guild_id, product_id)
        .map_err(|_| ApiError::internal())?;
    let credential = store
        .credential(product_id, "premium")
        .map_err(|_| ApiError::internal())?
        .ok_or_else(|| {
            ApiError::conflict(
                "no_premium_bot",
                "The creator has not connected the premium bot yet.",
            )
        })?;
    Ok(Subscribed {
        credential,
        profile,
    })
}

pub(crate) async fn get(
    State(state): State<WebState>,
    User(auth): User,
    Path((product, guild)): Path<(u64, u64)>,
) -> Result<Json<ProfileData>, ApiError> {
    let sub = check_subscribed(&state, auth.user_id, product, guild)?;
    let (nick, bio, avatar_set, banner_set) = sub.profile.map_or((None, None, false, false), |p| {
        (p.nick, p.bio, p.avatar_set, p.banner_set)
    });
    Ok(Json(ProfileData {
        nick,
        bio,
        avatar_set,
        banner_set,
        bot_name: sub.credential.bot_name,
        client_id: sub.credential.client_id,
        max_nick_chars: MAX_NICK_CHARS,
        max_bio_chars: MAX_BIO_CHARS,
        max_image_mb: state.config.max_image_bytes / (1024 * 1024),
    }))
}

fn image_change(
    bytes: &[u8],
    remove: bool,
    max: usize,
) -> Result<Option<Option<String>>, ImageError> {
    if !bytes.is_empty() {
        return data_uri(bytes, max).map(|uri| Some(Some(uri)));
    }
    Ok(remove.then_some(None))
}

fn profile_error_to_api(error: ProfileError) -> ApiError {
    match error {
        ProfileError::BadToken => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "bad_token",
            "Discord refused the bot's token. Ask the creator to paste a new one.",
        ),
        ProfileError::NotInServer => ApiError::conflict(
            "bot_not_in_server",
            "Invite the premium bot to this server first.",
        ),
        ProfileError::Forbidden => ApiError::forbidden(
            "discord_refused",
            "The bot may lack the Change Nickname permission. Give its role that permission and try again.",
        ),
        ProfileError::Invalid => ApiError::bad_request(
            "discord_invalid",
            "Discord refused these values. Check the name, bio and images.",
        ),
        ProfileError::RateLimited => ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "discord_rate_limited",
            "Discord limits how often a bot profile can change. Try again in a few minutes.",
        ),
        ProfileError::Unavailable => ApiError::new(
            StatusCode::BAD_GATEWAY,
            "discord_unavailable",
            "Discord did not answer. Try again in a minute.",
        ),
    }
}

fn image_error_to_api(error: ImageError, max_bytes: usize) -> ApiError {
    let msg = match error {
        ImageError::Empty => "The file is empty.".to_string(),
        ImageError::TooLarge => {
            format!("The image is larger than {} MB.", max_bytes / (1024 * 1024))
        }
        ImageError::UnsupportedType => "Only PNG, JPEG and GIF images are accepted.".to_string(),
    };
    ApiError::bad_request("invalid_image", msg)
}

#[derive(Default)]
struct ProfileForm {
    nick: String,
    bio: String,
    avatar: Vec<u8>,
    banner: Vec<u8>,
    remove_avatar: bool,
    remove_banner: bool,
}

async fn read_multipart(mut multipart: Multipart) -> Option<ProfileForm> {
    let mut form = ProfileForm::default();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(_) => return None,
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "nick" => form.nick = field.text().await.ok()?,
            "bio" => form.bio = field.text().await.ok()?,
            "avatar" => form.avatar = field.bytes().await.ok()?.to_vec(),
            "banner" => form.banner = field.bytes().await.ok()?.to_vec(),
            "remove_avatar" => form.remove_avatar = true,
            "remove_banner" => form.remove_banner = true,
            _ => {}
        }
    }
    Some(form)
}

pub(crate) async fn apply(
    State(state): State<WebState>,
    User(auth): User,
    Path((product, guild)): Path<(u64, u64)>,
    multipart: Multipart,
) -> Result<StatusCode, ApiError> {
    let sub = check_subscribed(&state, auth.user_id, product, guild)?;
    let Some(form) = read_multipart(multipart).await else {
        return Err(ApiError::bad_request(
            "invalid_form",
            "The form could not be read. The files may be too large.",
        ));
    };
    let (Some(nick), Some(bio)) = (
        clean_text(&form.nick, MAX_NICK_CHARS, false),
        clean_text(&form.bio, MAX_BIO_CHARS, true),
    ) else {
        return Err(ApiError::bad_request(
            "invalid_text",
            format!("Name: at most {MAX_NICK_CHARS} chars. Bio: at most {MAX_BIO_CHARS} chars, no control characters."),
        ));
    };
    let max_image = state.config.max_image_bytes;
    let (avatar, banner) = match (
        image_change(&form.avatar, form.remove_avatar, max_image),
        image_change(&form.banner, form.remove_banner, max_image),
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return Err(image_error_to_api(e, max_image)),
    };
    let key = format!("profile-{product}-{guild}");
    if let Err(secs) =
        state
            .limiter
            .check_and_record(&key, (state.clock)(), state.config.deploy_cooldown_secs)
    {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            format!("Wait {secs} more second(s) before changing the profile again."),
        ));
    }
    let Some(token) = open_token(&state, product, Tier::Premium, &sub.credential) else {
        return Err(ApiError::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "unreadable_token",
            "The stored token cannot be read. Ask the creator to paste it again.",
        ));
    };
    let patch = ProfilePatch {
        nick: Some(nick.clone()),
        bio: Some(bio.clone()),
        avatar: avatar.clone(),
        banner: banner.clone(),
    };
    let profiles = state.profiles.clone();
    tokio::task::spawn_blocking(move || profiles.apply(&token, guild, &patch))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(profile_error_to_api)?;
    let before = sub.profile.as_ref();
    let saved = GuildProfile {
        nick,
        bio,
        avatar_set: avatar.map_or(before.is_some_and(|p| p.avatar_set), |a| a.is_some()),
        banner_set: banner.map_or(before.is_some_and(|p| p.banner_set), |b| b.is_some()),
        updated_at: (state.clock)(),
    };
    state
        .store
        .lock()
        .map_err(|_| ApiError::internal())?
        .save_guild_profile(GuildId(guild), ProductId(product), &saved)
        .map_err(|_| ApiError::internal())?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_are_trimmed_and_empty_means_default() {
        assert_eq!(
            clean_text("  Rex  ", 32, false),
            Some(Some("Rex".to_string()))
        );
        assert_eq!(clean_text("   ", 32, false), Some(None));
        assert_eq!(clean_text("", 32, false), Some(None));
    }

    #[test]
    fn length_limit_counts_characters_not_bytes() {
        assert!(clean_text(&"é".repeat(32), 32, false).is_some());
        assert!(clean_text(&"é".repeat(33), 32, false).is_none());
        assert!(clean_text(&"a".repeat(190), 190, true).is_some());
        assert!(clean_text(&"a".repeat(191), 190, true).is_none());
    }

    #[test]
    fn control_characters_are_refused_and_newlines_only_where_allowed() {
        assert!(clean_text("a\u{0}b", 32, true).is_none());
        assert!(clean_text("a\u{7}b", 32, false).is_none());
        assert!(clean_text("a\nb", 32, false).is_none());
        assert_eq!(
            clean_text("a\r\nb", 32, true),
            Some(Some("a\nb".to_string()))
        );
    }

    #[test]
    fn file_wins_over_remove_and_no_file_without_remove_changes_nothing() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        assert!(matches!(image_change(&png, true, 1000), Ok(Some(Some(_)))));
        assert!(matches!(image_change(&png, false, 1000), Ok(Some(Some(_)))));
        assert_eq!(image_change(&[], true, 1000), Ok(Some(None)));
        assert_eq!(image_change(&[], false, 1000), Ok(None));
        assert_eq!(
            image_change(b"not an image", false, 1000),
            Err(ImageError::UnsupportedType)
        );
    }
}
