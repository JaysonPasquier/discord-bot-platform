// Ce que l'API renvoie : des structures simples, sans jeton ni secret. Les identifiants Discord
// sont des chaînes (ils dépassent 2^53 : un nombre JavaScript les arrondirait en silence).
use platform_runner::{tier_name, ContainerState};
use platform_store::{Deployment, Source};
use serde::Serialize;

use crate::icons::bot_slot;
use crate::summary::{BotCard, VersionSummary};

#[derive(Serialize)]
pub(crate) struct DeploymentDto {
    pub id: u64,
    pub tier: String,
    pub commit: String,
    pub branch: String,
    pub trigger: String,
    pub status: &'static str,
    pub image: String,
    pub created_at: i64,
    pub finished_at: Option<i64>,
}

impl From<&Deployment> for DeploymentDto {
    fn from(d: &Deployment) -> Self {
        Self {
            id: d.id,
            tier: d.tier.clone(),
            commit: d.commit.clone(),
            branch: d.branch.clone(),
            trigger: d.trigger.clone(),
            status: d.status.as_str(),
            image: d.image.clone(),
            created_at: d.created_at,
            finished_at: d.finished_at,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct VersionSummaryDto {
    pub tier: &'static str,
    pub has_source: bool,
    pub has_credential: bool,
    pub app_name: Option<String>,
    pub latest: Option<DeploymentDto>,
    pub crash_looping: bool,
}

impl From<&VersionSummary> for VersionSummaryDto {
    fn from(v: &VersionSummary) -> Self {
        Self {
            tier: tier_name(v.tier),
            has_source: v.has_source,
            has_credential: v.has_credential,
            app_name: v.app_name.clone(),
            latest: v.latest.as_ref().map(DeploymentDto::from),
            crash_looping: v.crash_looping,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct BotSummaryDto {
    pub id: u64,
    pub name: String,
    pub slot: &'static str,
    pub free: VersionSummaryDto,
    pub premium: VersionSummaryDto,
    pub last_deploy: Option<DeploymentDto>,
    pub premium_subscribers: usize,
}

impl From<&BotCard> for BotSummaryDto {
    fn from(c: &BotCard) -> Self {
        Self {
            id: c.product.id.0,
            name: c.product.name.clone(),
            slot: bot_slot(c.product.id.0),
            free: VersionSummaryDto::from(&c.free),
            premium: VersionSummaryDto::from(&c.premium),
            last_deploy: c.last_deploy.as_ref().map(DeploymentDto::from),
            premium_subscribers: c.premium_subscribers,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct SourceDto {
    pub repo: String,
    pub branch: String,
    pub auto_deploy: bool,
}

impl From<&Source> for SourceDto {
    fn from(s: &Source) -> Self {
        Self {
            repo: s.repo.clone(),
            branch: s.branch.clone(),
            auto_deploy: s.auto_deploy,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct AppDto {
    pub name: String,
    pub client_id: String,
    pub invite_url: String,
}

#[derive(Serialize)]
pub(crate) struct ContainerDto {
    // unreachable (Docker ne répond pas) | none | running | restarting | exited | other
    pub status: &'static str,
    pub exit_code: Option<i32>,
    pub detail: Option<String>,
}

impl ContainerDto {
    pub(crate) fn from_state(state: &Option<Option<ContainerState>>) -> Self {
        let plain = |status| Self {
            status,
            exit_code: None,
            detail: None,
        };
        match state {
            None => plain("unreachable"),
            Some(None) => plain("none"),
            Some(Some(ContainerState::Running)) => plain("running"),
            Some(Some(ContainerState::Restarting)) => plain("restarting"),
            Some(Some(ContainerState::Exited(code))) => Self {
                status: "exited",
                exit_code: Some(*code),
                detail: None,
            },
            Some(Some(ContainerState::Other(text))) => Self {
                status: "other",
                exit_code: None,
                detail: Some(text.clone()),
            },
        }
    }
}

#[derive(Serialize)]
pub(crate) struct VersionDto {
    pub tier: &'static str,
    pub source: Option<SourceDto>,
    pub app: Option<AppDto>,
    pub container: ContainerDto,
    pub logs: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_container_state_has_its_own_status() {
        let cases: [(Option<Option<ContainerState>>, &str); 5] = [
            (None, "unreachable"),
            (Some(None), "none"),
            (Some(Some(ContainerState::Running)), "running"),
            (Some(Some(ContainerState::Restarting)), "restarting"),
            (Some(Some(ContainerState::Exited(3))), "exited"),
        ];
        for (state, status) in cases {
            assert_eq!(ContainerDto::from_state(&state).status, status);
        }
        let exited = ContainerDto::from_state(&Some(Some(ContainerState::Exited(3))));
        assert_eq!(exited.exit_code, Some(3));
        let other = ContainerDto::from_state(&Some(Some(ContainerState::Other("paused".into()))));
        assert_eq!(
            (other.status, other.detail.as_deref()),
            ("other", Some("paused"))
        );
    }
}
