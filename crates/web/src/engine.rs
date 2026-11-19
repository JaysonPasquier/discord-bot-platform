use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use platform_build::{image_tag, BuildRequest};
use platform_core::ProductId;
use platform_runner::{bot_spec, container_name, tier_name, BotParams, ContainerState, Tier};
use platform_store::{credential_context, generate_token, hash_token, DeployStatus};

use crate::state::WebState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    NoSource,
    Storage,
}

// Un build à la fois par version d'un bot : deux pushes rapprochés se suivent, ils ne se croisent pas.
pub struct Engine {
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    inline: bool,
}

impl Engine {
    // `inline` : le déploiement s'exécute avant le retour de `enqueue` (tests). Sinon, en arrière-plan.
    pub fn new(inline: bool) -> Self {
        Self {
            locks: Mutex::new(HashMap::new()),
            inline,
        }
    }

    fn lock_for(&self, key: &str) -> Arc<Mutex<()>> {
        self.locks.lock().map_or_else(
            |_| Arc::new(Mutex::new(())),
            |mut map| map.entry(key.to_string()).or_default().clone(),
        )
    }
}

pub fn parse_tier(raw: &str) -> Option<Tier> {
    match raw {
        "free" => Some(Tier::Free),
        "premium" => Some(Tier::Premium),
        _ => None,
    }
}

// Met un déploiement en file et le lance. Renvoie son identifiant.
pub fn enqueue(
    state: &WebState,
    product: u64,
    tier: Tier,
    commit: &str,
    branch: &str,
    trigger: &str,
) -> Result<u64, EngineError> {
    let id = {
        let store = state.store.lock().map_err(|_| EngineError::Storage)?;
        if !matches!(
            store.source(ProductId(product), tier_name(tier)),
            Ok(Some(_))
        ) {
            return Err(EngineError::NoSource);
        }
        store
            .create_deployment(
                ProductId(product),
                tier_name(tier),
                commit,
                branch,
                trigger,
                (state.clock)(),
            )
            .map_err(|_| EngineError::Storage)?
    };
    if state.engine.inline {
        run_job(state, id);
    } else {
        let state = state.clone();
        tokio::task::spawn_blocking(move || run_job(&state, id));
    }
    Ok(id)
}

// Au démarrage : ce qui était en cours est perdu avec le processus, on le dit au lieu de laisser tourner un sablier.
pub fn recover_interrupted(state: &WebState) {
    let Ok(store) = state.store.lock() else {
        return;
    };
    let now = (state.clock)();
    for status in [DeployStatus::Queued, DeployStatus::Building] {
        for d in store.deployments_with_status(status).unwrap_or_default() {
            let log = format!("{}\nInterrupted: the platform restarted.", d.log);
            let _ = store.set_deployment_progress(
                d.id,
                DeployStatus::Failed,
                &d.image,
                &log,
                Some(now),
            );
        }
    }
}

// Démarre le container d'une version avec un nouveau jeton d'API ; commun au déploiement d'un dépôt
// et au déploiement de l'image d'exemple.
pub fn launch(
    state: &WebState,
    product: u64,
    tier: Tier,
    image: &str,
    discord_token: &str,
) -> Result<(), ()> {
    let name = container_name(product, tier);
    let token = generate_token().map_err(|_| ())?;
    let (registered, creator_env) = state.store.lock().map_or((false, vec![]), |s| {
        let ok = s
            .replace_container_token(&name, &hash_token(&token), ProductId(product))
            .is_ok();
        let vars = s
            .env_vars_for_launch(product, tier_name(tier))
            .unwrap_or_default();
        (ok, vars)
    });
    if !registered {
        return Err(());
    }
    let mut extra: Vec<String> = vec![
        format!("TIER={}", tier_name(tier)),
        format!("DASHBOARD_URL={}", state.config.public_url),
    ];
    let tier_str = tier_name(tier);
    for (k, v) in &creator_env {
        let val = crate::env_vars_pages::decrypt_env(&state.vault, product, tier_str, k, v);
        extra.push(format!("{k}={val}"));
    }
    let spec = bot_spec(&BotParams {
        product,
        tier,
        image,
        network: "bridge",
        api_url: &state.config.api_url_for_containers,
        api_token: &token,
        discord_token: Some(discord_token),
        extra_env: &extra,
    })
    .map_err(|_| ())?;
    state.containers.deploy(&spec).map_err(|_| ())
}

struct Log {
    text: String,
}

impl Log {
    fn push(&mut self, line: &str) {
        if !self.text.is_empty() {
            self.text.push('\n');
        }
        self.text.push_str(line);
    }
}

fn save(state: &WebState, id: u64, status: DeployStatus, image: &str, log: &Log, done: bool) {
    let finished = done.then(|| (state.clock)());
    if let Ok(store) = state.store.lock() {
        let _ = store.set_deployment_progress(id, status, image, &log.text, finished);
    }
}

fn fail(state: &WebState, id: u64, image: &str, log: &mut Log, why: &str) {
    log.push(why);
    save(state, id, DeployStatus::Failed, image, log, true);
    if let Ok(Ok(Some(j))) = state.store.lock().map(|s| s.deployment(id)) {
        notify(state, j.product.0, &j.tier, false);
    }
}

