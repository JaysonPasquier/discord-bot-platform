use platform_runner::Tier;

use crate::error::CliError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Deploy {
    pub product: u64,
    pub tier: Tier,
    pub image: String,
    pub api_url: String,
    pub network: String,
    pub env: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ProductToken { product: u64 },
    Pay { guild: u64, product: u64, days: u32 },
    Cancel { guild: u64, product: u64 },
    Dispute { guild: u64, product: u64 },
    DisputeWon { guild: u64, product: u64 },
    Status { guild: u64, product: u64 },
    Validate { file: String },
    Serve { addr: String },
    Doctor { addr: String },
    Deploy(Deploy),
    RotateVault { new_key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub db: String,
    pub command: Command,
}

const MAX_DAYS: u32 = 3650;

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

struct Flags(Vec<(String, String)>);

impl Flags {
    fn parse(args: &[String]) -> Result<Self, CliError> {
        let mut pairs = Vec::new();
        let mut iter = args.iter();
        while let Some(key) = iter.next() {
            let name = key
                .strip_prefix("--")
                .filter(|n| !n.is_empty())
                .ok_or_else(|| usage(format!("unexpected argument {key}")))?;
            let value = iter
                .next()
                .ok_or_else(|| usage(format!("--{name} needs a value")))?;
            pairs.push((name.to_string(), value.clone()));
        }
        Ok(Self(pairs))
    }

    fn take_all(&mut self, name: &str) -> Vec<String> {
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.0)
            .into_iter()
            .partition(|(k, _)| k == name);
        self.0 = kept;
        taken.into_iter().map(|(_, v)| v).collect()
    }

    fn take_opt(&mut self, name: &str) -> Result<Option<String>, CliError> {
        let mut values = self.take_all(name);
        match values.len() {
            0 => Ok(None),
            1 => Ok(values.pop()),
            _ => Err(usage(format!("--{name} given more than once"))),
        }
    }

    fn take(&mut self, name: &str) -> Result<String, CliError> {
        self.take_opt(name)?
            .ok_or_else(|| usage(format!("--{name} is required")))
    }

    fn id(&mut self, name: &str) -> Result<u64, CliError> {
        let raw = self.take(name)?;
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(usage(format!("--{name} must be a decimal number")));
        }
        raw.parse()
            .map_err(|_| usage(format!("--{name} is too large")))
    }

    fn days(&mut self) -> Result<u32, CliError> {
        let raw = self.take("days")?;
        match raw.parse::<u32>() {
            Ok(d) if (1..=MAX_DAYS).contains(&d) && raw.bytes().all(|b| b.is_ascii_digit()) => {
                Ok(d)
            }
            _ => Err(usage(format!("--days must be between 1 and {MAX_DAYS}"))),
        }
    }

    fn finish(self) -> Result<(), CliError> {
        match self.0.first() {
            Some((name, _)) => Err(usage(format!("unknown flag --{name}"))),
            None => Ok(()),
        }
    }
}

fn parse_tier(raw: &str) -> Result<Tier, CliError> {
    match raw {
        "free" => Ok(Tier::Free),
        "premium" => Ok(Tier::Premium),
        _ => Err(usage("--tier must be free or premium")),
    }
}

