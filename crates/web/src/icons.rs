// Interface icons (ported from ui-mockup/icons.js) and the bot/account gradient badges.
// Everything here is decoration, never data: the glyph a bot gets and the gradient an account
// gets are both picked deterministically from an id, nothing is invented or stored.
// A small shared icon set: not every icon is used by every page yet.
#![allow(dead_code)]

pub(crate) const GITHUB: &str = r##"<svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8Z"/></svg>"##;

pub(crate) const DISCORD: &str = r##"<svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M19.6 5.2A17.6 17.6 0 0 0 15.3 4l-.5 1.1a16.4 16.4 0 0 0-4.9 0L9.4 4a17.5 17.5 0 0 0-4.3 1.3C2.4 9.4 1.6 13.4 2 17.4a17.8 17.8 0 0 0 5.3 2.7l1.1-1.8a11.5 11.5 0 0 1-1.8-.9l.4-.3a12.6 12.6 0 0 0 10.8 0l.4.3c-.6.4-1.2.7-1.8.9l1.1 1.8a17.7 17.7 0 0 0 5.3-2.7c.5-4.6-.8-8.6-3.2-12.2ZM8.7 15c-1 0-1.9-1-1.9-2.2s.8-2.2 1.9-2.2 1.9 1 1.9 2.2-.8 2.2-1.9 2.2Zm6.6 0c-1 0-1.9-1-1.9-2.2s.8-2.2 1.9-2.2 1.9 1 1.9 2.2-.8 2.2-1.9 2.2Z"/></svg>"##;

pub(crate) const BRANCH: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="5.5" r="1.5"/><path d="M4.5 5v6M11.5 7c0 2.5-2 3.5-7 4"/></svg>"##;

pub(crate) const CHEV: &str = r##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 4 4 4-4 4"/></svg>"##;

pub(crate) const CHECK: &str = r##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m3 8.5 3 3 7-7"/></svg>"##;

pub(crate) const INFO: &str = r##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="8" cy="8" r="6.25"/><path d="M8 7.25v3.5M8 5.25h.01"/></svg>"##;

pub(crate) const ALERT: &str = r##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 2 1.5 13.5h13z"/><path d="M8 6.5v3M8 11.5h.01"/></svg>"##;

pub(crate) const EXTERNAL: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9.5 2.5h4v4M13.5 2.5 7 9M11.5 9.5v3a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h3"/></svg>"##;

pub(crate) const ROCKET: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9.5 2.5c2.5 0 4 1.5 4 4L8 12 4 8z"/><path d="M4 8 2 7.5 4.5 5H7M8 12l.5 2 2.5-2.5V9"/><circle cx="10" cy="6" r="1"/></svg>"##;

pub(crate) const REDO: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M13 3v3.5H9.5"/><path d="M13 6.5A5.5 5.5 0 1 0 13.5 9"/></svg>"##;

pub(crate) const STOP: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3.5" y="3.5" width="9" height="9" rx="1.5"/></svg>"##;

pub(crate) const PLUS: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 3v10M3 8h10"/></svg>"##;

pub(crate) const FILE: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 1.5H4.5A1.5 1.5 0 0 0 3 3v10a1.5 1.5 0 0 0 1.5 1.5h7A1.5 1.5 0 0 0 13 13V5.5z"/><path d="M9 1.5v4h4"/></svg>"##;

pub(crate) const SLASH: &str = r##"<svg class="slash" viewBox="0 0 16 32" aria-hidden="true"><path d="M10.5 6 5.5 26" stroke="currentColor" stroke-width="1" fill="none"/></svg>"##;

pub(crate) const SEARCH: &str = r##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="m10.5 10.5 3.5 3.5"/></svg>"##;

pub(crate) const KEBAB: &str = r##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="3.5" cy="8" r=".75" fill="currentColor"/><circle cx="8" cy="8" r=".75" fill="currentColor"/><circle cx="12.5" cy="8" r=".75" fill="currentColor"/></svg>"##;

pub(crate) const DOWN: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m4 6 4 4 4-4"/></svg>"##;

pub(crate) const COPY: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="5" y="5" width="9" height="9" rx="1.5"/><path d="M11 5V3.5A1.5 1.5 0 0 0 9.5 2h-6A1.5 1.5 0 0 0 2 3.5v6A1.5 1.5 0 0 0 3.5 11H5"/></svg>"##;

pub(crate) const USERS: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="6" cy="5.5" r="2.5"/><path d="M1.5 13.5c0-2.5 2-4 4.5-4s4.5 1.5 4.5 4M11 3.2a2.5 2.5 0 0 1 0 4.6M12.5 9.7c1.3.6 2 1.9 2 3.8"/></svg>"##;

pub(crate) const SERVER: &str = r##"<svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="2" y="2.5" width="12" height="4.5" rx="1"/><rect x="2" y="9" width="12" height="4.5" rx="1"/><path d="M4.5 4.75h.01M4.5 11.25h.01"/></svg>"##;

// Bot glyphs: one drawn path per slot, white on the slot's own gradient (`.ic-<slot>` in app.css).
const SLOTS: [&str; 12] = [
    "lumen",
    "cadence",
    "foghorn",
    "tally",
    "beacon",
    "quartermaster",
    "parley",
    "ledger",
    "ward",
    "scribe",
    "relay",
    "quizmaster",
];