// Envoie une notification Discord webhook (best-effort, les erreurs sont ignorées).
fn notify(state: &WebState, product: u64, tier: &str, success: bool) {
    let url = match state
        .store
        .lock()
        .ok()
        .and_then(|s| s.notification_webhook(product).ok().flatten())
    {
        Some(u) => u,
        None => return,
    };
    let emoji = if success { "\u{2705}" } else { "\u{274C}" };
    let verb = if success { "succeeded" } else { "failed" };
    let tier_safe = tier.replace('"', "");
    let body = format!(
        r#"{{"content":"{emoji} Deployment **{verb}** for the **{tier_safe}** version."}}"#
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build()
        .into();
    let _ = agent
        .post(url.as_str())
        .header("Content-Type", "application/json")
        .send(body.as_str());
}

// Étapes : construire l'image (sauf si ce commit l'est déjà), démarrer le container, le déclarer
// « en service ». Toute étape qui échoue laisse l'ancien déploiement en service.
fn run_job(state: &WebState, id: u64) {
    let Some(job) = state
        .store
        .lock()
        .ok()
        .and_then(|s| s.deployment(id).ok().flatten())
    else {
        return;
    };
    let mut log = Log {
        text: String::new(),
    };
    let Some(tier) = parse_tier(&job.tier) else {
        return fail(state, id, "", &mut log, "Unknown version.");
    };
    let product = job.product.0;
    let found = state.store.lock().ok().and_then(|s| {
        let source = s.source(job.product, &job.tier).ok().flatten()?;
        let credential = s.credential(job.product, &job.tier).ok()?;
        Some((source, credential))
    });
    let Some((source, credential)) = found else {
        return fail(
            state,
            id,
            "",
            &mut log,
            "This version has no repository any more.",
        );
    };
    let Some(credential) = credential else {
        return fail(
            state,
            id,
            "",
            &mut log,
            "Connect this version's Discord bot first (paste its token).",
        );
    };
    let context = credential_context(job.product, &job.tier);
    let Ok(discord_token) = state.vault.open(&credential.token_sealed, &context) else {
        return fail(
            state,
            id,
            "",
            &mut log,
            "The stored Discord token cannot be read: paste it again.",
        );
    };

    let lock = state.engine.lock_for(&container_name(product, tier));
    let _turn = lock.lock();

    let image = image_tag(product, &job.tier, &job.commit);
    log.push(&format!(
        "Commit {} on {}",
        &job.commit[..job.commit.len().min(7)],
        job.branch
    ));
    save(state, id, DeployStatus::Building, &image, &log, false);
    // trigger="rebuild" ignore l'image existante et reconstruit depuis zéro.
    if job.trigger != "rebuild" && state.containers.image_exists(&image).unwrap_or(false) {
        log.push("This commit was already built: reusing its image.");
    } else {
        let request = BuildRequest {
            installation: source.installation,
            repo: source.repo.clone(),
            sha: job.commit.clone(),
            product,
            tier: job.tier.clone(),
        };
        let mut progress = |line: &str| {
            log.push(line);
            save(state, id, DeployStatus::Building, &image, &log, false);
        };
        match state.builder.build(&request, &mut progress) {
            Ok(out) => {
                log.push("Build finished.");
                log.push(&out.log);
            }
            Err(error) => {
                return fail(
                    state,
                    id,
                    &image,
                    &mut log,
                    &format!("Build failed: {error}"),
                );
            }
        }
    }
    log.push("Starting the container.");
    save(state, id, DeployStatus::Building, &image, &log, false);
    if launch(state, product, tier, &image, &discord_token).is_err() {
        return fail(
            state,
            id,
            &image,
            &mut log,
            "Docker did not start the container.",
        );
    }
    // Vérification d'acceptation : un démarrage Docker réussi ne veut pas dire que le bot tourne
    // (jeton invalide, erreur au lancement). Ne détecte pas un plantage plus tardif, après une
    // connexion réussie à Discord : à améliorer plus tard (vérification admin).
    let container = container_name(product, tier);
    match state.containers.state(&container) {
        Ok(Some(ContainerState::Exited(code))) => {
            let tail = state.containers.logs(&container, 30).unwrap_or_default();
            return fail(
                state,
                id,
                &image,
                &mut log,
                &format!("The container exited immediately (code {code}). Last lines:\n{tail}"),
            );
        }
        Ok(Some(ContainerState::Restarting)) => {
            let tail = state.containers.logs(&container, 30).unwrap_or_default();
            return fail(
                state,
                id,
                &image,
                &mut log,
                &format!("The container is stuck restarting. Last lines:\n{tail}"),
            );
        }
        _ => {}
    }
    log.push("Live.");
    let now = (state.clock)();
    if let Ok(store) = state.store.lock() {
        let _ =
            store.set_deployment_progress(id, DeployStatus::Ready, &image, &log.text, Some(now));
        let _ = store.make_current(id, now);
    }
    notify(state, product, &job.tier, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_always_gets_the_same_lock_and_two_versions_never_share_one() {
        let engine = Engine::new(true);
        let a = engine.lock_for("bot-1-free");
        assert!(Arc::ptr_eq(&a, &engine.lock_for("bot-1-free")));
        assert!(!Arc::ptr_eq(&a, &engine.lock_for("bot-1-premium")));
        assert!(!Arc::ptr_eq(&a, &engine.lock_for("bot-2-free")));
    }

    #[test]
    fn log_lines_are_separated_by_a_single_newline() {
        let mut log = Log {
            text: String::new(),
        };
        log.push("one");
        assert_eq!(log.text, "one");
        log.push("two");
        log.push("three");
        assert_eq!(log.text, "one\ntwo\nthree");
    }

    #[test]
    fn only_the_two_known_versions_parse() {
        assert_eq!(parse_tier("free"), Some(Tier::Free));
        assert_eq!(parse_tier("premium"), Some(Tier::Premium));
        for bad in ["", "Free", "gold", "free ", "premium\n"] {
            assert_eq!(parse_tier(bad), None, "{bad:?}");
        }
    }
}
