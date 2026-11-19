// Jamais de clé de production sans décision explicite. On refuse tout ce qui
// ne ressemble pas clairement à une clé de test, plutôt que de faire confiance à l'environnement.

pub fn is_test_secret_key(key: &str) -> bool {
    key.starts_with("sk_test_") && key.len() > "sk_test_".len()
}

pub fn is_live_secret_key(key: &str) -> bool {
    key.starts_with("sk_live_") || key.starts_with("rk_live_")
}

pub fn is_webhook_secret(secret: &str) -> bool {
    secret.starts_with("whsec_") && secret.len() > "whsec_".len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keys_are_recognized_and_live_keys_are_refused() {
        assert!(is_test_secret_key("sk_test_abc123"));
        assert!(!is_test_secret_key("sk_live_abc123"));
        assert!(!is_test_secret_key("sk_test_"));
        assert!(!is_test_secret_key(""));
        assert!(is_live_secret_key("sk_live_abc123"));
        assert!(is_live_secret_key("rk_live_abc123"));
        assert!(!is_live_secret_key("sk_test_abc123"));
    }

    #[test]
    fn webhook_secrets_have_the_whsec_prefix() {
        assert!(is_webhook_secret("whsec_abc123"));
        assert!(!is_webhook_secret("whsec_"));
        assert!(!is_webhook_secret("abc123"));
    }
}
