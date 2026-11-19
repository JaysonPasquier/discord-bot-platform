use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use crate::error::GitHubError;
use crate::jwt::app_jwt;
use crate::types::{valid_branch, valid_repo, valid_sha, Branch, Installation, Repo};

const API: &str = "https://api.github.com";
const WEB: &str = "https://github.com";
const USER_AGENT: &str = "bot-platform (https://github.com/your-org/bot-platform)";
const API_VERSION: &str = "2022-11-28";
const MAX_ARCHIVE_BYTES: u64 = 100 * 1024 * 1024;
// Un jeton d'installation vit une heure : on en redemande un avant la fin.
const TOKEN_CACHE_SECS: i64 = 50 * 60;

// Jamais de `Debug` : la clé privée et le secret client ne doivent pas finir dans un journal.
pub struct GitHubApp {
    pub app_id: String,
    pub client_id: String,
    pub client_secret: String,
    pub slug: String,
    pub private_key_pem: String,
}

pub trait GitHub: Send + Sync + 'static {
    // Adresse où envoyer l'utilisateur pour autoriser l'application (flux web).
    fn authorize_url(&self, state: &str, redirect: &str) -> Option<String>;
    // Page GitHub où l'utilisateur installe l'application et choisit les dépôts.
    fn install_url(&self) -> Option<String>;
    // Échange le code contre un jeton utilisateur le temps d'une requête (jamais conservé) et
    // liste les installations de cet utilisateur : seul moyen sûr de lier une installation à un compte.
    fn user_installations(
        &self,
        code: &str,
        redirect: &str,
    ) -> Result<Vec<Installation>, GitHubError>;
    fn repositories(&self, installation: u64) -> Result<Vec<Repo>, GitHubError>;
    fn branches(&self, installation: u64, repo: &str) -> Result<Vec<Branch>, GitHubError>;
    fn branch_head(
        &self,
        installation: u64,
        repo: &str,
        branch: &str,
    ) -> Result<String, GitHubError>;
    // Archive `.tar.gz` du dépôt à ce commit.
    fn download(&self, installation: u64, repo: &str, sha: &str) -> Result<Vec<u8>, GitHubError>;
}

pub struct NoGitHub;

impl GitHub for NoGitHub {
    fn authorize_url(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    fn install_url(&self) -> Option<String> {
        None
    }
    fn user_installations(&self, _: &str, _: &str) -> Result<Vec<Installation>, GitHubError> {
        Err(GitHubError::NotConfigured)
    }
    fn repositories(&self, _: u64) -> Result<Vec<Repo>, GitHubError> {
        Err(GitHubError::NotConfigured)
    }
    fn branches(&self, _: u64, _: &str) -> Result<Vec<Branch>, GitHubError> {
        Err(GitHubError::NotConfigured)
    }
    fn branch_head(&self, _: u64, _: &str, _: &str) -> Result<String, GitHubError> {
        Err(GitHubError::NotConfigured)
    }
    fn download(&self, _: u64, _: &str, _: &str) -> Result<Vec<u8>, GitHubError> {
        Err(GitHubError::NotConfigured)
    }
}

pub struct HttpGitHub {
    app: GitHubApp,
    api_base: String,
    web_base: String,
    agent: ureq::Agent,
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
    tokens: Mutex<HashMap<u64, (String, i64)>>,
}

impl HttpGitHub {
    pub fn new(app: GitHubApp, clock: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        Self::with_bases(app, API.to_string(), WEB.to_string(), clock)
    }

    // Les adresses sont des paramètres pour tester la forme exacte des requêtes contre un faux serveur.
    pub fn with_bases(
        app: GitHubApp,
        api_base: String,
        web_base: String,
        clock: Box<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            app,
            api_base,
            web_base,
            agent,
            clock,
            tokens: Mutex::new(HashMap::new()),
        }
    }

