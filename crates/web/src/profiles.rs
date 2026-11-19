use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use platform_core::{GuildId, PremiumStatus, ProductId};
use platform_runner::Tier;
use platform_store::{BotCredential, GuildProfile, Product};

use crate::billing::{load_entitlement, status_of};
use crate::bots::open_token;
use crate::dashboard::{guild_name, invite_button};
use crate::guild_profile::{ProfileError, ProfilePatch};
use crate::html::{clean_text, escape, AccountTab, Nav};
use crate::image::{data_uri, ImageError};
use crate::render::{
    message_back, not_found, page_in, redirect, require, require_csrf, server_error,
};
use crate::session::Auth;
use crate::state::WebState;

const MAX_NICK_CHARS: usize = 32;
const MAX_BIO_CHARS: usize = 190;

struct Subscribed {
    product: Product,
    credential: BotCredential,
    profile: Option<GuildProfile>,
}

// Seul l'acheteur de cet abonnement, tant qu'il est actif, peut changer le profil du bot dans ce serveur.
fn subscribed(
    state: &WebState,
    auth: &Auth,
    product: u64,
    guild: u64,
) -> Result<Subscribed, Response> {
    let (product_id, guild_id) = (ProductId(product), GuildId(guild));
    let Ok(store) = state.store.lock() else {
        return Err(server_error(state, Some(auth)));
    };
    let owns = store.subscriptions_of(auth.user_id).map(|subs| {
        subs.iter()
            .any(|s| s.guild == guild_id && s.product == product_id)
    });
    let found = store.product(product_id);
    let (Ok(true), Ok(Some(found))) = (owns, found) else {
        return Err(not_found(state, auth));
    };
    let Ok(entitlement) = load_entitlement(&store, guild_id, product_id) else {
        return Err(server_error(state, Some(auth)));
    };
    let profile = store.guild_profile(guild_id, product_id);
    let credential = store.credential(product_id, "premium");
    drop(store);
    if status_of(state, &entitlement) == PremiumStatus::Expired {
        return Err(message_back(
            StatusCode::FORBIDDEN,
            state,
            Some(auth),
            "Subscription not active",
            "Customizing the bot needs an active premium subscription for this server.",
            "/dashboard",
        ));
    }
    let (Ok(profile), Ok(credential)) = (profile, credential) else {
        return Err(server_error(state, Some(auth)));
    };
    let Some(credential) = credential else {
        return Err(message_back(
            StatusCode::CONFLICT,
            state,
            Some(auth),
            "Premium bot not available",
            "The creator has not connected the premium bot yet.",
            "/dashboard",
        ));
    };
    Ok(Subscribed {
        product: found,
        credential,
        profile,
    })
}

