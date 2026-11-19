use std::collections::HashSet;

use crate::domain::{check_domain, DomainError};
use crate::model::{BotConfig, Plan};

const SUPPORTED_SCHEMA: u32 = 1;
const MIN_PRICE_CENTS: u32 = 300;
const MAX_PLANS: usize = 5;
const MAX_DOMAINS: usize = 20;
const MAX_COMMAND_LEN: usize = 200;
const MAX_CONTACT_LEN: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    UnsupportedSchema(u32),
    InvalidName,
    InvalidSupportContact,
    InvalidStartCommand,
    TooManyDomains,
    InvalidDomain(DomainError),
    DuplicateDomain,
    NoPlans,
    TooManyPlans,
    InvalidPlanId,
    DuplicatePlanId,
    PriceBelowFloor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub field: String,
    pub problem: Problem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    Syntax(String),
    Invalid(Vec<Issue>),
}

pub fn parse(text: &str) -> Result<BotConfig, ConfigError> {
    let config: BotConfig = toml::from_str(text).map_err(|e| ConfigError::Syntax(e.to_string()))?;
    let issues = validate(&config);
    if issues.is_empty() {
        Ok(config)
    } else {
        Err(ConfigError::Invalid(issues))
    }
}

pub fn validate(config: &BotConfig) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut add = |field: &str, problem: Problem| {
        issues.push(Issue {
            field: field.to_string(),
            problem,
        });
    };

    if config.schema_version != SUPPORTED_SCHEMA {
        add(
            "schema_version",
            Problem::UnsupportedSchema(config.schema_version),
        );
    }
    if !valid_name(&config.name) {
        add("name", Problem::InvalidName);
    }
    let contact = config.support_contact.trim();
    if contact.is_empty() || contact.len() > MAX_CONTACT_LEN {
        add("support_contact", Problem::InvalidSupportContact);
    }
    if !valid_command(&config.free.start_command) {
        add("free.start_command", Problem::InvalidStartCommand);
    }

    if config.allowed_domains.len() > MAX_DOMAINS {
        add("allowed_domains", Problem::TooManyDomains);
    }
    let mut seen_domains = HashSet::new();
    for (i, domain) in config.allowed_domains.iter().enumerate() {
        let field = format!("allowed_domains[{i}]");
        if let Err(e) = check_domain(domain) {
            add(&field, Problem::InvalidDomain(e));
        } else if !seen_domains.insert(domain.as_str()) {
            add(&field, Problem::DuplicateDomain);
        }
    }

    if let Some(premium) = &config.premium {
        if !valid_command(&premium.start_command) {
            add("premium.start_command", Problem::InvalidStartCommand);
        }
        validate_plans(&premium.plans, &mut add);
    }
    issues
}

fn validate_plans(plans: &[Plan], add: &mut impl FnMut(&str, Problem)) {
    if plans.is_empty() {
        add("premium.plans", Problem::NoPlans);
    }
    if plans.len() > MAX_PLANS {
        add("premium.plans", Problem::TooManyPlans);
    }
    let mut seen_ids = HashSet::new();
    for (i, plan) in plans.iter().enumerate() {
        let field = format!("premium.plans[{i}]");
        if !valid_plan_id(&plan.id) {
            add(&field, Problem::InvalidPlanId);
        } else if !seen_ids.insert(plan.id.as_str()) {
            add(&field, Problem::DuplicatePlanId);
        }
        if plan.price_cents < MIN_PRICE_CENTS {
            add(&field, Problem::PriceBelowFloor);
        }
    }
}