    fn installation_token(&self, installation: u64) -> Result<String, GitHubError> {
        let now = (self.clock)();
        if let Some((token, until)) = self
            .tokens
            .lock()
            .ok()
            .and_then(|t| t.get(&installation).cloned())
        {
            if now < until {
                return Ok(token);
            }
        }
        let jwt = app_jwt(&self.app.app_id, &self.app.private_key_pem, now)?;
        let mut response = self
            .agent
            .post(format!(
                "{}/app/installations/{installation}/access_tokens",
                self.api_base
            ))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("Authorization", format!("Bearer {jwt}"))
            .send_empty()
            .map_err(|e| GitHubError::Http(e.to_string()))?;
        check(&response)?;
        let body = read_text(&mut response)?;
        let token = serde_json::from_str::<RawToken>(&body)
            .map_err(|_| GitHubError::Parse)?
            .token;
        if let Ok(mut tokens) = self.tokens.lock() {
            tokens.insert(installation, (token.clone(), now + TOKEN_CACHE_SECS));
        }
        Ok(token)
    }

    fn get(
        &self,
        path: &str,
        token: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, GitHubError> {
        let response = self
            .agent
            .get(format!("{}{path}", self.api_base))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", API_VERSION)
            .header("Authorization", format!("Bearer {token}"))
            .call()
            .map_err(|e| GitHubError::Http(e.to_string()))?;
        check(&response)?;
        Ok(response)
    }

    fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        token: &str,
    ) -> Result<T, GitHubError> {
        let mut response = self.get(path, token)?;
        serde_json::from_str(&read_text(&mut response)?).map_err(|_| GitHubError::Parse)
    }
}

fn check(response: &ureq::http::Response<ureq::Body>) -> Result<(), GitHubError> {
    let exhausted = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|v| v.to_str().ok())
        == Some("0");
    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 => Err(GitHubError::Unauthorized),
        403 if exhausted => Err(GitHubError::RateLimited),
        403 => Err(GitHubError::Unauthorized),
        404 => Err(GitHubError::NotFound),
        429 => Err(GitHubError::RateLimited),
        other => Err(GitHubError::Http(format!("unexpected status {other}"))),
    }
}

