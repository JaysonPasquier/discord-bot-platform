use std::net::SocketAddr;

use crate::webconfig::web_settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    pub name: &'static str,
    pub detail: String,
}

pub struct Facts {
    pub docker_version: Option<String>,
    pub image_present: Option<bool>,
    pub port_free: bool,
    // Ce qui empêche de lire la clé privée de l'application GitHub (jamais la clé elle-même).
    pub github_key_problem: Option<String>,
}

// Les secrets ne sont jamais affichés : seuls leur présence et les identifiants publics le sont.
pub fn doctor_checks(
    get: &dyn Fn(&str) -> Option<String>,
    addr: &SocketAddr,
    facts: &Facts,
) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut add = |level: Level, name: &'static str, detail: String| {
        checks.push(Check {
            level,
            name,
            detail,
        });
    };

    match &facts.docker_version {
        Some(version) => add(Level::Ok, "docker", format!("engine {version} is running")),
        None => add(
            Level::Fail,
            "docker",
            "Docker Desktop is not running, or the `docker` command was not found".to_string(),
        ),
    }
    let settings = match web_settings(get, addr) {
        Ok(settings) => settings,
        Err(error) => {
            add(Level::Fail, "settings", error.to_string());
            return checks;
        }
    };
    add(
        Level::Ok,
        "settings",
        "the environment variables are valid".to_string(),
    );

    let image = &settings.config.bot_image;
    match facts.image_present {
        Some(true) => add(Level::Ok, "image", format!("{image} is built")),
        Some(false) => add(
            Level::Warn,
            "image",
            format!("{image} is not built yet (scripts\\dashboard.ps1 builds it)"),
        ),
        None => {}
    }
    if facts.port_free {
        add(Level::Ok, "port", format!("{addr} is free"));
    } else {
        add(
            Level::Fail,
            "port",
            format!("{addr} is already in use: another copy of the dashboard may be running"),
        );
    }

    match (&settings.oauth, settings.config.dev_login) {
        (Some((id, _)), dev) => {
            let note = if dev { " (dev login is also on)" } else { "" };
            add(Level::Ok, "login", format!("Discord login configured, application id {id}{note}"));
            add(
                Level::Ok,
                "redirect",
                format!(
                    "register exactly this redirect in the Discord portal: {}/auth/callback",
                    settings.config.public_url
                ),
            );
        }
        (None, true) => add(
            Level::Warn,
            "login",
            "only the dev login (two fake accounts) is available, no Discord login".to_string(),
        ),
        (None, false) => add(
            Level::Fail,
            "login",
            "no way to log in: set DISCORD_OAUTH_CLIENT_ID and DISCORD_OAUTH_CLIENT_SECRET, or use -Dev"
                .to_string(),
        ),
    }

    match (&settings.github, &facts.github_key_problem) {
        (None, _) => add(
            Level::Warn,
            "github",
            "not configured: only the example bot can be deployed (docs/SETUP_GITHUB.md)"
                .to_string(),
        ),
        (Some(_), Some(problem)) => add(
            Level::Fail,
            "github",
            format!("the private key is unusable: {problem}"),
        ),
        (Some(g), None) => {
            add(
                Level::Ok,
                "github",
                format!("app {} ({}) configured", g.app_id, g.slug),
            );
            add(
                Level::Ok,
                "github callback",
                format!(
                    "register exactly this callback URL on the GitHub App: {}/github/callback",
                    settings.config.public_url
                ),
            );
            let hook = settings.config.github_webhook_secret.is_some();
            let (level, text) = match (hook, settings.poll_secs) {
                (false, 0) => (
                    Level::Warn,
                    "no webhook secret and polling is off: pushes will not deploy by themselves"
                        .to_string(),
                ),
                (true, 0) => (
                    Level::Ok,
                    "by webhook only (needs a public address)".to_string(),
                ),
                (false, n) => (
                    Level::Ok,
                    format!("by polling every {n} s (no webhook secret)"),
                ),
                (true, n) => (Level::Ok, format!("by webhook, and by polling every {n} s")),
            };
            add(level, "auto-deploy", text);
        }
    }
    match settings.master_key {
        Some(_) => add(
            Level::Ok,
            "master key",
            "set: bot tokens are stored encrypted (keep a copy of it, without it they cannot be read)"
                .to_string(),
        ),
        None => add(
            Level::Fail,
            "master key",
            "PLATFORM_MASTER_KEY is missing: scripts\\dashboard.ps1 creates one in .env".to_string(),
        ),
    }
    checks
}

pub fn has_failure(checks: &[Check]) -> bool {
    checks.iter().any(|c| c.level == Level::Fail)
}