pub fn parse(args: &[String]) -> Result<Invocation, CliError> {
    let mut rest = args;
    let mut db = "platform.sqlite".to_string();
    if rest.first().map(String::as_str) == Some("--db") {
        db = rest
            .get(1)
            .ok_or_else(|| usage("--db needs a value"))?
            .clone();
        rest = rest.get(2..).unwrap_or(&[]);
    }
    let (name, flags) = rest.split_first().ok_or_else(|| usage("missing command"))?;
    let mut f = Flags::parse(flags)?;
    let command = match name.as_str() {
        "product-token" => Command::ProductToken {
            product: f.id("product")?,
        },
        "pay" => Command::Pay {
            guild: f.id("guild")?,
            product: f.id("product")?,
            days: f.days()?,
        },
        "cancel" => Command::Cancel {
            guild: f.id("guild")?,
            product: f.id("product")?,
        },
        "dispute" => Command::Dispute {
            guild: f.id("guild")?,
            product: f.id("product")?,
        },
        "dispute-won" => Command::DisputeWon {
            guild: f.id("guild")?,
            product: f.id("product")?,
        },
        "status" => Command::Status {
            guild: f.id("guild")?,
            product: f.id("product")?,
        },
        "validate" => Command::Validate {
            file: f.take("file")?,
        },
        "serve" => Command::Serve {
            addr: f
                .take_opt("addr")?
                .unwrap_or_else(|| "127.0.0.1:8080".to_string()),
        },
        "doctor" => Command::Doctor {
            addr: f
                .take_opt("addr")?
                .unwrap_or_else(|| "127.0.0.1:8080".to_string()),
        },
        "deploy" => Command::Deploy(Deploy {
            product: f.id("product")?,
            tier: parse_tier(&f.take("tier")?)?,
            image: f.take("image")?,
            api_url: f.take("api-url")?,
            network: f
                .take_opt("network")?
                .unwrap_or_else(|| "bridge".to_string()),
            env: f.take_all("env"),
        }),
        "rotate-vault" => Command::RotateVault {
            new_key: f.take("new-key")?,
        },
        other => return Err(usage(format!("unknown command {other}"))),
    };
    f.finish()?;
    Ok(Invocation { db, command })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Invocation, CliError> {
        let owned: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        parse(&owned)
    }

    fn command(args: &[&str]) -> Command {
        match run(args) {
            Ok(i) => i.command,
            Err(e) => panic!("{args:?} rejected: {e}"),
        }
    }

    fn rejected(args: &[&str]) -> bool {
        matches!(run(args), Err(CliError::Usage(_)))
    }

    #[test]
    fn the_database_defaults_and_can_be_overridden() {
        let default = run(&["status", "--guild", "1", "--product", "2"]).map(|i| i.db);
        assert_eq!(default.ok().as_deref(), Some("platform.sqlite"));
        let custom =
            run(&["--db", "x.db", "status", "--guild", "1", "--product", "2"]).map(|i| i.db);
        assert_eq!(custom.ok().as_deref(), Some("x.db"));
    }

    #[test]
    fn store_commands_take_guild_and_product() {
        let g = |c: &str| command(&[c, "--guild", "10", "--product", "1"]);
        assert_eq!(
            g("cancel"),
            Command::Cancel {
                guild: 10,
                product: 1
            }
        );
        assert_eq!(
            g("dispute"),
            Command::Dispute {
                guild: 10,
                product: 1
            }
        );
        assert_eq!(
            g("dispute-won"),
            Command::DisputeWon {
                guild: 10,
                product: 1
            }
        );
        assert_eq!(
            g("status"),
            Command::Status {
                guild: 10,
                product: 1
            }
        );
        assert_eq!(
            command(&["pay", "--guild", "10", "--product", "1", "--days", "30"]),
            Command::Pay {
                guild: 10,
                product: 1,
                days: 30
            }
        );
        assert_eq!(
            command(&["product-token", "--product", "7"]),
            Command::ProductToken { product: 7 }
        );
    }

    #[test]
    fn flag_order_does_not_matter() {
        assert_eq!(
            command(&["pay", "--days", "5", "--product", "1", "--guild", "10"]),
            Command::Pay {
                guild: 10,
                product: 1,
                days: 5
            }
        );
    }

    #[test]
    fn days_are_between_1_and_3650() {
        for (days, ok) in [
            ("0", false),
            ("1", true),
            ("3650", true),
            ("3651", false),
            ("-1", false),
            ("+5", false),
            ("x", false),
            ("", false),
        ] {
            let r = run(&["pay", "--guild", "1", "--product", "1", "--days", days]);
            assert_eq!(r.is_ok(), ok, "{days:?}");
        }
    }

    #[test]
    fn identifiers_must_be_plain_decimal_numbers() {
        for bad in ["abc", "+5", "-1", "1.5", "", "18446744073709551616"] {
            assert!(
                rejected(&["status", "--guild", bad, "--product", "1"]),
                "{bad:?}"
            );
        }
        assert_eq!(
            command(&[
                "status",
                "--guild",
                "18446744073709551615",
                "--product",
                "0"
            ]),
            Command::Status {
                guild: u64::MAX,
                product: 0
            }
        );
    }

    #[test]
    fn structural_mistakes_are_usage_errors() {
        assert!(rejected(&[]));
        assert!(rejected(&["--db"]));
        assert!(rejected(&["--db", "x.db"]));
        assert!(rejected(&["bogus"]));
        assert!(rejected(&["status", "--guild", "1"]));
        assert!(rejected(&["status", "--guild"]));
        assert!(rejected(&["status", "guild", "1"]));
        assert!(rejected(&["status", "--", "1"]));
        assert!(rejected(&[
            "status",
            "--guild",
            "1",
            "--product",
            "2",
            "--extra",
            "3"
        ]));
        assert!(rejected(&[
            "status",
            "--guild",
            "1",
            "--guild",
            "2",
            "--product",
            "2"
        ]));
    }

    #[test]
    fn validate_and_serve() {
        assert_eq!(
            command(&["validate", "--file", "bot.toml"]),
            Command::Validate {
                file: "bot.toml".to_string()
            }
        );
        assert_eq!(
            command(&["serve"]),
            Command::Serve {
                addr: "127.0.0.1:8080".to_string()
            }
        );
        assert_eq!(
            command(&["serve", "--addr", "0.0.0.0:9"]),
            Command::Serve {
                addr: "0.0.0.0:9".to_string()
            }
        );
    }

    #[test]
    fn doctor_defaults_to_the_local_address() {
        assert_eq!(
            command(&["doctor"]),
            Command::Doctor {
                addr: "127.0.0.1:8080".to_string()
            }
        );
        assert_eq!(
            command(&["doctor", "--addr", "127.0.0.1:9090"]),
            Command::Doctor {
                addr: "127.0.0.1:9090".to_string()
            }
        );
        assert!(rejected(&["doctor", "--port", "1"]));
    }

    #[test]
    fn deploy_collects_options_and_repeated_env() {
        let c = command(&[
            "deploy",
            "--product",
            "1",
            "--tier",
            "premium",
            "--image",
            "bot:1",
            "--api-url",
            "http://h:80",
            "--env",
            "A=1",
            "--env",
            "B=2",
        ]);
        assert_eq!(
            c,
            Command::Deploy(Deploy {
                product: 1,
                tier: Tier::Premium,
                image: "bot:1".to_string(),
                api_url: "http://h:80".to_string(),
                network: "bridge".to_string(),
                env: vec!["A=1".to_string(), "B=2".to_string()],
            })
        );
        let free = command(&[
            "deploy",
            "--product",
            "1",
            "--tier",
            "free",
            "--image",
            "bot:1",
            "--api-url",
            "u",
            "--network",
            "mynet",
        ]);
        assert!(
            matches!(free, Command::Deploy(d) if d.tier == Tier::Free && d.network == "mynet" && d.env.is_empty())
        );
        assert!(rejected(&[
            "deploy",
            "--product",
            "1",
            "--tier",
            "gold",
            "--image",
            "i",
            "--api-url",
            "u"
        ]));
    }
}