pub(crate) async fn show(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((product, guild)): Path<(u64, u64)>,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let sub = match subscribed(&state, &auth, product, guild) {
        Ok(s) => s,
        Err(response) => return response,
    };
    let (nick, bio) = sub
        .profile
        .as_ref()
        .map_or((String::new(), String::new()), |p| {
            (
                p.nick.clone().unwrap_or_default(),
                p.bio.clone().unwrap_or_default(),
            )
        });
    let (avatar_set, banner_set) = sub
        .profile
        .as_ref()
        .map_or((false, false), |p| (p.avatar_set, p.banner_set));
    let csrf = escape(&auth.csrf);
    let flag = |set: bool| if set { "custom" } else { "default" };
    let max_mb = megabytes(state.config.max_image_bytes);
    let file_field = |field: &str, label: &str, set: &str| -> String {
        format!(
            r#"<div class="field"><span class="field-label">{label}</span><div class="file-row"><label class="btn btn-sm" for="p-{field}">Upload</label><input type="file" name="{field}" id="p-{field}" accept="image/png,image/jpeg,image/gif"><span class="file-name hint" id="p-{field}-name">{label} is currently {set}. PNG, JPEG or GIF, up to {max_mb} MB</span></div>
<label class="check small"><input type="checkbox" name="remove_{field}" value="1"><span>Use the bot's own {label_lc}</span></label></div>"#,
            label_lc = label.to_lowercase(),
        )
    };
    let server = escape(&guild_name(&auth, GuildId(guild)));
    let bot_name = sub.credential.bot_name.clone();
    let shown = if nick.is_empty() {
        bot_name.clone()
    } else {
        nick.clone()
    };
    let body = format!(
        r#"<div class="page"><p class="crumb-line"><a href="/servers">Servers</a><span>/</span><span>{server}</span></p>
<div class="page-head"><div class="row">{icon}<div><h1>Customize {name}</h1><p>Changes apply only to {server}. Leave a field empty to keep the bot's own profile.</p></div></div></div>
<div class="split">
<form method="post" action="/subscriptions/{product}/{guild}/profile" enctype="multipart/form-data"><div class="setting"><div class="s-body"><input type="hidden" name="csrf" value="{csrf}">
<div class="row-wrap">{invite}</div>
<div class="field"><label for="p-nick">Name in this server</label><input class="input" id="p-nick" type="text" name="nick" value="{nick}" maxlength="{max_nick}" placeholder="{bot}" autocomplete="off"></div>
<div class="field"><label for="p-bio">About me</label><textarea class="input" id="p-bio" name="bio" maxlength="{max_bio}" placeholder="Shown on the bot's profile">{bio}</textarea><span class="hint" id="p-bio-count">{bio_len} / {max_bio}</span></div>
{avatar_field}{banner_field}
<div class="note">{info}<span>The bot needs the Change Nickname permission in {server}. If Discord refuses a change, the reason is shown here.</span></div></div>
<div class="s-foot"><a class="link" href="/servers">Cancel</a><button class="btn btn-primary btn-sm" type="submit">Apply to Discord</button></div></div></form>
<aside class="buy"><div class="section-head"><h2>Preview</h2><span class="faint small">{server}</span></div>
<div id="pv-profile">{profile}</div>
<p class="hint section-gap">Uploaded images appear after you save.</p>
<div class="card card-pad section"><div class="strong">Good to know</div><ul class="tips"><li>Only members of {server} see these changes.</li><li>Discord limits how often a bot's profile can change. If a save is refused, wait a few minutes.</li><li>Cancelling premium returns the bot to its own profile.</li></ul></div></aside>
</div></div>"#,
        icon = crate::icons::bot_icon(product, 48),
        avatar_field = file_field("avatar", "Picture", flag(avatar_set)),
        banner_field = file_field("banner", "Banner", flag(banner_set)),
        name = escape(&sub.product.name),
        bot = escape(&bot_name),
        invite = invite_button(
            "Invite the premium bot to this server",
            &sub.credential.client_id
        ),
        product = product,
        guild = guild,
        nick = escape(&nick),
        bio = escape(&bio),
        bio_len = bio.chars().count(),
        max_nick = MAX_NICK_CHARS,
        max_bio = MAX_BIO_CHARS,
        info = crate::icons::INFO,
        profile = crate::icons::dc_profile(product, &shown, &bio),
    );
    page_in(
        &auth,
        &state,
        "Customize",
        &body,
        &Nav::Account(AccountTab::Servers),
    )
    .into_response()
}

#[derive(Default)]
struct ProfileForm {
    csrf: String,
    nick: String,
    bio: String,
    avatar: Vec<u8>,
    banner: Vec<u8>,
    remove_avatar: bool,
    remove_banner: bool,
}

