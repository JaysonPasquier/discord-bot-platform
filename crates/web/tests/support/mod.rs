#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use platform_build::{image_tag, BuildError, BuildOutput, BuildRequest, ImageBuilder};
use platform_core::{GuildId, ProductId};
use platform_github::{Branch, GitHub, GitHubError, Installation, Repo};
use platform_runner::{ContainerState, RunSpec};
use platform_store::{credential_context, hash_token, BotCredential, SqliteStore, Vault};
use platform_web::{
    router, BotApp, BotVerifier, ContainerError, Containers, Discord, DiscordError, DiscordUser,
    Engine, Guild, GuildProfiles, ProfileError, ProfilePatch, Secret, WebConfig, WebState,
};
use ring::hmac;
use tower::ServiceExt;

pub const START: i64 = 1_700_000_000;
pub const TEST_KEY: [u8; 32] = [9; 32];
pub const WEBHOOK_SECRET: &str = "hook-secret";
pub const STRIPE_WEBHOOK_SECRET: &str = "whsec_test_fake_secret";

// Discord vu par les acheteurs : enregistre chaque modification de profil demandée.
pub struct FakeProfiles {
    pub calls: Mutex<Vec<(String, u64, ProfilePatch)>>,
    pub result: Mutex<Result<(), ProfileError>>,
}

impl Default for FakeProfiles {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Ok(())),
        }
    }
}

impl FakeProfiles {
    pub fn calls(&self) -> Vec<(String, u64, ProfilePatch)> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn fail_with(&self, error: ProfileError) {
        if let Ok(mut result) = self.result.lock() {
            *result = Err(error);
        }
    }
}

impl GuildProfiles for FakeProfiles {
    fn apply(&self, token: &str, guild: u64, patch: &ProfilePatch) -> Result<(), ProfileError> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((token.to_string(), guild, patch.clone()));
        }
        self.result
            .lock()
            .map_or(Err(ProfileError::Unavailable), |r| *r)
    }
}

// Discord vu par les créateurs : seuls les jetons déclarés ici sont acceptés.
#[derive(Default)]
pub struct FakeBots {
    pub known: Mutex<HashMap<String, BotApp>>,
    pub asked: Mutex<Vec<String>>,
    pub down: Mutex<bool>,
    pub counts: Mutex<HashMap<String, u64>>,
    pub count_calls: Mutex<u32>,
}

impl FakeBots {
    pub fn set_guild_count(&self, token: &str, n: u64) {
        if let Ok(mut counts) = self.counts.lock() {
            counts.insert(token.to_string(), n);
        }
    }

    pub fn count_calls(&self) -> u32 {
        self.count_calls.lock().map(|c| *c).unwrap_or(0)
    }

    pub fn know(&self, token: &str, id: &str, name: &str) {
        if let Ok(mut known) = self.known.lock() {
            known.insert(
                token.to_string(),
                BotApp {
                    id: id.to_string(),
                    name: name.to_string(),
                },
            );
        }
    }

    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().map(|a| a.clone()).unwrap_or_default()
    }
}

impl BotVerifier for FakeBots {
    fn verify(&self, token: &str) -> Result<BotApp, DiscordError> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(token.to_string());
        }
        if self.down.lock().is_ok_and(|d| *d) {
            return Err(DiscordError::Http("down".to_string()));
        }
        self.known
            .lock()
            .ok()
            .and_then(|k| k.get(token).cloned())
            .ok_or(DiscordError::Rejected)
    }

    fn guild_count(&self, token: &str) -> Option<u64> {
        if let Ok(mut calls) = self.count_calls.lock() {
            *calls += 1;
        }
        self.counts.lock().ok()?.get(token).copied()
    }
}

pub struct FakeDiscord {
    pub configured: bool,
    pub login: Mutex<Result<(DiscordUser, Vec<Guild>), DiscordError>>,
}

impl Discord for FakeDiscord {
    fn authorize_url(&self, state: &str) -> Option<String> {
        self.configured
            .then(|| format!("https://discord.test/authorize?state={state}"))
    }

    fn login(&self, code: &str) -> Result<(DiscordUser, Vec<Guild>), DiscordError> {
        if code == "good" {
            self.login
                .lock()
                .map_or(Err(DiscordError::Parse), |r| r.clone())
        } else {
            Err(DiscordError::Http("rejected".to_string()))
        }
    }
}

