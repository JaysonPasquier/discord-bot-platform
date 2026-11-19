use platform_core::{Entitlement, GuildId, PaymentEvent, ProductId, Timestamp};
use platform_store::{generate_token, hash_token, SqliteStore};

use crate::args::Command;
use crate::error::CliError;
use crate::GRACE_SECS;

pub fn execute(store: &SqliteStore, command: &Command, now: Timestamp) -> Result<String, CliError> {
    match command {
        Command::ProductToken { product } => {
            let token = generate_token()?;
            store.add_token(&hash_token(&token), ProductId(*product))?;
            Ok(token)
        }
        Command::Pay {
            guild,
            product,
            days,
        } => {
            let current = load(store, *guild, *product)?;
            let event = current.payment_for_days(now, *days);
            apply(store, current, event, now)
        }
        Command::Cancel { guild, product } => {
            let current = load(store, *guild, *product)?;
            apply(
                store,
                current,
                PaymentEvent::Canceled { effective: now },
                now,
            )
        }
        Command::Dispute { guild, product } => apply(
            store,
            load(store, *guild, *product)?,
            PaymentEvent::Disputed,
            now,
        ),
        Command::DisputeWon { guild, product } => apply(
            store,
            load(store, *guild, *product)?,
            PaymentEvent::DisputeWon,
            now,
        ),
        Command::Status { guild, product } => Ok(describe(&load(store, *guild, *product)?, now)),
        Command::Validate { .. }
        | Command::Serve { .. }
        | Command::Doctor { .. }
        | Command::Deploy(_)
        | Command::RotateVault { .. } => Err(CliError::Usage("not a store command".to_string())),
    }
}

fn load(store: &SqliteStore, guild: u64, product: u64) -> Result<Entitlement, CliError> {
    let (g, p) = (GuildId(guild), ProductId(product));
    Ok(store
        .entitlement(g, p)?
        .unwrap_or_else(|| Entitlement::new(g, p)))
}

fn apply(
    store: &SqliteStore,
    current: Entitlement,
    event: PaymentEvent,
    now: Timestamp,
) -> Result<String, CliError> {
    let updated = current.apply(event);
    store.save_entitlement(&updated)?;
    Ok(describe(&updated, now))
}

fn describe(e: &Entitlement, now: Timestamp) -> String {
    let status = e.status(now, GRACE_SECS);
    let paid_until = if e.paid_until == i64::MIN {
        "never".to_string()
    } else {
        e.paid_until.to_string()
    };
    format!(
        "status={} access={} paid_until={} revoked={} canceled={}",
        status.as_str(),
        status.grants_access(),
        paid_until,
        e.revoked,
        e.canceled
    )
}

pub fn validate_text(text: &str) -> Result<String, CliError> {
    match platform_config::parse(text) {
        Ok(config) => {
            let plans = config.premium.as_ref().map_or(0, |p| p.plans.len());
            Ok(format!("ok: {} ({} plans)", config.name, plans))
        }
        Err(platform_config::ConfigError::Syntax(message)) => Err(CliError::Config(message)),
        Err(platform_config::ConfigError::Invalid(issues)) => {
            let lines: Vec<String> = issues
                .iter()
                .map(|i| format!("{}: {:?}", i.field, i.problem))
                .collect();
            Err(CliError::Config(lines.join("\n")))
        }
    }
}