async fn read_form(mut multipart: Multipart) -> Option<ProfileForm> {
    let mut form = ProfileForm::default();
    // Une erreur de lecture (corps tronqué ou trop gros) refuse tout le formulaire.
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return None,
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "csrf" => form.csrf = field.text().await.ok()?,
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

fn megabytes(bytes: usize) -> usize {
    bytes / (1024 * 1024)
}

// Fichier envoyé : remplace l'image. Sinon case cochée : retour à l'image par défaut. Sinon : inchangée.
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

fn image_error_text(error: ImageError, max: usize) -> String {
    match error {
        ImageError::Empty => "The file is empty.".to_string(),
        ImageError::TooLarge => format!("The image is larger than {} MB.", megabytes(max)),
        ImageError::UnsupportedType => "Only PNG, JPEG and GIF images are accepted.".to_string(),
    }
}

fn profile_error(error: ProfileError) -> (StatusCode, &'static str, &'static str) {
    match error {
        ProfileError::BadToken => (
            StatusCode::BAD_GATEWAY,
            "The bot cannot be reached",
            "Discord refused the bot's token. Ask the creator to paste a new one.",
        ),
        ProfileError::NotInServer => (
            StatusCode::CONFLICT,
            "The bot is not in this server",
            "Invite the premium bot to this server first (button on the previous page).",
        ),
        ProfileError::Forbidden => (
            StatusCode::FORBIDDEN,
            "Discord refused the change",
            "The bot may lack the Change Nickname permission in this server. Give its role that permission and try again.",
        ),
        ProfileError::Invalid => (
            StatusCode::BAD_REQUEST,
            "Discord refused these values",
            "Check the name, the bio and the images (size and format), then try again.",
        ),
        ProfileError::RateLimited => (
            StatusCode::TOO_MANY_REQUESTS,
            "Too many changes",
            "Discord limits how often a bot profile can change. Try again in a few minutes.",
        ),
        ProfileError::Unavailable => (
            StatusCode::BAD_GATEWAY,
            "Discord did not answer",
            "The change could not be applied right now. Try again in a minute.",
        ),
    }
}

pub(crate) async fn apply(
    State(state): State<WebState>,
    headers: HeaderMap,
    Path((product, guild)): Path<(u64, u64)>,
    multipart: Multipart,
) -> Response {
    let auth = match require(&state, &headers) {
        Ok(a) => a,
        Err(response) => return response,
    };
    let back = format!("/subscriptions/{product}/{guild}");
    let Some(form) = read_form(multipart).await else {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid form",
            "The form could not be read. The files may be too large.",
            &back,
        );
    };
    if let Err(response) = require_csrf(&state, &auth, &form.csrf) {
        return response;
    }
    let sub = match subscribed(&state, &auth, product, guild) {
        Ok(s) => s,
        Err(response) => return response,
    };
    let (Some(nick), Some(bio)) = (
        clean_text(&form.nick, MAX_NICK_CHARS, false),
        clean_text(&form.bio, MAX_BIO_CHARS, true),
    ) else {
        return message_back(
            StatusCode::BAD_REQUEST,
            &state,
            Some(&auth),
            "Invalid text",
            &format!("The name has at most {MAX_NICK_CHARS} characters and the bio {MAX_BIO_CHARS}, without control characters."),
            &back,
        );
    };
    let max_image = state.config.max_image_bytes;
    let images = (
        image_change(&form.avatar, form.remove_avatar, max_image),
        image_change(&form.banner, form.remove_banner, max_image),
    );
    let (avatar, banner) = match images {
        (Ok(avatar), Ok(banner)) => (avatar, banner),
        (Err(error), _) | (_, Err(error)) => {
            return message_back(
                StatusCode::BAD_REQUEST,
                &state,
                Some(&auth),
                "Invalid image",
                &image_error_text(error, max_image),
                &back,
            )
        }
    };
    let Some(token) = open_token(&state, product, Tier::Premium, &sub.credential) else {
        return message_back(
            StatusCode::INTERNAL_SERVER_ERROR,
            &state,
            Some(&auth),
            "Stored token unreadable",
            "The creator's stored token cannot be read. Ask the creator to paste it again.",
            &back,
        );
    };
    let key = format!("profile-{product}-{guild}");
    let wait =
        state
            .limiter
            .check_and_record(&key, (state.clock)(), state.config.deploy_cooldown_secs);
    if let Err(seconds) = wait {
        return message_back(
            StatusCode::TOO_MANY_REQUESTS,
            &state,
            Some(&auth),
            "Slow down",
            &format!("Wait {seconds} more second(s) before changing the profile again."),
            &back,
        );
    }
    let patch = ProfilePatch {
        nick: Some(nick.clone()),
        bio: Some(bio.clone()),
        avatar: avatar.clone(),
        banner: banner.clone(),
    };
    let profiles = state.profiles.clone();
    let applied = tokio::task::spawn_blocking(move || profiles.apply(&token, guild, &patch)).await;
    match applied {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let (status, title, text) = profile_error(error);
            return message_back(status, &state, Some(&auth), title, text, &back);
        }
        Err(_) => return server_error(&state, Some(&auth)),
    }
    let before = sub.profile.as_ref();
    let saved = GuildProfile {
        nick,
        bio,
        avatar_set: avatar.map_or(before.is_some_and(|p| p.avatar_set), |a| a.is_some()),
        banner_set: banner.map_or(before.is_some_and(|p| p.banner_set), |b| b.is_some()),
        updated_at: (state.clock)(),
    };
    let stored = state
        .store
        .lock()
        .map(|s| s.save_guild_profile(GuildId(guild), ProductId(product), &saved));
    if !matches!(stored, Ok(Ok(()))) {
        return server_error(&state, Some(&auth));
    }
    redirect(&back)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_shown_in_whole_megabytes() {
        assert_eq!(megabytes(2 * 1024 * 1024), 2);
        assert_eq!(megabytes(10 * 1024 * 1024), 10);
        assert_eq!(megabytes(1024 * 1024 - 1), 0);
        assert!(image_error_text(ImageError::TooLarge, 10 * 1024 * 1024).contains("10 MB"));
    }

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
    fn the_length_limit_counts_characters_not_bytes() {
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
    fn a_file_wins_over_the_remove_box_and_no_file_without_the_box_changes_nothing() {
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

    #[test]
    fn every_discord_failure_has_its_own_status_and_never_echoes_discord_text() {
        let all = [
            ProfileError::BadToken,
            ProfileError::NotInServer,
            ProfileError::Forbidden,
            ProfileError::Invalid,
            ProfileError::RateLimited,
            ProfileError::Unavailable,
        ];
        let titles: std::collections::HashSet<_> =
            all.iter().map(|e| profile_error(*e).1).collect();
        assert_eq!(titles.len(), all.len());
        assert_eq!(
            profile_error(ProfileError::RateLimited).0,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            profile_error(ProfileError::NotInServer).0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            profile_error(ProfileError::Invalid).0,
            StatusCode::BAD_REQUEST
        );
    }
}