#[derive(Default)]
pub struct FakeContainers {
    pub deployed: Mutex<Vec<RunSpec>>,
    pub stopped: Mutex<Vec<String>>,
    pub states: Mutex<HashMap<String, ContainerState>>,
    pub logs: Mutex<String>,
    pub fail_deploy: Mutex<bool>,
    pub images: Mutex<HashSet<String>>,
}

impl Containers for FakeContainers {
    fn deploy(&self, spec: &RunSpec) -> Result<(), ContainerError> {
        if self.fail_deploy.lock().is_ok_and(|f| *f) {
            return Err(ContainerError);
        }
        if let Ok(mut list) = self.deployed.lock() {
            list.push(spec.clone());
        }
        Ok(())
    }

    fn stop(&self, name: &str) -> Result<(), ContainerError> {
        if let Ok(mut list) = self.stopped.lock() {
            list.push(name.to_string());
        }
        Ok(())
    }

    fn state(&self, name: &str) -> Result<Option<ContainerState>, ContainerError> {
        Ok(self
            .states
            .lock()
            .map_err(|_| ContainerError)?
            .get(name)
            .cloned())
    }

    fn logs(&self, _name: &str, _lines: u32) -> Result<String, ContainerError> {
        Ok(self.logs.lock().map_err(|_| ContainerError)?.clone())
    }

    fn image_exists(&self, image: &str) -> Result<bool, ContainerError> {
        Ok(self
            .images
            .lock()
            .map_err(|_| ContainerError)?
            .contains(image))
    }
}

// GitHub vu par la plateforme : des dépôts et des branches déclarés par le test.
pub struct FakeGitHub {
    pub repos: Mutex<HashMap<u64, Vec<Repo>>>,
    pub branches: Mutex<HashMap<String, Vec<Branch>>>,
    pub installs: Mutex<Result<Vec<Installation>, GitHubError>>,
    pub down: Mutex<bool>,
    pub configured: std::sync::atomic::AtomicBool,
    pub fail: Mutex<Option<GitHubError>>,
    pub redirects: Mutex<Vec<String>>,
}

impl Default for FakeGitHub {
    fn default() -> Self {
        Self {
            repos: Mutex::new(HashMap::new()),
            branches: Mutex::new(HashMap::new()),
            installs: Mutex::new(Ok(vec![])),
            down: Mutex::new(false),
            configured: std::sync::atomic::AtomicBool::new(true),
            fail: Mutex::new(None),
            redirects: Mutex::new(Vec::new()),
        }
    }
}

pub const HEAD_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const HEAD_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

impl FakeGitHub {
    pub fn offer(&self, installation: u64, repo: &str, branches: &[(&str, &str)]) {
        if let Ok(mut repos) = self.repos.lock() {
            repos.entry(installation).or_default().push(Repo {
                full_name: repo.to_string(),
                default_branch: branches.first().map_or("main", |b| b.0).to_string(),
                private: true,
            });
        }
        if let Ok(mut map) = self.branches.lock() {
            map.insert(
                repo.to_string(),
                branches
                    .iter()
                    .map(|(name, sha)| Branch {
                        name: (*name).to_string(),
                        sha: (*sha).to_string(),
                    })
                    .collect(),
            );
        }
    }

    pub fn push(&self, repo: &str, branch: &str, sha: &str) {
        if let Ok(mut map) = self.branches.lock() {
            if let Some(list) = map.get_mut(repo) {
                for b in list.iter_mut().filter(|b| b.name == branch) {
                    b.sha = sha.to_string();
                }
            }
        }
    }

    fn check(&self) -> Result<(), GitHubError> {
        if let Some(error) = self.fail.lock().ok().and_then(|f| f.clone()) {
            return Err(error);
        }
        if self.down.lock().is_ok_and(|d| *d) {
            Err(GitHubError::Http("down".to_string()))
        } else {
            Ok(())
        }
    }
}