fn read_text(response: &mut ureq::http::Response<ureq::Body>) -> Result<String, GitHubError> {
    response
        .body_mut()
        .read_to_string()
        .map_err(|e| GitHubError::Http(e.to_string()))
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[derive(Deserialize)]
struct RawToken {
    token: String,
}

#[derive(Deserialize)]
struct RawUserToken {
    access_token: Option<String>,
}

#[derive(Deserialize)]
struct RawInstallations {
    installations: Vec<RawInstallation>,
}

#[derive(Deserialize)]
struct RawInstallation {
    id: u64,
    account: Option<RawAccount>,
    #[serde(default)]
    repository_selection: String,
    suspended_at: Option<String>,
}

#[derive(Deserialize)]
struct RawAccount {
    login: String,
}

#[derive(Deserialize)]
struct RawRepos {
    repositories: Vec<RawRepo>,
}

#[derive(Deserialize)]
struct RawRepo {
    full_name: String,
    default_branch: String,
    private: bool,
    #[serde(default)]
    archived: bool,
}

#[derive(Deserialize)]
struct RawBranch {
    name: String,
    commit: RawCommit,
}

#[derive(Deserialize)]
struct RawCommit {
    sha: String,
}

impl GitHub for HttpGitHub {
    fn authorize_url(&self, state: &str, redirect: &str) -> Option<String> {
        Some(format!(
            "{}/login/oauth/authorize?client_id={}&redirect_uri={}&state={}",
            self.web_base,
            percent_encode(&self.app.client_id),
            percent_encode(redirect),
            percent_encode(state)
        ))
    }

    fn install_url(&self) -> Option<String> {
        Some(format!(
            "{}/apps/{}/installations/new",
            self.web_base,
            percent_encode(&self.app.slug)
        ))
    }

    fn user_installations(
        &self,
        code: &str,
        redirect: &str,
    ) -> Result<Vec<Installation>, GitHubError> {
        let mut response = self
            .agent
            .post(format!("{}/login/oauth/access_token", self.web_base))
            .header("User-Agent", USER_AGENT)
            .header("Accept", "application/json")
            .send_form([
                ("client_id", self.app.client_id.as_str()),
                ("client_secret", self.app.client_secret.as_str()),
                ("code", code),
                ("redirect_uri", redirect),
            ])
            .map_err(|e| GitHubError::Http(e.to_string()))?;
        check(&response)?;
        // GitHub répond 200 avec `{"error": ...}` quand le code est refusé.
        let user_token = serde_json::from_str::<RawUserToken>(&read_text(&mut response)?)
            .map_err(|_| GitHubError::Parse)?
            .access_token
            .ok_or(GitHubError::Unauthorized)?;
        let list: RawInstallations =
            self.get_json("/user/installations?per_page=100", &user_token)?;
        Ok(list
            .installations
            .into_iter()
            .filter(|i| i.suspended_at.is_none())
            .filter_map(|i| {
                Some(Installation {
                    id: i.id,
                    login: i.account?.login,
                    selection: i.repository_selection,
                })
            })
            .collect())
    }

    fn repositories(&self, installation: u64) -> Result<Vec<Repo>, GitHubError> {
        let token = self.installation_token(installation)?;
        let list: RawRepos = self.get_json("/installation/repositories?per_page=100", &token)?;
        Ok(list
            .repositories
            .into_iter()
            .filter(|r| !r.archived && valid_repo(&r.full_name))
            .map(|r| Repo {
                full_name: r.full_name,
                default_branch: r.default_branch,
                private: r.private,
            })
            .collect())
    }

    fn branches(&self, installation: u64, repo: &str) -> Result<Vec<Branch>, GitHubError> {
        if !valid_repo(repo) {
            return Err(GitHubError::InvalidName);
        }
        let token = self.installation_token(installation)?;
        let list: Vec<RawBranch> =
            self.get_json(&format!("/repos/{repo}/branches?per_page=100"), &token)?;
        Ok(list
            .into_iter()
            .filter(|b| valid_branch(&b.name) && valid_sha(&b.commit.sha))
            .map(|b| Branch {
                name: b.name,
                sha: b.commit.sha,
            })
            .collect())
    }

    fn branch_head(
        &self,
        installation: u64,
        repo: &str,
        branch: &str,
    ) -> Result<String, GitHubError> {
        if !valid_repo(repo) || !valid_branch(branch) {
            return Err(GitHubError::InvalidName);
        }
        let token = self.installation_token(installation)?;
        let found: RawBranch =
            self.get_json(&format!("/repos/{repo}/branches/{branch}"), &token)?;
        if valid_sha(&found.commit.sha) {
            Ok(found.commit.sha)
        } else {
            Err(GitHubError::Parse)
        }
    }

    fn download(&self, installation: u64, repo: &str, sha: &str) -> Result<Vec<u8>, GitHubError> {
        if !valid_repo(repo) || !valid_sha(sha) {
            return Err(GitHubError::InvalidName);
        }
        let token = self.installation_token(installation)?;
        // GitHub répond 302 vers codeload : l'en-tête d'autorisation n'est pas renvoyé à l'autre hôte.
        let mut response = self.get(&format!("/repos/{repo}/tarball/{sha}"), &token)?;
        response
            .body_mut()
            .with_config()
            .limit(MAX_ARCHIVE_BYTES)
            .read_to_vec()
            .map_err(|e| match e {
                ureq::Error::BodyExceedsLimit(_) => GitHubError::TooLarge,
                other => GitHubError::Http(other.to_string()),
            })
    }
}

#[cfg(test)]
mod tests {
    use platform_testkit::{fake_routes, Reply};

    use super::*;
    use crate::jwt::tests::make_key;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
    const TOKEN_JSON: &str = r#"{"token":"ghs_installtoken","expires_at":"2030-01-01T00:00:00Z"}"#;

    fn app() -> GitHubApp {
        GitHubApp {
            app_id: "99".to_string(),
            client_id: "Iv1.abc".to_string(),
            client_secret: "client-secret".to_string(),
            slug: "my-app".to_string(),
            private_key_pem: make_key(true),
        }
    }

    fn client(base: &str, now: i64) -> HttpGitHub {
        HttpGitHub::with_bases(
            app(),
            base.to_string(),
            base.to_string(),
            Box::new(move || now),
        )
    }

    fn routes(extra: Vec<(&str, Reply)>) -> Vec<(String, Reply)> {
        let mut all = vec![(
            "POST /app/installations/7/access_tokens".to_string(),
            Reply::json(201, TOKEN_JSON),
        )];
        all.extend(extra.into_iter().map(|(p, r)| (p.to_string(), r)));
        all
    }

    #[test]
    fn the_urls_follow_the_documented_shapes() {
        let c = client("https://github.test", 0);
        assert_eq!(
            c.authorize_url("st", "http://127.0.0.1:8080/github/callback").as_deref(),
            Some("https://github.test/login/oauth/authorize?client_id=Iv1.abc&redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fgithub%2Fcallback&state=st")
        );
        assert_eq!(
            c.install_url().as_deref(),
            Some("https://github.test/apps/my-app/installations/new")
        );
    }

    #[test]
    fn repositories_use_an_installation_token_minted_with_a_signed_jwt() {
        let body = r#"{"total_count":3,"repositories":[
            {"full_name":"alice/bot","default_branch":"main","private":true},
            {"full_name":"alice/old","default_branch":"main","private":false,"archived":true},
            {"full_name":"../evil","default_branch":"main","private":false}]}"#;
        let (base, server) = fake_routes(
            routes(vec![(
                "GET /installation/repositories",
                Reply::json(200, body),
            )]),
            2,
        );
        let repos = client(&base, 1_700_000_000).repositories(7);
        let seen = server.join().unwrap_or_default();
        assert_eq!(
            repos,
            Ok(vec![Repo {
                full_name: "alice/bot".to_string(),
                default_branch: "main".to_string(),
                private: true
            }])
        );
        assert_eq!(seen.len(), 2);
        let mint = seen[0].to_lowercase();
        assert!(
            mint.starts_with("post /app/installations/7/access_tokens "),
            "{mint}"
        );
        assert!(mint.contains("authorization: bearer ey"), "{mint}");
        assert!(mint.contains("x-github-api-version: 2022-11-28"));
        assert!(mint.contains("user-agent: bot-platform"));
        let list = seen[1].to_lowercase();
        assert!(
            list.starts_with("get /installation/repositories?per_page=100 "),
            "{list}"
        );
        assert!(list.contains("authorization: bearer ghs_installtoken"));
        assert!(list.contains("accept: application/vnd.github+json"));
    }

    #[test]
    fn the_installation_token_is_reused_until_it_is_close_to_expiring() {
        let repos = r#"{"total_count":0,"repositories":[]}"#;
        let (base, server) = fake_routes(
            routes(vec![(
                "GET /installation/repositories",
                Reply::json(200, repos),
            )]),
            3,
        );
        let c = client(&base, 1_000);
        assert!(c.repositories(7).is_ok() && c.repositories(7).is_ok());
        let first = server.join().unwrap_or_default();
        assert_eq!(first.iter().filter(|r| r.starts_with("POST ")).count(), 1);

        // Un horloge avancée de plus de 50 minutes force un nouveau jeton.
        let (base, server) = fake_routes(
            routes(vec![(
                "GET /installation/repositories",
                Reply::json(200, repos),
            )]),
            4,
        );
        let late = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(1_000));
        let clock = late.clone();
        let c = HttpGitHub::with_bases(
            app(),
            base.clone(),
            base,
            Box::new(move || clock.load(std::sync::atomic::Ordering::SeqCst)),
        );
        assert!(c.repositories(7).is_ok());
        late.store(1_000 + 50 * 60, std::sync::atomic::Ordering::SeqCst);
        assert!(c.repositories(7).is_ok());
        let seen = server.join().unwrap_or_default();
        assert_eq!(seen.iter().filter(|r| r.starts_with("POST ")).count(), 2);
    }

    #[test]
    fn branches_and_the_head_commit_are_read_for_the_chosen_repository() {
        let list = format!(
            r#"[{{"name":"main","commit":{{"sha":"{SHA}","url":"u"}},"protected":false}},{{"name":"bad name","commit":{{"sha":"{SHA}","url":"u"}},"protected":false}}]"#
        );
        let one = format!(r#"{{"name":"feature/x","commit":{{"sha":"{SHA}","url":"u"}}}}"#);
        let (base, server) = fake_routes(
            routes(vec![
                (
                    "GET /repos/alice/bot/branches?per_page=100",
                    Reply::json(200, &list),
                ),
                (
                    "GET /repos/alice/bot/branches/feature/x",
                    Reply::json(200, &one),
                ),
            ]),
            3,
        );
        let c = client(&base, 5);
        assert_eq!(
            c.branches(7, "alice/bot"),
            Ok(vec![Branch {
                name: "main".to_string(),
                sha: SHA.to_string()
            }])
        );
        assert_eq!(
            c.branch_head(7, "alice/bot", "feature/x"),
            Ok(SHA.to_string())
        );
        assert_eq!(server.join().map(|s| s.len()).ok(), Some(3));
    }

    #[test]
    fn names_are_checked_before_any_request_is_made() {
        let c = client("http://127.0.0.1:1", 0);
        assert_eq!(c.branches(7, "../x"), Err(GitHubError::InvalidName));
        assert_eq!(
            c.branch_head(7, "alice/bot", "a..b"),
            Err(GitHubError::InvalidName)
        );
        assert_eq!(
            c.branch_head(7, "bad", "main"),
            Err(GitHubError::InvalidName)
        );
        assert_eq!(
            c.download(7, "alice/bot", "main"),
            Err(GitHubError::InvalidName)
        );
        assert_eq!(c.download(7, "a/b/c", SHA), Err(GitHubError::InvalidName));
    }

    #[test]
    fn the_archive_follows_the_redirect_without_leaking_the_token() {
        let (base, server) = fake_routes(
            routes(vec![
                (
                    &format!("GET /repos/alice/bot/tarball/{SHA}"),
                    Reply::redirect("/codeload/archive"),
                ),
                (
                    "GET /codeload/archive",
                    Reply::bytes(200, b"ARCHIVE-BYTES".to_vec()),
                ),
            ]),
            3,
        );
        let bytes = client(&base, 5).download(7, "alice/bot", SHA);
        let seen = server.join().unwrap_or_default();
        assert_eq!(bytes, Ok(b"ARCHIVE-BYTES".to_vec()));
        assert!(seen[1]
            .to_lowercase()
            .contains("authorization: bearer ghs_installtoken"));
        assert!(
            !seen[2].to_lowercase().contains("authorization"),
            "{}",
            seen[2]
        );
    }

    #[test]
    fn statuses_map_to_the_matching_error() {
        for (status, rate, expected) in [
            (401, "", GitHubError::Unauthorized),
            (403, "", GitHubError::Unauthorized),
            (404, "", GitHubError::NotFound),
            (429, "", GitHubError::RateLimited),
            (
                500,
                "",
                GitHubError::Http("unexpected status 500".to_string()),
            ),
        ] {
            let _ = rate;
            let (base, _) = fake_routes(
                routes(vec![(
                    "GET /installation/repositories",
                    Reply::json(status, "{}"),
                )]),
                2,
            );
            assert_eq!(client(&base, 5).repositories(7), Err(expected), "{status}");
        }
        let mut limited = Reply::json(403, "{}");
        limited
            .headers
            .push(("x-ratelimit-remaining".to_string(), "0".to_string()));
        let (base, _) = fake_routes(routes(vec![("GET /installation/repositories", limited)]), 2);
        assert_eq!(
            client(&base, 5).repositories(7),
            Err(GitHubError::RateLimited)
        );
    }

    #[test]
    fn a_failed_token_request_stops_everything() {
        let (base, _) = fake_routes(
            vec![(
                "POST /app/installations/7/access_tokens".to_string(),
                Reply::json(404, "{}"),
            )],
            1,
        );
        assert_eq!(client(&base, 5).repositories(7), Err(GitHubError::NotFound));
        let (base, _) = fake_routes(
            vec![(
                "POST /app/installations/7/access_tokens".to_string(),
                Reply::json(201, "{}"),
            )],
            1,
        );
        assert_eq!(client(&base, 5).repositories(7), Err(GitHubError::Parse));
    }

    #[test]
    fn a_user_login_lists_only_their_active_installations_and_forgets_the_token() {
        let installs = r#"{"total_count":3,"installations":[
            {"id":7,"account":{"login":"alice"},"repository_selection":"selected"},
            {"id":8,"account":{"login":"org"},"repository_selection":"all","suspended_at":"2026-01-01T00:00:00Z"},
            {"id":9,"repository_selection":"all"}]}"#;
        let (base, server) = fake_routes(
            vec![
                (
                    "POST /login/oauth/access_token".to_string(),
                    Reply::json(
                        200,
                        r#"{"access_token":"ghu_usertoken","token_type":"bearer"}"#,
                    ),
                ),
                (
                    "GET /user/installations".to_string(),
                    Reply::json(200, installs),
                ),
            ],
            2,
        );
        let list =
            client(&base, 5).user_installations("thecode", "http://127.0.0.1:8080/github/callback");
        let seen = server.join().unwrap_or_default();
        assert_eq!(
            list,
            Ok(vec![Installation {
                id: 7,
                login: "alice".to_string(),
                selection: "selected".to_string()
            }])
        );
        let exchange = seen[0].to_lowercase();
        assert!(exchange.contains("accept: application/json"));
        assert!(exchange.contains("client_id=iv1.abc") && exchange.contains("code=thecode"));
        assert!(exchange.contains("client_secret=client-secret"));
        assert!(exchange.contains("redirect_uri=http%3a%2f%2f127.0.0.1%3a8080%2fgithub%2fcallback"));
        assert!(seen[1]
            .to_lowercase()
            .contains("authorization: bearer ghu_usertoken"));
    }

    #[test]
    fn a_refused_code_is_unauthorized() {
        let (base, _) = fake_routes(
            vec![(
                "POST /login/oauth/access_token".to_string(),
                Reply::json(200, r#"{"error":"bad_verification_code"}"#),
            )],
            1,
        );
        assert_eq!(
            client(&base, 5).user_installations("x", "http://r"),
            Err(GitHubError::Unauthorized)
        );
    }

    #[test]
    fn an_unreachable_server_is_an_http_error_and_no_github_means_not_configured() {
        assert!(matches!(
            client("http://127.0.0.1:1", 5).repositories(7),
            Err(GitHubError::Http(_))
        ));
        assert_eq!(NoGitHub.repositories(1), Err(GitHubError::NotConfigured));
        assert_eq!(NoGitHub.authorize_url("s", "r"), None);
        assert_eq!(NoGitHub.install_url(), None);
        assert_eq!(
            NoGitHub.download(1, "a/b", SHA),
            Err(GitHubError::NotConfigured)
        );
    }

    #[test]
    fn the_limits_are_what_the_documentation_says() {
        assert_eq!(MAX_ARCHIVE_BYTES, 104_857_600);
        assert_eq!(TOKEN_CACHE_SECS, 3000);
    }

    #[test]
    fn without_github_every_operation_says_it_is_not_configured() {
        assert_eq!(
            NoGitHub.user_installations("c", "r"),
            Err(GitHubError::NotConfigured)
        );
        assert_eq!(NoGitHub.branches(1, "a/b"), Err(GitHubError::NotConfigured));
        assert_eq!(
            NoGitHub.branch_head(1, "a/b", "main"),
            Err(GitHubError::NotConfigured)
        );
    }

    #[test]
    fn the_percent_encoding_keeps_only_unreserved_characters() {
        assert_eq!(percent_encode("a-b.c_d~e"), "a-b.c_d~e");
        assert_eq!(percent_encode("a b&c=d/é"), "a%20b%26c%3Dd%2F%C3%A9");
    }
}
