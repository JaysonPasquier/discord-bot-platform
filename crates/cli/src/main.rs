#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use platform_cli::{
    deploy_spec, doctor_checks, execute, has_failure, parse, render, serve_all, unix_now,
    validate_file, web_settings, CliError, Command, Facts,
};
use std::time::Duration;

use platform_build::{DockerBuilder, ImageBuilder};
use platform_github::{check_private_key, GitHub, GitHubApp, HttpGitHub, NoGitHub};
use platform_runner::Docker;
use platform_store::{parse_key, SqliteStore, Vault};
use platform_stripe::{HttpStripe, NoStripe, Stripe};
use platform_web::{
    poll_once, recover_interrupted, snapshot_all, DeployLimiter, Discord, DockerContainers, Engine,
    HttpBotVerifier, HttpDiscord, HttpGuildProfiles, NoDiscord, WebState,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args).await {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

// Vérifie périodiquement la tête des branches suivies : fait marcher l'auto-déploiement sans adresse publique.
fn spawn_poller(state: WebState, seconds: u32) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(u64::from(seconds)));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let state = state.clone();
            let _ = tokio::task::spawn_blocking(move || poll_once(&state)).await;
        }
    });
}

// Une fois par heure : l'historique des abonnés se remplit même les jours où personne ne regarde.
fn spawn_snapshots(state: WebState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(3600));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let state = state.clone();
            let _ = tokio::task::spawn_blocking(move || snapshot_all(&state)).await;
        }
    });
}

fn docker() -> Docker {
    Docker::new(std::env::var("DOCKER_BIN").unwrap_or_else(|_| "docker".to_string()))
}

fn parse_addr(addr: &str) -> Result<SocketAddr, CliError> {
    addr.parse()
        .map_err(|_| CliError::Usage(format!("--addr {addr:?} must look like HOST:PORT")))
}

fn doctor(addr: &str) -> Result<String, CliError> {
    let socket = parse_addr(addr)?;
    let get = |name: &str| std::env::var(name).ok();
    let image = web_settings(&get, &socket)
        .map(|s| s.config.bot_image)
        .unwrap_or_default();
    let docker = docker();
    let docker_version = docker.server_version().ok();
    let image_present = match (&docker_version, image.is_empty()) {
        (Some(_), false) => docker.image_exists(&image).ok(),
        _ => None,
    };
    let facts = Facts {
        docker_version,
        image_present,
        port_free: std::net::TcpListener::bind(socket).is_ok(),
        github_key_problem: github_key_problem(&get, &socket),
    };
    let checks = doctor_checks(&get, &socket, &facts);
    println!("{}", render(&checks));
    if has_failure(&checks) {
        Err(CliError::Diagnostic)
    } else {
        Ok(String::new())
    }
}

// `None` : pas de GitHub configuré, ou la clé se lit. `Some` : ce qui ne va pas, sans afficher la clé.
fn github_key_problem(get: &dyn Fn(&str) -> Option<String>, socket: &SocketAddr) -> Option<String> {
    let settings = web_settings(get, socket).ok()?;
    let github = settings.github?;
    match std::fs::read_to_string(&github.key_file) {
        Err(_) => Some(format!(
            "cannot read the file {}",
            github.key_file.display()
        )),
        Ok(pem) => check_private_key(&pem)
            .err()
            .map(|_| "the file is not a valid RSA private key (.pem from GitHub)".to_string()),
    }
}

fn github_client(settings: &platform_cli::WebSettings) -> Result<Arc<dyn GitHub>, CliError> {
    let Some(g) = &settings.github else {
        return Ok(Arc::new(NoGitHub));
    };
    let pem = std::fs::read_to_string(&g.key_file).map_err(|e| {
        CliError::Usage(format!(
            "cannot read GITHUB_APP_PRIVATE_KEY_FILE {}: {e}",
            g.key_file.display()
        ))
    })?;
    check_private_key(&pem).map_err(|_| {
        CliError::Usage("GITHUB_APP_PRIVATE_KEY_FILE is not a valid RSA private key".to_string())
    })?;
    Ok(Arc::new(HttpGitHub::new(
        GitHubApp {
            app_id: g.app_id.clone(),
            client_id: g.client_id.clone(),
            client_secret: g.client_secret.expose().to_string(),
            slug: g.slug.clone(),
            private_key_pem: pem,
        },
        Box::new(unix_now),
    )))
}