impl GitHub for FakeGitHub {
    fn authorize_url(&self, state: &str, redirect: &str) -> Option<String> {
        if state != "probe" {
            if let Ok(mut seen) = self.redirects.lock() {
                seen.push(redirect.to_string());
            }
        }
        self.configured
            .load(Ordering::SeqCst)
            .then(|| format!("https://github.test/authorize?state={state}"))
    }
    fn install_url(&self) -> Option<String> {
        self.configured
            .load(Ordering::SeqCst)
            .then(|| "https://github.test/apps/test/installations/new".to_string())
    }
    fn user_installations(
        &self,
        code: &str,
        redirect: &str,
    ) -> Result<Vec<Installation>, GitHubError> {
        if let Ok(mut seen) = self.redirects.lock() {
            seen.push(redirect.to_string());
        }
        if code != "good" {
            return Err(GitHubError::Unauthorized);
        }
        self.installs
            .lock()
            .map_or(Err(GitHubError::Parse), |i| i.clone())
    }
    fn repositories(&self, installation: u64) -> Result<Vec<Repo>, GitHubError> {
        self.check()?;
        Ok(self
            .repos
            .lock()
            .map_err(|_| GitHubError::Parse)?
            .get(&installation)
            .cloned()
            .unwrap_or_default())
    }
    fn branches(&self, _installation: u64, repo: &str) -> Result<Vec<Branch>, GitHubError> {
        self.check()?;
        self.branches
            .lock()
            .map_err(|_| GitHubError::Parse)?
            .get(repo)
            .cloned()
            .ok_or(GitHubError::NotFound)
    }
    fn branch_head(
        &self,
        installation: u64,
        repo: &str,
        branch: &str,
    ) -> Result<String, GitHubError> {
        self.branches(installation, repo)?
            .into_iter()
            .find(|b| b.name == branch)
            .map(|b| b.sha)
            .ok_or(GitHubError::NotFound)
    }
    fn download(&self, _: u64, _: &str, _: &str) -> Result<Vec<u8>, GitHubError> {
        Err(GitHubError::NotConfigured)
    }
}

// Le constructeur d'images : rien n'est construit, les demandes sont seulement enregistrées.
#[derive(Default)]
pub struct FakeBuilder {
    pub requests: Mutex<Vec<BuildRequest>>,
    pub failure: Mutex<Option<String>>,
}

