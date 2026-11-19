use crate::spec::{RunSpec, SpecError, Tier};

const RESERVED: [&str; 3] = ["PLATFORM_API_URL", "PLATFORM_API_TOKEN", "DISCORD_TOKEN"];

pub struct BotParams<'a> {
    pub product: u64,
    pub tier: Tier,
    pub image: &'a str,
    pub network: &'a str,
    pub api_url: &'a str,
    pub api_token: &'a str,
    pub discord_token: Option<&'a str>,
    pub extra_env: &'a [String],
}

pub fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Free => "free",
        Tier::Premium => "premium",
    }
}

pub fn container_name(product: u64, tier: Tier) -> String {
    format!("bot-{product}-{}", tier_name(tier))
}

// Un seul volume par produit, pas par container : passer de gratuit à premium (ou l'inverse)
// garde les données et la configuration du bot au même endroit.
pub fn volume_name(product: u64) -> String {
    format!("bot-{product}-data")
}

// Les variables réservées viennent de la plateforme : un créateur ne peut pas les remplacer.
pub fn bot_spec(p: &BotParams<'_>) -> Result<RunSpec, SpecError> {
    let mut env = vec![
        ("PLATFORM_API_URL".to_string(), p.api_url.to_string()),
        ("PLATFORM_API_TOKEN".to_string(), p.api_token.to_string()),
    ];
    if let Some(token) = p.discord_token {
        env.push(("DISCORD_TOKEN".to_string(), token.to_string()));
    }
    for pair in p.extra_env {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| SpecError::MalformedEnvPair(pair.clone()))?;
        if RESERVED.contains(&name) {
            return Err(SpecError::ReservedEnvName(name.to_string()));
        }
        env.push((name.to_string(), value.to_string()));
    }
    Ok(RunSpec {
        name: container_name(p.product, p.tier),
        image: p.image.to_string(),
        tier: p.tier,
        network: p.network.to_string(),
        env,
        command: vec![],
        volume: Some(volume_name(p.product)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params<'a>(tier: Tier, extra: &'a [String], discord: Option<&'a str>) -> BotParams<'a> {
        BotParams {
            product: 7,
            tier,
            image: "bot:1",
            network: "bridge",
            api_url: "http://host:8080",
            api_token: "api-token",
            discord_token: discord,
            extra_env: extra,
        }
    }

    fn spec(p: &BotParams<'_>) -> RunSpec {
        match bot_spec(p) {
            Ok(s) => s,
            Err(e) => panic!("rejected: {e:?}"),
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn names_follow_product_and_tier() {
        assert_eq!(container_name(7, Tier::Free), "bot-7-free");
        assert_eq!(container_name(7, Tier::Premium), "bot-7-premium");
        assert_eq!(
            spec(&params(Tier::Premium, &[], None)).name,
            "bot-7-premium"
        );
    }

    #[test]
    fn the_volume_is_one_per_product_shared_by_both_tiers() {
        assert_eq!(volume_name(7), "bot-7-data");
        assert_eq!(volume_name(8), "bot-8-data");
        assert_eq!(
            spec(&params(Tier::Free, &[], None)).volume.as_deref(),
            Some("bot-7-data")
        );
        assert_eq!(
            spec(&params(Tier::Premium, &[], None)).volume.as_deref(),
            Some("bot-7-data")
        );
    }

    #[test]
    fn tier_image_and_network_are_passed_through() {
        let s = spec(&params(Tier::Premium, &[], None));
        assert_eq!(
            (s.tier, s.image.as_str(), s.network.as_str()),
            (Tier::Premium, "bot:1", "bridge")
        );
        assert!(s.command.is_empty());
    }

    #[test]
    fn the_platform_variables_are_always_set() {
        let s = spec(&params(Tier::Free, &[], None));
        assert_eq!(
            s.env,
            vec![
                (
                    "PLATFORM_API_URL".to_string(),
                    "http://host:8080".to_string()
                ),
                ("PLATFORM_API_TOKEN".to_string(), "api-token".to_string()),
            ]
        );
    }

    #[test]
    fn the_discord_token_is_added_only_when_given() {
        let none = spec(&params(Tier::Free, &[], None));
        assert!(none.env.iter().all(|(n, _)| n != "DISCORD_TOKEN"));
        let some = spec(&params(Tier::Free, &[], Some("d-token")));
        assert!(some
            .env
            .contains(&("DISCORD_TOKEN".to_string(), "d-token".to_string())));
    }

    #[test]
    fn extra_variables_are_split_on_the_first_equals_sign() {
        let extra = strings(&["A=1", "B=x=y", "C="]);
        let s = spec(&params(Tier::Free, &extra, None));
        assert!(s.env.contains(&("A".to_string(), "1".to_string())));
        assert!(s.env.contains(&("B".to_string(), "x=y".to_string())));
        assert!(s.env.contains(&("C".to_string(), String::new())));
    }

    #[test]
    fn malformed_and_reserved_variables_are_rejected() {
        let malformed = strings(&["NOEQUALS"]);
        assert_eq!(
            bot_spec(&params(Tier::Free, &malformed, None)).err(),
            Some(SpecError::MalformedEnvPair("NOEQUALS".to_string()))
        );
        for name in RESERVED {
            let reserved = vec![format!("{name}=x")];
            assert_eq!(
                bot_spec(&params(Tier::Free, &reserved, None)).err(),
                Some(SpecError::ReservedEnvName(name.to_string()))
            );
        }
    }
}
