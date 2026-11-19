use platform_runner::{bot_spec, BotParams, RunSpec};

use crate::args::Deploy;
use crate::error::CliError;

// Les secrets viennent de l'environnement du processus, jamais de la ligne de commande
// (visible dans la liste des processus).
pub fn deploy_spec(
    deploy: &Deploy,
    api_token: &str,
    discord_token: Option<&str>,
) -> Result<RunSpec, CliError> {
    bot_spec(&BotParams {
        product: deploy.product,
        tier: deploy.tier,
        image: &deploy.image,
        network: &deploy.network,
        api_url: &deploy.api_url,
        api_token,
        discord_token,
        extra_env: &deploy.env,
    })
    .map_err(|e| CliError::Usage(format!("{e:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_runner::Tier;

    fn deploy(tier: Tier, env: &[&str]) -> Deploy {
        Deploy {
            product: 7,
            tier,
            image: "bot:1".to_string(),
            api_url: "http://host:8080".to_string(),
            network: "bridge".to_string(),
            env: env.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn the_arguments_reach_the_spec() {
        let Ok(spec) = deploy_spec(&deploy(Tier::Premium, &["A=1"]), "api-token", Some("d")) else {
            panic!("rejected");
        };
        assert_eq!(spec.name, "bot-7-premium");
        assert_eq!(spec.image, "bot:1");
        assert_eq!(spec.tier, Tier::Premium);
        let names: Vec<&str> = spec.env.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "PLATFORM_API_URL",
                "PLATFORM_API_TOKEN",
                "DISCORD_TOKEN",
                "A"
            ]
        );
    }

    #[test]
    fn malformed_and_reserved_variables_are_usage_errors() {
        for bad in ["NOEQUALS", "DISCORD_TOKEN=x", "PLATFORM_API_TOKEN=x"] {
            let r = deploy_spec(&deploy(Tier::Free, &[bad]), "t", None);
            assert!(matches!(r, Err(CliError::Usage(_))), "{bad}");
        }
    }
}
