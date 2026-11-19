use platform_store::{DeployStatus, Deployment};

use crate::html::{date, escape};
use crate::icons;

pub(crate) fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

pub(crate) fn ago(now: i64, then: i64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        i64::MIN..=-1 => "just now".to_string(),
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => date(then),
    }
}

pub(crate) fn status_label(status: DeployStatus) -> &'static str {
    match status {
        DeployStatus::Queued => "Queued",
        DeployStatus::Building => "Building",
        DeployStatus::Running => "Live",
        DeployStatus::Ready => "Ready",
        DeployStatus::Failed => "Failed",
    }
}

// Mêmes classes que ui-mockup/ : « en file » et « en construction » partagent "working".
pub(crate) fn status_class(status: DeployStatus) -> &'static str {
    match status {
        DeployStatus::Queued | DeployStatus::Building => "working",
        DeployStatus::Running => "live",
        DeployStatus::Ready => "ready",
        DeployStatus::Failed => "failed",
    }
}

// `.status` porte la mise en page, `.state <classe>` la couleur (sélecteur composé dans app.css).
pub(crate) fn status_badge(status: DeployStatus) -> String {
    format!(
        r#"<span class="status state {}"><span class="dot" aria-hidden="true"></span>{}</span>"#,
        status_class(status),
        status_label(status)
    )
}

pub(crate) fn dur(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

// Durée d'un déploiement terminé ; vide tant qu'il tourne.
pub(crate) fn duration(d: &Deployment) -> String {
    match d.finished_at {
        Some(end) if end >= d.created_at => dur(end - d.created_at),
        _ => String::new(),
    }
}

pub(crate) fn tier_title(tier: &str) -> &'static str {
    if tier == "premium" {
        "Premium"
    } else {
        "Free"
    }
}

pub(crate) fn branch_pill(name: &str) -> String {
    format!(
        r#"<span class="branch">{}<span class="truncate">{}</span></span>"#,
        icons::BRANCH,
        escape(name)
    )
}

// Une ligne de la liste des déploiements, comme dans la maquette : identifiant, état, source, date, menu.
pub(crate) fn dep_row(d: &Deployment, csrf: &str, now: i64, user_id: u64) -> String {
    let id = d.product.0;
    let busy = d.status.is_active();
    let current = if d.status == DeployStatus::Running {
        r#"<span class="current">Current</span>"#
    } else {
        ""
    };
    let time = {
        let t = duration(d);
        if t.is_empty() && d.status == DeployStatus::Queued {
            "Waiting".to_string()
        } else {
            t
        }
    };
    format!(
        r#"<div class="dep"><div class="id"><span class="row"><a href="/products/{id}/deployments/{did}">{sha}</a>{current}</span><span class="env">{tier}</span></div>
<div class="st">{state}<span class="t">{time}</span></div>
<div class="src">{branch}<span class="msg">{tier_lc} version</span></div>
<div class="when"><span>{ago}</span><span class="who">{who}{trigger}</span></div>
<details class="menu"><summary class="btn btn-ghost btn-icon btn-xs" aria-label="Actions for {sha}">{kebab}</summary><div class="menu-pop" role="menu"><a href="/products/{id}/deployments/{did}" role="menuitem">Inspect deployment</a><form method="post" action="/products/{id}/deployments/{did}/redeploy"><input type="hidden" name="csrf" value="{csrf}"><button role="menuitem" type="submit"{disabled}>Redeploy this commit</button></form><button role="menuitem" type="button" data-action="copy" data-text="{full}">Copy commit</button></div></details></div>"#,
        did = d.id,
        sha = escape(short(&d.commit)),
        full = escape(&d.commit),
        tier = tier_title(&d.tier),
        tier_lc = escape(&d.tier),
        state = status_badge(d.status),
        branch = branch_pill(&d.branch),
        ago = ago(now, d.created_at),
        who = icons::account_badge(user_id, "sm"),
        trigger = escape(&d.trigger),
        kebab = icons::KEBAB,
        csrf = escape(csrf),
        disabled = if busy { " disabled" } else { "" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_switch_units_at_exact_boundaries() {
        assert_eq!(ago(100, 100), "just now");
        assert_eq!(ago(159, 100), "just now");
        assert_eq!(ago(160, 100), "1 min ago");
        assert_eq!(ago(100 + 3599, 100), "59 min ago");
        assert_eq!(ago(100 + 3600, 100), "1 h ago");
        assert_eq!(ago(100 + 86_399, 100), "23 h ago");
        assert_eq!(ago(100 + 86_400, 100), "1970-01-01");
        assert_eq!(ago(100, 200), "just now");
    }

    #[test]
    fn commits_are_shortened_to_seven_characters() {
        assert_eq!(short("0123456789abcdef"), "0123456");
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(""), "");
    }

    #[test]
    fn each_status_has_its_own_label_and_color_class() {
        let all = [
            (DeployStatus::Queued, "Queued", "working"),
            (DeployStatus::Building, "Building", "working"),
            (DeployStatus::Running, "Live", "live"),
            (DeployStatus::Ready, "Ready", "ready"),
            (DeployStatus::Failed, "Failed", "failed"),
        ];
        for (status, label, class) in all {
            assert_eq!(status_label(status), label);
            assert_eq!(status_class(status), class);
            let badge = status_badge(status);
            assert!(badge.contains(label) && badge.contains(&format!("state {class}")));
        }
    }

    #[test]
    fn the_badge_carries_the_layout_class_and_a_hidden_dot() {
        let badge = status_badge(DeployStatus::Running);
        assert!(badge.contains(r#"class="status state live""#));
        assert!(badge.contains(r#"<span class="dot" aria-hidden="true"></span>"#));
    }
}
