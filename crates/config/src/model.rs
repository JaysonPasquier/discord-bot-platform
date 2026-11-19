use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Node,
    Python,
    Rust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Interval {
    Month,
    Year,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub id: String,
    pub interval: Interval,
    pub price_cents: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entrypoint {
    pub start_command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Premium {
    pub start_command: String,
    pub plans: Vec<Plan>,
}

// `deny_unknown_fields` refuse tout champ inconnu, donc aussi un token collé par erreur dans le fichier.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotConfig {
    pub schema_version: u32,
    pub name: String,
    pub language: Language,
    pub support_contact: String,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    pub free: Entrypoint,
    pub premium: Option<Premium>,
}