async fn run(args: &[String]) -> Result<String, CliError> {
    let invocation = parse(args)?;
    match &invocation.command {
        Command::Validate { file } => validate_file(file),
        Command::Doctor { addr } => doctor(addr),
        Command::Serve { addr } => {
            let socket = parse_addr(addr)?;
            let settings = web_settings(&|name| std::env::var(name).ok(), &socket)?;
            let Some(master_key) = settings.master_key else {
                return Err(CliError::MissingSecret("PLATFORM_MASTER_KEY"));
            };
            let path = Path::new(&invocation.db);
            let (api_store, web_store) = (SqliteStore::open(path)?, SqliteStore::open(path)?);
            let discord: Arc<dyn Discord> = match &settings.oauth {
                Some((id, secret)) => Arc::new(HttpDiscord::new(
                    id.clone(),
                    secret.clone(),
                    format!("{}/auth/callback", settings.config.public_url),
                )),
                None => Arc::new(NoDiscord),
            };
            let github = github_client(&settings)?;
            let sdk_dir = settings.sdk_dir.is_dir().then(|| settings.sdk_dir.clone());
            let _ = std::fs::create_dir_all(&settings.work_dir);
            let builder: Arc<dyn ImageBuilder> = Arc::new(DockerBuilder::new(
                github.clone(),
                docker(),
                settings.work_dir.clone(),
                sdk_dir,
                Duration::from_secs(u64::from(settings.build_timeout_secs)),
            ));
            let poll_secs = settings.poll_secs;
            let github_ready = settings.github.is_some();
            let clock: platform_web::Clock = Arc::new(unix_now);
            let dev_login = settings.config.dev_login;
            let stripe: Arc<dyn Stripe> = match &settings.stripe_secret_key {
                Some(key) => Arc::new(HttpStripe::new(key.expose().to_string())),
                None => Arc::new(NoStripe),
            };
            let web = WebState {
                store: Arc::new(Mutex::new(web_store)),
                discord,
                bots: Arc::new(HttpBotVerifier::new()),
                profiles: Arc::new(HttpGuildProfiles::new()),
                github,
                builder,
                engine: Arc::new(Engine::new(false)),
                vault: Arc::new(Vault::new(&master_key)),
                containers: Arc::new(DockerContainers(docker())),
                stripe,
                clock: clock.clone(),
                config: Arc::new(settings.config),
                limiter: Arc::new(DeployLimiter::default()),
                auth_limiter: Arc::new(platform_web::AuthLimiter::default()),
            };
            recover_interrupted(&web);
            if github_ready && poll_secs > 0 {
                spawn_poller(web.clone(), poll_secs);
            }
            spawn_snapshots(web.clone());
            let listener = TcpListener::bind(socket).await?;
            println!("listening on http://{}", listener.local_addr()?);
            if dev_login {
                eprintln!("warning: DEV LOGIN is enabled (no Discord needed). Local use only.");
            }
            serve_all(listener, api_store, web, clock).await?;
            Ok(String::new())
        }
        Command::Deploy(deploy) => {
            let api_token = std::env::var("PLATFORM_API_TOKEN")
                .map_err(|_| CliError::MissingSecret("PLATFORM_API_TOKEN"))?;
            let discord_token = std::env::var("DISCORD_TOKEN").ok();
            let spec = deploy_spec(deploy, &api_token, discord_token.as_deref())?;
            Ok(docker().run(&spec)?)
        }
        Command::RotateVault { new_key } => {
            let master_key = std::env::var("PLATFORM_MASTER_KEY")
                .map_err(|_| CliError::MissingSecret("PLATFORM_MASTER_KEY"))?;
            let old_bytes = parse_key(&master_key).ok_or_else(|| {
                CliError::Usage("PLATFORM_MASTER_KEY must be a 64-character hex string".to_string())
            })?;
            let new_bytes = parse_key(new_key).ok_or_else(|| {
                CliError::Usage("--new-key must be a 64-character hex string".to_string())
            })?;
            let old_vault = Vault::new(&old_bytes);
            let new_vault = Vault::new(&new_bytes);
            let store = SqliteStore::open(Path::new(&invocation.db))?;
            let count = store.rotate_vault_key(&old_vault, &new_vault)?;
            Ok(format!(
                "Rotated {count} credential(s). Update PLATFORM_MASTER_KEY to the new key before restarting."
            ))
        }
        command => {
            let store = SqliteStore::open(Path::new(&invocation.db))?;
            execute(&store, command, unix_now())
        }
    }
}