impl FakeBuilder {
    pub fn requests(&self) -> Vec<BuildRequest> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl ImageBuilder for FakeBuilder {
    fn build(
        &self,
        request: &BuildRequest,
        progress: &mut dyn FnMut(&str),
    ) -> Result<BuildOutput, BuildError> {
        if let Ok(mut list) = self.requests.lock() {
            list.push(request.clone());
        }
        progress("Fake download");
        if let Some(message) = self.failure.lock().ok().and_then(|f| f.clone()) {
            return Err(BuildError::Docker(message));
        }
        Ok(BuildOutput {
            image: image_tag(request.product, &request.tier, &request.sha),
            log: "fake build log".to_string(),
        })
    }
}

// Stripe : la session créée est toujours la même URL prévisible, sauf si `failure` est posé.
#[derive(Default)]
pub struct FakeStripe {
    pub requests: Mutex<Vec<String>>,
    pub failure: Mutex<Option<platform_stripe::StripeError>>,
}

impl platform_stripe::Stripe for FakeStripe {
    fn create_checkout_session(
        &self,
        params: &platform_stripe::CheckoutParams<'_>,
    ) -> Result<platform_stripe::CheckoutSession, platform_stripe::StripeError> {
        if let Ok(mut list) = self.requests.lock() {
            list.push(params.product_name.to_string());
        }
        if let Some(error) = self.failure.lock().ok().and_then(|f| f.clone()) {
            return Err(error);
        }
        Ok(platform_stripe::CheckoutSession {
            id: "cs_test_fake".to_string(),
            url: "https://checkout.stripe.test/session/cs_test_fake".to_string(),
        })
    }
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Resp {
    pub fn location(&self) -> Option<&str> {
        self.headers.get(LOCATION).and_then(|v| v.to_str().ok())
    }

    pub fn set_cookies(&self) -> Vec<String> {
        self.headers
            .get_all(SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect()
    }

    pub fn cookie(&self, name: &str) -> Option<String> {
        self.set_cookies().into_iter().find_map(|c| {
            let first = c.split(';').next()?.to_string();
            let (key, value) = first.split_once('=')?;
            (key == name && !value.is_empty()).then(|| value.to_string())
        })
    }
}

pub struct App {
    pub state: WebState,
    pub router: Router,
    pub store: Arc<Mutex<SqliteStore>>,
    pub containers: Arc<FakeContainers>,
    pub discord: Arc<FakeDiscord>,
    pub bots: Arc<FakeBots>,
    pub profiles: Arc<FakeProfiles>,
    pub github: Arc<FakeGitHub>,
    pub builder: Arc<FakeBuilder>,
    pub stripe: Arc<FakeStripe>,
    pub vault: Arc<Vault>,
    pub now: Arc<AtomicI64>,
}

pub fn user(id: u64, name: &str) -> DiscordUser {
    DiscordUser {
        id,
        name: name.to_string(),
    }
}

pub fn guild(id: &str, name: &str) -> Guild {
    Guild {
        id: id.to_string(),
        name: name.to_string(),
    }
}

impl App {
    pub fn new(dev_login: bool, discord_configured: bool) -> Self {
        Self::with_limits(dev_login, discord_configured, 10, 0)
    }

    pub fn with_limits(
        dev_login: bool,
        discord_configured: bool,
        max_products: u32,
        deploy_cooldown_secs: u32,
    ) -> Self {
        Self::build(
            dev_login,
            discord_configured,
            max_products,
            deploy_cooldown_secs,
            None,
            None,
        )
    }

    // Comme `new`, mais avec `frontend_url` configuré : pour vérifier qu'un chemin propre à l'app
    // React redevient une adresse absolue une fois que l'utilisateur revient de GitHub.
    pub fn with_frontend_url(frontend_url: &str) -> Self {
        Self::build(true, false, 10, 0, Some(frontend_url.to_string()), None)
    }

    // Comme `new`, mais avec `admin_user_id` configuré : pour vérifier `is_admin`.
    pub fn with_admin(admin_user_id: &str) -> Self {
        Self::build(true, false, 10, 0, None, Some(admin_user_id.to_string()))
    }

    fn build(
        dev_login: bool,
        discord_configured: bool,
        max_products: u32,
        deploy_cooldown_secs: u32,
        frontend_url: Option<String>,
        admin_user_id: Option<String>,
    ) -> Self {
        let Ok(store) = SqliteStore::in_memory() else {
            panic!("cannot open in-memory store");
        };
        let store = Arc::new(Mutex::new(store));
        let now = Arc::new(AtomicI64::new(START));
        let clock = now.clone();
        let discord = Arc::new(FakeDiscord {
            configured: discord_configured,
            login: Mutex::new(Ok((user(77, "dana"), vec![guild("500", "Dana's server")]))),
        });
        let containers = Arc::new(FakeContainers::default());
        let bots = Arc::new(FakeBots::default());
        let profiles = Arc::new(FakeProfiles::default());
        let github = Arc::new(FakeGitHub::default());
        let builder = Arc::new(FakeBuilder::default());
        let stripe = Arc::new(FakeStripe::default());
        let vault = Arc::new(Vault::new(&TEST_KEY));
        let config = WebConfig {
            public_url: "http://127.0.0.1:8080".to_string(),
            api_url_for_containers: "http://host.docker.internal:8080".to_string(),
            dev_login,
            bot_image: "bot:test".to_string(),
            subscription_days: 30,
            grace_secs: 259_200,
            max_products,
            max_image_bytes: 2 * 1024 * 1024,
            github_webhook_secret: Some(Secret::new(WEBHOOK_SECRET)),
            deploy_cooldown_secs,
            frontend_url,
            admin_user_id,
            stripe_webhook_secret: Some(Secret::new(STRIPE_WEBHOOK_SECRET)),
            stripe_currency: "usd".to_string(),
            maintenance_banner: None,
        };
        let state = WebState {
            store: store.clone(),
            discord: discord.clone(),
            bots: bots.clone(),
            profiles: profiles.clone(),
            github: github.clone(),
            builder: builder.clone(),
            engine: Arc::new(Engine::new(true)),
            vault: vault.clone(),
            containers: containers.clone(),
            stripe: stripe.clone(),
            clock: Arc::new(move || clock.load(Ordering::SeqCst)),
            config: Arc::new(config),
            limiter: Arc::new(platform_web::DeployLimiter::default()),
            auth_limiter: Arc::new(platform_web::AuthLimiter::default()),
        };
        Self {
            router: router(state.clone()),
            state,
            store,
            containers,
            discord,
            bots,
            profiles,
            github,
            builder,
            stripe,
            vault,
            now,
        }
    }

    // Enregistre directement le bot d'un créateur, comme si le formulaire avait réussi.
    pub fn seed_bot(&self, product: u64, tier: &str, token: &str, client_id: &str) {
        let context = credential_context(ProductId(product), tier);
        let Ok(sealed) = self.vault.seal(token, &context) else {
            panic!("cannot seal");
        };
        let credential = BotCredential {
            client_id: client_id.to_string(),
            bot_name: format!("Bot {client_id}"),
            token_sealed: sealed,
        };
        let saved = self
            .store
            .lock()
            .map(|s| s.save_credential(ProductId(product), tier, &credential));
        assert!(matches!(saved, Ok(Ok(()))), "cannot seed the bot");
    }

    pub fn stored_credential(&self, product: u64, tier: &str) -> Option<BotCredential> {
        self.store
            .lock()
            .ok()
            .and_then(|s| s.credential(ProductId(product), tier).ok().flatten())
    }

    pub async fn send(&self, method: &str, path: &str, cookie: Option<&str>, form: &str) -> Resp {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(c) = cookie {
            request = request.header(COOKIE, c);
        }
        if method == "POST" {
            request = request.header("content-type", "application/x-www-form-urlencoded");
        }
        let Ok(request) = request.body(Body::from(form.to_string())) else {
            panic!("bad request");
        };
        let Ok(response) = self.router.clone().oneshot(request).await;
        let (parts, body) = response.into_parts();
        let Ok(bytes) = to_bytes(body, 1 << 20).await else {
            panic!("cannot read body");
        };
        Resp {
            status: parts.status,
            headers: parts.headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    // Appel de l'API JSON : jeton CSRF en en-tête, corps JSON éventuel.
    pub async fn api(
        &self,
        method: &str,
        path: &str,
        cookie: Option<&str>,
        csrf: Option<&str>,
        json: Option<&str>,
    ) -> Resp {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(c) = cookie {
            request = request.header(COOKIE, c);
        }
        if let Some(t) = csrf {
            request = request.header("x-csrf-token", t);
        }
        if json.is_some() {
            request = request.header("content-type", "application/json");
        }
        let Ok(request) = request.body(Body::from(json.unwrap_or_default().to_string())) else {
            panic!("bad request");
        };
        let Ok(response) = self.router.clone().oneshot(request).await;
        let (parts, body) = response.into_parts();
        let Ok(bytes) = to_bytes(body, 1 << 20).await else {
            panic!("cannot read body");
        };
        Resp {
            status: parts.status,
            headers: parts.headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    // Formulaire multipart de l'API JSON : jeton CSRF en en-tête.
    pub async fn api_multipart(
        &self,
        path: &str,
        cookie: &str,
        csrf: &str,
        parts: &[(&str, Option<&str>, &[u8])],
    ) -> Resp {
        let boundary = "----testboundary7MA4YWxkTrZu0gW";
        let mut body: Vec<u8> = Vec::new();
        for (name, file, content) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match file {
                Some(file) => body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            body.extend_from_slice(content);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let Ok(request) = Request::builder()
            .method("POST")
            .uri(path)
            .header(COOKIE, cookie)
            .header("x-csrf-token", csrf)
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
        else {
            panic!("bad request");
        };
        let Ok(response) = self.router.clone().oneshot(request).await;
        let (parts, body) = response.into_parts();
        let Ok(bytes) = to_bytes(body, 1 << 20).await else {
            panic!("cannot read body");
        };
        Resp {
            status: parts.status,
            headers: parts.headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    // Corps brut (envoi d'une image) : jeton CSRF en en-tête.
    pub async fn api_bytes(&self, path: &str, cookie: &str, csrf: &str, bytes: Vec<u8>) -> Resp {
        let Ok(request) = Request::builder()
            .method("POST")
            .uri(path)
            .header(COOKIE, cookie)
            .header("x-csrf-token", csrf)
            .header("content-type", "application/octet-stream")
            .body(Body::from(bytes))
        else {
            panic!("bad request");
        };
        let Ok(response) = self.router.clone().oneshot(request).await;
        let (parts, body) = response.into_parts();
        let Ok(bytes) = to_bytes(body, 1 << 20).await else {
            panic!("cannot read body");
        };
        Resp {
            status: parts.status,
            headers: parts.headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    // Envoi d'un formulaire `multipart/form-data` : (nom, nom de fichier éventuel, contenu).
    pub async fn post_multipart(
        &self,
        path: &str,
        cookie: Option<&str>,
        parts: &[(&str, Option<&str>, &[u8])],
    ) -> Resp {
        let boundary = "----testboundary7MA4YWxkTrZu0gW";
        let mut body: Vec<u8> = Vec::new();
        for (name, file, content) in parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match file {
                Some(file) => body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                ),
            }
            body.extend_from_slice(content);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let mut request = Request::builder().method("POST").uri(path).header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        );
        if let Some(c) = cookie {
            request = request.header(COOKIE, c);
        }
        let Ok(request) = request.body(Body::from(body)) else {
            panic!("bad request");
        };
        let Ok(response) = self.router.clone().oneshot(request).await;
        let (parts, body) = response.into_parts();
        let Ok(bytes) = to_bytes(body, 1 << 20).await else {
            panic!("cannot read body");
        };
        Resp {
            status: parts.status,
            headers: parts.headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        }
    }

    pub async fn get(&self, path: &str, cookie: Option<&str>) -> Resp {
        self.send("GET", path, cookie, "").await
    }

    pub async fn post(&self, path: &str, cookie: Option<&str>, form: &str) -> Resp {
        self.send("POST", path, cookie, form).await
    }

    // Retourne (en-tête Cookie, jeton CSRF) d'une session de test.
    pub async fn dev_login(&self, who: &str) -> (String, String) {
        let response = self.post("/dev-login", None, &format!("who={who}")).await;
        let Some(value) = response.cookie("session") else {
            panic!("no session cookie: {}", response.status);
        };
        let csrf = self.csrf_of(&value);
        (format!("session={value}"), csrf)
    }

    pub fn csrf_of(&self, cookie_value: &str) -> String {
        let store = self.store.lock();
        let session = store.ok().and_then(|s| {
            s.session(&hash_token(cookie_value), self.now.load(Ordering::SeqCst))
                .ok()
                .flatten()
        });
        match session {
            Some(s) => s.csrf,
            None => panic!("no session for that cookie"),
        }
    }

    // Accepte les CGU pour l'utilisateur associé à ce cookie de session.
    pub fn accept_cgu_for(&self, cookie_value: &str) {
        let now = self.now.load(Ordering::SeqCst);
        let discord_id = {
            let Ok(store) = self.store.lock() else { return };
            store
                .session(&hash_token(cookie_value), now)
                .ok()
                .flatten()
                .map(|s| s.discord_id)
        };
        if let Some(id) = discord_id {
            let Ok(store) = self.store.lock() else { return };
            let _ = store.accept_cgu(id, now);
        }
    }

    pub fn advance(&self, secs: i64) {
        self.now.fetch_add(secs, Ordering::SeqCst);
    }

    pub fn entitlement(&self, guild: u64, product: u64) -> Option<platform_core::Entitlement> {
        self.store.lock().ok().and_then(|s| {
            s.entitlement(GuildId(guild), ProductId(product))
                .ok()
                .flatten()
        })
    }

    pub fn product_for_token(&self, token: &str) -> Option<ProductId> {
        self.store
            .lock()
            .ok()
            .and_then(|s| s.product_for_token(&hash_token(token)).ok().flatten())
    }

    // Poste un événement Stripe signé avec l'horloge du test : comme un vrai webhook.
    pub async fn stripe_webhook(&self, body: &str) -> StatusCode {
        let now = self.now.load(Ordering::SeqCst);
        let signature = stripe_signature(STRIPE_WEBHOOK_SECRET, body, now);
        let Ok(request) = Request::builder()
            .method("POST")
            .uri("/webhooks/stripe")
            .header("stripe-signature", signature)
            .body(Body::from(body.to_string()))
        else {
            panic!("bad request");
        };
        let Ok(response) = self.router.clone().oneshot(request).await;
        response.status()
    }

    // Même chose, sans signature : pour vérifier qu'elle est bien exigée.
    pub async fn stripe_webhook_unsigned(&self, body: &str) -> StatusCode {
        let Ok(request) = Request::builder()
            .method("POST")
            .uri("/webhooks/stripe")
            .body(Body::from(body.to_string()))
        else {
            panic!("bad request");
        };
        let Ok(response) = self.router.clone().oneshot(request).await;
        response.status()
    }
}

pub fn stripe_signature(secret: &str, body: &str, timestamp: i64) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut signed = timestamp.to_string().into_bytes();
    signed.push(b'.');
    signed.extend_from_slice(body.as_bytes());
    let tag = hmac::sign(&key, &signed);
    let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    format!("t={timestamp},v1={hex}")
}

// Construit un faux événement `checkout.session.completed` tel que Stripe l'enverrait, avec la
// métadonnée posée par `subscribe()` à la création de la session. L'identifiant d'événement et du
// PaymentIntent dépendent de `guild`/`product`/l'heure du test (l'horloge avance à chaque appel
// significatif) : deux appels, même pour le même serveur (un réabonnement), ne se percutent jamais
// sur l'idempotence (`stripe_events`) ni sur la correspondance de litige (`stripe_intents`).
pub fn stripe_checkout_completed(
    guild: &str,
    product: u64,
    user: u64,
    days: u32,
    plan: &str,
    cents: u32,
    at: i64,
) -> String {
    stripe_checkout_event(
        "checkout.session.completed",
        "paid",
        guild,
        product,
        user,
        days,
        plan,
        cents,
        at,
    )
}

// Comme `stripe_checkout_completed`, mais pour un moyen de paiement asynchrone : la session se
// termine d'abord `payment_status: "unpaid"` (ignoré), puis `checkout.session.async_payment_succeeded`
// avec `payment_status: "paid"` confirme le paiement plus tard.
pub fn stripe_checkout_unpaid(
    guild: &str,
    product: u64,
    user: u64,
    days: u32,
    plan: &str,
    cents: u32,
    at: i64,
) -> String {
    stripe_checkout_event(
        "checkout.session.completed",
        "unpaid",
        guild,
        product,
        user,
        days,
        plan,
        cents,
        at,
    )
}

pub fn stripe_async_payment_succeeded(
    guild: &str,
    product: u64,
    user: u64,
    days: u32,
    plan: &str,
    cents: u32,
    at: i64,
) -> String {
    stripe_checkout_event(
        "checkout.session.async_payment_succeeded",
        "paid",
        guild,
        product,
        user,
        days,
        plan,
        cents,
        at,
    )
}

// Construit un faux événement de litige (`charge.dispute.created` ou `.closed`). `status` n'a de
// sens que pour `.closed` ("won" lève la révocation, tout autre statut la laisse en place) ; pour
// `.created`, passer n'importe quoi (ignoré par le serveur).
pub fn stripe_dispute_event(kind: &str, payment_intent: &str, status: &str, nonce: &str) -> String {
    format!(
        r#"{{"id":"evt_dispute_{payment_intent}_{nonce}","type":"{kind}","data":{{"object":{{
            "payment_intent":"{payment_intent}","status":"{status}"
        }}}}}}"#
    )
}

#[allow(clippy::too_many_arguments)]
fn stripe_checkout_event(
    kind: &str,
    payment_status: &str,
    guild: &str,
    product: u64,
    user: u64,
    days: u32,
    plan: &str,
    cents: u32,
    at: i64,
) -> String {
    format!(
        r#"{{"id":"evt_{guild}_{product}_{at}_{payment_status}","type":"{kind}","data":{{"object":{{
            "payment_status":"{payment_status}","payment_intent":"pi_{guild}_{product}_{at}","amount_total":{cents},
            "metadata":{{"guild_id":"{guild}","product_id":"{product}","user_id":"{user}","days":"{days}","plan_name":"{plan}"}}
        }}}}}}"#
    )
}

pub fn form_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