fn glyph_path(slot: &str) -> &'static str {
    match slot {
        "lumen" => {
            r#"<path d="M12 3.5 19 6v5.5c0 4.2-2.9 7.6-7 9-4.1-1.4-7-4.8-7-9V6z"/><path d="m9 12 2.2 2.2L15.5 10"/>"#
        }
        "cadence" => r#"<path d="M4 10.5v3M8 7.5v9M12 4.5v15M16 8v8M20 10.5v3"/>"#,
        "foghorn" => {
            r#"<path d="M4 10v4h3l7 4.5v-13L7 10z"/><path d="M17.5 9.5a3.5 3.5 0 0 1 0 5M20 7a7 7 0 0 1 0 10"/>"#
        }
        "tally" => {
            r#"<path d="M4 20h16"/><path d="M6.5 20v-5M11.5 20v-9M16.5 20V5"/><path d="m14.5 7 2-2 2 2"/>"#
        }
        "beacon" => {
            r#"<path d="M3.5 7A1.5 1.5 0 0 1 5 5.5h14A1.5 1.5 0 0 1 20.5 7v3a2 2 0 0 0 0 4v3a1.5 1.5 0 0 1-1.5 1.5H5A1.5 1.5 0 0 1 3.5 17v-3a2 2 0 0 0 0-4z"/><path d="M14.5 8v1.5M14.5 11.25v1.5M14.5 14.5V16"/>"#
        }
        "quartermaster" => {
            r#"<circle cx="6" cy="6.5" r="1.6"/><circle cx="6" cy="12" r="1.6"/><circle cx="6" cy="17.5" r="1.6"/><path d="M10 6.5h9M10 12h9M10 17.5h5"/>"#
        }
        "parley" => r#"<path d="M4 6.5h11M4 12h16M4 17.5h7" stroke-width="2.6"/>"#,
        "ledger" => {
            r#"<ellipse cx="12" cy="6.5" rx="7" ry="2.5"/><path d="M5 6.5v5c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5v-5M5 11.5v5c0 1.4 3.1 2.5 7 2.5s7-1.1 7-2.5v-5"/>"#
        }
        "ward" => {
            r#"<rect x="5.5" y="10.5" width="13" height="9.5" rx="2"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5M12 14.25v2"/>"#
        }
        "scribe" => {
            r#"<path d="M5 19l1-4L16.5 4.5a2 2 0 0 1 3 3L9 18z"/><path d="m14.5 6.5 3 3M4 21h16"/>"#
        }
        "relay" => {
            r#"<circle cx="5.5" cy="12" r="2"/><path d="M7.5 12H19M7.5 12 18.5 5.5M7.5 12l11 6.5"/>"#
        }
        "quizmaster" => {
            r#"<path d="M4.5 5.5h15v10h-9L6 19.5v-4H4.5z"/><path d="M10 9a2 2 0 1 1 2.8 1.8c-.5.2-.8.6-.8 1.1M12 13.75h.01"/>"#
        }
        _ => r#"<circle cx="12" cy="12" r="6.5"/><path d="M9.5 12h5"/>"#,
    }
}

fn hash(seed: u64) -> u64 {
    // Mélange grossier mais stable : seul le rang (mod N) compte, jamais la valeur elle-même.
    seed.wrapping_mul(2_654_435_761).wrapping_add(seed >> 7)
}

// Une icône de bot : glyphe dessiné sur son dégradé, choisi par l'identifiant du bot (jamais son nom).
pub(crate) fn bot_icon(id: u64, size: u32) -> String {
    let slot = SLOTS[(hash(id) % SLOTS.len() as u64) as usize];
    let class = if size == 40 {
        format!("ic ic-{slot}")
    } else {
        format!("ic ic-{size} ic-{slot}")
    };
    format!(
        r#"<span class="{class}" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{}</svg></span>"#,
        glyph_path(slot)
    )
}

// Le nom de la pastille d'un bot (sa classe `ic-<nom>` donne aussi le dégradé de la bannière).
pub(crate) fn bot_slot(id: u64) -> &'static str {
    SLOTS[(hash(id) % SLOTS.len() as u64) as usize]
}

// Le profil Discord d'un bot, comme un membre le voit (palette Discord propre, jamais celle du site).
pub(crate) fn dc_profile(id: u64, name: &str, bio: &str) -> String {
    let user: String = name
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<div class="dc-profile"><div class="banner ic-{slot}"></div><div class="pf-body"><div class="pf-av">{icon}<span class="on"></span></div><div class="pf-name"><span>{name}</span><span class="dc-app">APP</span></div><div class="pf-user">{user}</div><div class="pf-box"><h5>About me</h5><div class="pf-bio">{bio}</div></div></div></div>"#,
        slot = bot_slot(id),
        icon = bot_icon(id, 84),
        name = crate::html::escape(name),
        user = crate::html::escape(&user),
        bio = crate::html::escape(bio),
    )
}

// Un badge de compte (connecté, pas une icône de bot) : juste un dégradé par identifiant.
pub(crate) fn account_badge(seed: u64, size: &str) -> String {
    let n = hash(seed) % 4;
    format!(r#"<span class="me {size} a{n}" aria-hidden="true"></span>"#)
}