pub fn validate_file(path: &str) -> Result<String, CliError> {
    validate_text(&std::fs::read_to_string(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Timestamp = 1_700_000_000;

    fn store() -> SqliteStore {
        match SqliteStore::in_memory() {
            Ok(s) => s,
            Err(e) => panic!("cannot open in-memory store: {e}"),
        }
    }

    fn run(s: &SqliteStore, command: Command, now: Timestamp) -> String {
        match execute(s, &command, now) {
            Ok(out) => out,
            Err(e) => panic!("{command:?} failed: {e}"),
        }
    }

    fn pay(s: &SqliteStore, days: u32, now: Timestamp) -> String {
        run(
            s,
            Command::Pay {
                guild: 10,
                product: 1,
                days,
            },
            now,
        )
    }

    fn status(s: &SqliteStore, now: Timestamp) -> String {
        run(
            s,
            Command::Status {
                guild: 10,
                product: 1,
            },
            now,
        )
    }

    #[test]
    fn an_unknown_guild_is_expired_and_never_paid() {
        assert_eq!(
            status(&store(), T),
            "status=expired access=false paid_until=never revoked=false canceled=false"
        );
    }

    #[test]
    fn paying_starts_a_period_of_the_requested_length() {
        let s = store();
        let out = pay(&s, 30, T);
        assert_eq!(
            out,
            format!(
                "status=active access=true paid_until={} revoked=false canceled=false",
                T + 30 * 86_400
            )
        );
        assert_eq!(status(&s, T), out);
    }

    #[test]
    fn a_renewal_extends_from_the_end_of_the_current_period() {
        let s = store();
        pay(&s, 30, T);
        let out = pay(&s, 30, T + 10 * 86_400);
        assert!(
            out.contains(&format!("paid_until={}", T + 60 * 86_400)),
            "{out}"
        );
    }

    #[test]
    fn paying_after_expiry_starts_from_now() {
        let s = store();
        pay(&s, 30, T);
        let later = T + 100 * 86_400;
        let out = pay(&s, 5, later);
        assert!(
            out.contains(&format!("paid_until={}", later + 5 * 86_400)),
            "{out}"
        );
    }

    #[test]
    fn the_end_of_the_period_gives_grace_then_expires() {
        let s = store();
        pay(&s, 1, T);
        let end = T + 86_400;
        assert!(status(&s, end - 1).starts_with("status=active"));
        assert!(status(&s, end).starts_with("status=grace access=true"));
        assert!(status(&s, end + i64::from(GRACE_SECS) - 1).starts_with("status=grace"));
        assert!(status(&s, end + i64::from(GRACE_SECS)).starts_with("status=expired access=false"));
    }

    #[test]
    fn canceling_ends_access_at_once_without_grace() {
        let s = store();
        pay(&s, 30, T);
        let out = run(
            &s,
            Command::Cancel {
                guild: 10,
                product: 1,
            },
            T + 100,
        );
        assert!(out.starts_with("status=expired access=false"), "{out}");
        assert!(out.ends_with("canceled=true"), "{out}");
    }

    #[test]
    fn paying_again_after_a_cancel_reactivates() {
        let s = store();
        pay(&s, 30, T);
        run(
            &s,
            Command::Cancel {
                guild: 10,
                product: 1,
            },
            T + 100,
        );
        let out = pay(&s, 30, T + 200);
        assert!(out.starts_with("status=active"), "{out}");
        assert!(out.ends_with("canceled=false"), "{out}");
    }

    #[test]
    fn a_dispute_revokes_and_only_winning_it_restores_access() {
        let s = store();
        pay(&s, 30, T);
        let disputed = run(
            &s,
            Command::Dispute {
                guild: 10,
                product: 1,
            },
            T,
        );
        assert!(
            disputed.starts_with("status=expired access=false"),
            "{disputed}"
        );
        assert!(pay(&s, 30, T).starts_with("status=expired"));
        let won = run(
            &s,
            Command::DisputeWon {
                guild: 10,
                product: 1,
            },
            T,
        );
        assert!(won.starts_with("status=active access=true"), "{won}");
    }

    #[test]
    fn guilds_and_products_are_independent() {
        let s = store();
        pay(&s, 30, T);
        let other_guild = run(
            &s,
            Command::Status {
                guild: 11,
                product: 1,
            },
            T,
        );
        let other_product = run(
            &s,
            Command::Status {
                guild: 10,
                product: 2,
            },
            T,
        );
        assert!(other_guild.starts_with("status=expired"));
        assert!(other_product.starts_with("status=expired"));
    }

    #[test]
    fn a_product_token_is_registered_hashed() {
        let s = store();
        let token = run(&s, Command::ProductToken { product: 7 }, T);
        assert_eq!(token.len(), 64);
        assert!(matches!(
            s.product_for_token(&hash_token(&token)),
            Ok(Some(ProductId(7)))
        ));
        assert!(matches!(
            s.product_for_token(&hash_token("other")),
            Ok(None)
        ));
    }

    #[test]
    fn non_store_commands_are_refused_by_execute() {
        let s = store();
        for c in [
            Command::Validate { file: "x".into() },
            Command::Serve { addr: "x".into() },
            Command::Doctor { addr: "x".into() },
        ] {
            assert!(matches!(execute(&s, &c, T), Err(CliError::Usage(_))));
        }
    }

    const VALID: &str = r#"
schema_version = 1
name = "My Bot"
language = "node"
support_contact = "support@example.com"

[free]
start_command = "node index.js"

[premium]
start_command = "node premium.js"

[[premium.plans]]
id = "pro"
interval = "month"
price_cents = 500
"#;

    #[test]
    fn a_valid_file_is_accepted_with_its_plan_count() {
        assert!(matches!(validate_text(VALID), Ok(s) if s == "ok: My Bot (1 plans)"));
        let free_only = VALID.split("[premium]").next().unwrap_or_default();
        assert!(matches!(validate_text(free_only), Ok(s) if s == "ok: My Bot (0 plans)"));
    }

    #[test]
    fn semantic_problems_are_listed_one_per_line() {
        let bad = VALID
            .replace("price_cents = 500", "price_cents = 1")
            .replace("\"My Bot\"", "\"x\"");
        let Err(CliError::Config(text)) = validate_text(&bad) else {
            panic!("expected a config error");
        };
        assert_eq!(text.lines().count(), 2, "{text}");
        assert!(text.contains("name: InvalidName"), "{text}");
        assert!(text.contains("premium.plans[0]: PriceBelowFloor"), "{text}");
    }

    #[test]
    fn syntax_errors_are_config_errors() {
        assert!(matches!(validate_text("nope ["), Err(CliError::Config(_))));
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        assert!(matches!(
            validate_file("/does/not/exist.toml"),
            Err(CliError::Io(_))
        ));
    }
}