pub fn render(checks: &[Check]) -> String {
    checks
        .iter()
        .map(|c| {
            let tag = match c.level {
                Level::Ok => "[ OK ]",
                Level::Warn => "[WARN]",
                Level::Fail => "[FAIL]",
            };
            format!("{tag} {}: {}", c.name, c.detail)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    fn good_vars() -> Vec<(&'static str, &'static str)> {
        vec![
            ("DISCORD_OAUTH_CLIENT_ID", "100"),
            ("DISCORD_OAUTH_CLIENT_SECRET", "oauth-secret-value"),
            ("PLATFORM_MASTER_KEY", KEY),
            ("GITHUB_APP_ID", "12345"),
            ("GITHUB_APP_CLIENT_ID", "Iv1.abc"),
            ("GITHUB_APP_CLIENT_SECRET", "github-client-secret"),
            ("GITHUB_APP_SLUG", "my-platform"),
            ("GITHUB_APP_PRIVATE_KEY_FILE", "app.pem"),
        ]
    }

    fn facts(docker: bool, image: Option<bool>, port_free: bool) -> Facts {
        Facts {
            docker_version: docker.then(|| "29.9.2".to_string()),
            image_present: image,
            port_free,
            github_key_problem: None,
        }
    }

    fn run(vars: &[(&str, &str)], facts: &Facts) -> Vec<Check> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        let Ok(addr) = "127.0.0.1:8080".parse::<SocketAddr>() else {
            panic!("bad address");
        };
        doctor_checks(&|name| map.get(name).cloned(), &addr, facts)
    }

    fn levels(checks: &[Check], name: &str) -> Vec<Level> {
        checks
            .iter()
            .filter(|c| c.name == name)
            .map(|c| c.level)
            .collect()
    }

    fn text(checks: &[Check], name: &str) -> String {
        checks
            .iter()
            .filter(|c| c.name == name)
            .map(|c| c.detail.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    }

    #[test]
    fn a_complete_setup_has_no_warning_and_no_failure() {
        let checks = run(&good_vars(), &facts(true, Some(true), true));
        assert!(
            checks.iter().all(|c| c.level == Level::Ok),
            "{}",
            render(&checks)
        );
        assert!(!has_failure(&checks));
        assert!(text(&checks, "docker").contains("29.9.2"));
        assert!(text(&checks, "image").contains("platform-discord-bot:dev is built"));
        assert!(text(&checks, "login").contains("application id 100"));
        assert!(text(&checks, "redirect").ends_with("http://127.0.0.1:8080/auth/callback"));
    }

    #[test]
    fn a_stopped_docker_is_a_failure_but_the_other_checks_still_run() {
        let checks = run(&good_vars(), &facts(false, None, true));
        assert_eq!(levels(&checks, "docker"), [Level::Fail]);
        assert_eq!(levels(&checks, "login"), [Level::Ok]);
        assert!(levels(&checks, "image").is_empty());
        assert!(has_failure(&checks));
    }

    #[test]
    fn invalid_settings_stop_the_diagnostic_early() {
        let checks = run(
            &[("DISCORD_OAUTH_CLIENT_ID", "100")],
            &facts(true, Some(true), true),
        );
        assert_eq!(levels(&checks, "settings"), [Level::Fail]);
        assert!(levels(&checks, "port").is_empty() && levels(&checks, "master key").is_empty());
        assert!(text(&checks, "settings").contains("DISCORD_OAUTH_CLIENT_SECRET"));
    }

    #[test]
    fn a_busy_port_is_a_failure() {
        let checks = run(&good_vars(), &facts(true, Some(true), false));
        assert_eq!(levels(&checks, "port"), [Level::Fail]);
        assert!(text(&checks, "port").contains("already in use"));
    }

    #[test]
    fn a_missing_image_is_only_a_warning() {
        let checks = run(&good_vars(), &facts(true, Some(false), true));
        assert_eq!(levels(&checks, "image"), [Level::Warn]);
        assert!(!has_failure(&checks));
    }

    #[test]
    fn the_login_check_follows_what_is_configured() {
        let no_login = run(&[], &facts(true, Some(true), true));
        assert_eq!(levels(&no_login, "login"), [Level::Fail]);
        let dev = run(
            &[("PLATFORM_DEV_LOGIN", "1")],
            &facts(true, Some(true), true),
        );
        assert_eq!(levels(&dev, "login"), [Level::Warn]);
        assert!(levels(&dev, "redirect").is_empty());
        let mut both = good_vars();
        both.push(("PLATFORM_DEV_LOGIN", "1"));
        let both = run(&both, &facts(true, Some(true), true));
        assert!(text(&both, "login").contains("dev login is also on"));
    }

    #[test]
    fn github_is_optional_but_the_diagnostic_says_what_it_unlocks() {
        let vars: Vec<_> = good_vars()
            .into_iter()
            .filter(|(k, _)| !k.starts_with("GITHUB"))
            .collect();
        let checks = run(&vars, &facts(true, Some(true), true));
        assert_eq!(levels(&checks, "github"), [Level::Warn]);
        assert!(text(&checks, "github").contains("SETUP_GITHUB"));
        assert!(!has_failure(&checks));
        assert!(levels(&checks, "auto-deploy").is_empty());
    }

    #[test]
    fn a_configured_github_shows_the_callback_and_how_pushes_are_noticed() {
        let checks = run(&good_vars(), &facts(true, Some(true), true));
        assert_eq!(levels(&checks, "github"), [Level::Ok]);
        assert!(
            text(&checks, "github").contains("12345")
                && text(&checks, "github").contains("my-platform")
        );
        assert!(text(&checks, "github callback").ends_with("http://127.0.0.1:8080/github/callback"));
        assert_eq!(
            text(&checks, "auto-deploy"),
            "by polling every 30 s (no webhook secret)"
        );
        let with = |extra: (&'static str, &'static str)| {
            let mut vars = good_vars();
            vars.push(extra);
            run(&vars, &facts(true, Some(true), true))
        };
        let hook = |c: &[Check]| text(c, "auto-deploy");
        let mut vars = good_vars();
        vars.push(("GITHUB_WEBHOOK_SECRET", "twelve-chars"));
        let both = run(&vars, &facts(true, Some(true), true));
        assert_eq!(hook(&both), "by webhook, and by polling every 30 s");
        vars.push(("PLATFORM_POLL_SECS", "0"));
        let only_hook = run(&vars, &facts(true, Some(true), true));
        assert_eq!(hook(&only_hook), "by webhook only (needs a public address)");
        let neither = with(("PLATFORM_POLL_SECS", "0"));
        assert_eq!(levels(&neither, "auto-deploy"), [Level::Warn]);
    }

    #[test]
    fn an_unreadable_github_key_is_a_failure_without_printing_anything_secret() {
        let mut f = facts(true, Some(true), true);
        f.github_key_problem = Some("cannot read the file app.pem".to_string());
        let checks = run(&good_vars(), &f);
        assert_eq!(levels(&checks, "github"), [Level::Fail]);
        assert!(text(&checks, "github").contains("app.pem"));
        assert!(has_failure(&checks));
        assert!(!render(&checks).contains("github-client-secret"));
    }

    #[test]
    fn a_missing_master_key_is_a_failure_with_the_fix() {
        let vars: Vec<_> = good_vars()
            .into_iter()
            .filter(|(k, _)| *k != "PLATFORM_MASTER_KEY")
            .collect();
        let checks = run(&vars, &facts(true, Some(true), true));
        assert_eq!(levels(&checks, "master key"), [Level::Fail]);
        assert!(text(&checks, "master key").contains("dashboard.ps1"));
        assert!(has_failure(&checks));
        let good = run(&good_vars(), &facts(true, Some(true), true));
        assert_eq!(levels(&good, "master key"), [Level::Ok]);
    }

    #[test]
    fn a_malformed_master_key_stops_the_diagnostic_naming_the_variable() {
        let mut vars = good_vars();
        vars.retain(|(k, _)| *k != "PLATFORM_MASTER_KEY");
        vars.push(("PLATFORM_MASTER_KEY", "too-short"));
        let checks = run(&vars, &facts(true, Some(true), true));
        assert_eq!(levels(&checks, "settings"), [Level::Fail]);
        assert!(text(&checks, "settings").contains("PLATFORM_MASTER_KEY"));
        assert!(!render(&checks).contains("too-short"));
    }

    #[test]
    fn secrets_never_appear_in_the_report() {
        let report = render(&run(&good_vars(), &facts(true, Some(true), true)));
        for secret in [KEY, "oauth-secret-value", "github-client-secret"] {
            assert!(!report.contains(secret), "{secret}");
        }
    }

    #[test]
    fn the_report_has_one_tagged_line_per_check() {
        let checks = vec![
            Check {
                level: Level::Ok,
                name: "a",
                detail: "fine".to_string(),
            },
            Check {
                level: Level::Warn,
                name: "b",
                detail: "hmm".to_string(),
            },
            Check {
                level: Level::Fail,
                name: "c",
                detail: "bad".to_string(),
            },
        ];
        assert_eq!(
            render(&checks),
            "[ OK ] a: fine\n[WARN] b: hmm\n[FAIL] c: bad"
        );
        assert!(has_failure(&checks));
        assert!(!has_failure(&checks[..2]));
        assert_eq!(render(&[]), "");
    }
}