pub fn valid_name(name: &str) -> bool {
    let len = name.chars().count();
    (3..=32).contains(&len)
        && name.trim() == name
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

fn valid_plan_id(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_command(command: &str) -> bool {
    !command.trim().is_empty()
        && command.len() <= MAX_COMMAND_LEN
        && !command.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Entrypoint, Interval, Language, Premium};

    fn plan(id: &str, price_cents: u32) -> Plan {
        Plan {
            id: id.to_string(),
            interval: Interval::Month,
            price_cents,
        }
    }

    fn base() -> BotConfig {
        BotConfig {
            schema_version: 1,
            name: "My Bot".to_string(),
            language: Language::Node,
            support_contact: "support@example.com".to_string(),
            allowed_domains: vec![],
            free: Entrypoint {
                start_command: "node index.js".to_string(),
            },
            premium: None,
        }
    }

    fn with_premium(plans: Vec<Plan>) -> BotConfig {
        BotConfig {
            premium: Some(Premium {
                start_command: "node premium.js".to_string(),
                plans,
            }),
            ..base()
        }
    }

    fn problems(config: &BotConfig) -> Vec<(String, Problem)> {
        validate(config)
            .into_iter()
            .map(|i| (i.field, i.problem))
            .collect()
    }

    fn only(field: &str, problem: Problem) -> Vec<(String, Problem)> {
        vec![(field.to_string(), problem)]
    }

    fn none() -> Vec<(String, Problem)> {
        Vec::new()
    }

    #[test]
    fn a_minimal_config_is_valid() {
        assert_eq!(problems(&base()), none());
    }

    #[test]
    fn a_full_config_is_valid() {
        let mut c = with_premium(vec![plan("pro", 500), plan("pro-year", 5000)]);
        c.allowed_domains = vec!["api.example.com".into(), "cdn.example.com".into()];
        assert_eq!(problems(&c), none());
    }

    #[test]
    fn only_schema_version_one_is_supported() {
        for v in [0, 2] {
            let c = BotConfig {
                schema_version: v,
                ..base()
            };
            assert_eq!(
                problems(&c),
                only("schema_version", Problem::UnsupportedSchema(v))
            );
        }
    }

    #[test]
    fn name_length_is_between_3_and_32() {
        for (name, ok) in [
            ("ab", false),
            ("abc", true),
            ("a".repeat(32).as_str(), true),
            ("a".repeat(33).as_str(), false),
        ] {
            let c = BotConfig {
                name: name.to_string(),
                ..base()
            };
            assert_eq!(problems(&c).is_empty(), ok, "{name}");
        }
    }

    #[test]
    fn name_rejects_edge_spaces_and_odd_characters() {
        for bad in [" abc", "abc ", "ab<c>", "bot!", "é-bot"] {
            let c = BotConfig {
                name: bad.to_string(),
                ..base()
            };
            assert_eq!(problems(&c), only("name", Problem::InvalidName), "{bad}");
        }
        for good in ["my-bot_2", "My Bot"] {
            let c = BotConfig {
                name: good.to_string(),
                ..base()
            };
            assert_eq!(problems(&c), none(), "{good}");
        }
    }

    #[test]
    fn name_length_counts_characters_not_bytes() {
        let c = BotConfig {
            name: "ééé".to_string(),
            ..base()
        };
        assert_eq!(problems(&c), only("name", Problem::InvalidName));
    }

    #[test]
    fn support_contact_is_required_and_bounded() {
        for (contact, ok) in [
            ("", false),
            ("   ", false),
            ("x", true),
            ("a".repeat(200).as_str(), true),
            ("a".repeat(201).as_str(), false),
        ] {
            let c = BotConfig {
                support_contact: contact.to_string(),
                ..base()
            };
            assert_eq!(problems(&c).is_empty(), ok, "len {}", contact.len());
        }
    }

    #[test]
    fn start_commands_must_be_plain_and_bounded() {
        for (cmd, ok) in [
            ("", false),
            ("  ", false),
            ("node a.js\nrm -rf /", false),
            ("node\0a.js", false),
            ("a".repeat(200).as_str(), true),
            ("a".repeat(201).as_str(), false),
            ("node a.js --flag", true),
        ] {
            let mut c = base();
            c.free.start_command = cmd.to_string();
            let expected = if ok {
                none()
            } else {
                only("free.start_command", Problem::InvalidStartCommand)
            };
            assert_eq!(problems(&c), expected, "{cmd:?}");
        }
    }

    #[test]
    fn premium_start_command_is_checked_too() {
        let mut c = with_premium(vec![plan("pro", 500)]);
        if let Some(p) = c.premium.as_mut() {
            p.start_command = String::new();
        }
        assert_eq!(
            problems(&c),
            only("premium.start_command", Problem::InvalidStartCommand)
        );
    }

    #[test]
    fn domain_errors_are_reported_with_their_index() {
        let mut c = base();
        c.allowed_domains = vec!["ok.example.com".into(), "*.example.com".into()];
        assert_eq!(
            problems(&c),
            only(
                "allowed_domains[1]",
                Problem::InvalidDomain(DomainError::Wildcard)
            )
        );
    }

    #[test]
    fn duplicate_domains_are_rejected() {
        let mut c = base();
        c.allowed_domains = vec!["a.example.com".into(), "a.example.com".into()];
        assert_eq!(
            problems(&c),
            only("allowed_domains[1]", Problem::DuplicateDomain)
        );
    }

    #[test]
    fn at_most_20_domains() {
        let mut c = base();
        c.allowed_domains = (0..20).map(|i| format!("d{i}.example.com")).collect();
        assert_eq!(problems(&c), none());
        c.allowed_domains.push("d20.example.com".into());
        assert_eq!(
            problems(&c),
            only("allowed_domains", Problem::TooManyDomains)
        );
    }

    #[test]
    fn premium_needs_at_least_one_plan() {
        assert_eq!(
            problems(&with_premium(vec![])),
            only("premium.plans", Problem::NoPlans)
        );
    }

    #[test]
    fn at_most_5_plans() {
        let five: Vec<Plan> = (0..5).map(|i| plan(&format!("p{i}"), 300)).collect();
        assert_eq!(problems(&with_premium(five.clone())), none());
        let mut six = five;
        six.push(plan("p5", 300));
        assert_eq!(
            problems(&with_premium(six)),
            only("premium.plans", Problem::TooManyPlans)
        );
    }

    #[test]
    fn price_floor_is_300_cents() {
        assert_eq!(problems(&with_premium(vec![plan("pro", 300)])), none());
        assert_eq!(
            problems(&with_premium(vec![plan("pro", 299)])),
            only("premium.plans[0]", Problem::PriceBelowFloor)
        );
    }

    #[test]
    fn plan_ids_are_lowercase_slugs_up_to_32() {
        for (id, ok) in [
            ("pro", true),
            ("pro-2", true),
            ("a".repeat(32).as_str(), true),
            ("a".repeat(33).as_str(), false),
            ("", false),
            ("Pro", false),
            ("pro_2", false),
            ("pro 2", false),
        ] {
            let issues = problems(&with_premium(vec![plan(id, 300)]));
            assert_eq!(issues.is_empty(), ok, "{id:?}");
        }
    }

    #[test]
    fn plan_ids_must_be_unique() {
        assert_eq!(
            problems(&with_premium(vec![plan("pro", 300), plan("pro", 400)])),
            only("premium.plans[1]", Problem::DuplicatePlanId)
        );
    }

    #[test]
    fn all_problems_are_reported_together() {
        let mut c = BotConfig {
            schema_version: 9,
            name: "x".to_string(),
            ..base()
        };
        c.allowed_domains = vec!["localhost".into()];
        assert_eq!(problems(&c).len(), 3);
    }

    const VALID_TOML: &str = r#"
schema_version = 1
name = "My Bot"
language = "rust"
support_contact = "https://discord.gg/example"
allowed_domains = ["api.example.com"]

[free]
start_command = "./bot"

[premium]
start_command = "./bot --premium"

[[premium.plans]]
id = "pro"
interval = "month"
price_cents = 500

[[premium.plans]]
id = "pro-year"
interval = "year"
price_cents = 5000
"#;

    #[test]
    fn parses_a_valid_file() {
        let Ok(config) = parse(VALID_TOML) else {
            panic!("a valid file was rejected");
        };
        assert_eq!(config.language, Language::Rust);
        assert_eq!(config.allowed_domains, vec!["api.example.com".to_string()]);
        let plans = config.premium.map(|p| p.plans).unwrap_or_default();
        assert_eq!(plans.len(), 2);
        assert_eq!(plans.get(1).map(|p| p.interval), Some(Interval::Year));
    }

    #[test]
    fn a_missing_required_field_is_a_syntax_error() {
        let text = VALID_TOML.replace("name = \"My Bot\"\n", "");
        assert!(matches!(parse(&text), Err(ConfigError::Syntax(_))));
    }

    #[test]
    fn an_unknown_field_such_as_a_token_is_rejected() {
        let text = format!("discord_token = \"secret\"\n{VALID_TOML}");
        assert!(matches!(parse(&text), Err(ConfigError::Syntax(_))));
    }

    #[test]
    fn an_unknown_language_is_rejected() {
        let text = VALID_TOML.replace("rust", "cobol");
        assert!(matches!(parse(&text), Err(ConfigError::Syntax(_))));
    }

    #[test]
    fn invalid_values_come_back_as_issues() {
        let text = VALID_TOML.replace("price_cents = 500", "price_cents = 100");
        assert_eq!(
            parse(&text),
            Err(ConfigError::Invalid(vec![Issue {
                field: "premium.plans[0]".to_string(),
                problem: Problem::PriceBelowFloor,
            }]))
        );
    }

    #[test]
    fn garbage_is_a_syntax_error() {
        assert!(matches!(parse("not toml ["), Err(ConfigError::Syntax(_))));
    }
}
