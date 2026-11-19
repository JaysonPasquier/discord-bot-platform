use ring::rand::SystemRandom;
use ring::signature::{RsaKeyPair, RSA_PKCS1_SHA256};

use crate::encoding::{base64_decode, base64url};
use crate::error::GitHubError;

// Le JWT d'une GitHub App : RS256, émis 60 s dans le passé (dérive d'horloge), 10 min au plus.
const BACKDATE_SECS: i64 = 60;
const LIFETIME_SECS: i64 = 9 * 60;

// GitHub fournit une clé PKCS#1 (« RSA PRIVATE KEY ») ; PKCS#8 (« PRIVATE KEY ») est aussi accepté.
fn key_pair(pem: &str) -> Result<RsaKeyPair, GitHubError> {
    let mut label = None;
    let mut body = String::new();
    for line in pem.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("-----BEGIN ") {
            label = rest.strip_suffix("-----");
        } else if !line.starts_with("-----") {
            body.push_str(line);
        }
    }
    let der = base64_decode(&body).ok_or(GitHubError::BadKey)?;
    match label {
        Some("RSA PRIVATE KEY") => RsaKeyPair::from_der(&der),
        Some("PRIVATE KEY") => RsaKeyPair::from_pkcs8(&der),
        _ => return Err(GitHubError::BadKey),
    }
    .map_err(|_| GitHubError::BadKey)
}

// Pour le diagnostic : la clé se lit-elle ?
pub fn check_private_key(pem: &str) -> Result<(), GitHubError> {
    key_pair(pem).map(|_| ())
}

pub(crate) fn app_jwt(app_id: &str, pem: &str, now: i64) -> Result<String, GitHubError> {
    let pair = key_pair(pem)?;
    let header = base64url(br#"{"alg":"RS256","typ":"JWT"}"#);
    let payload = format!(
        r#"{{"iat":{},"exp":{},"iss":"{}"}}"#,
        now - BACKDATE_SECS,
        now + LIFETIME_SECS,
        app_id
    );
    let signing_input = format!("{header}.{}", base64url(payload.as_bytes()));
    let mut signature = vec![0u8; pair.public().modulus_len()];
    pair.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        signing_input.as_bytes(),
        &mut signature,
    )
    .map_err(|_| GitHubError::BadKey)?;
    Ok(format!("{signing_input}.{}", base64url(&signature)))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::process::Command;

    use ring::signature::{UnparsedPublicKey, RSA_PKCS1_2048_8192_SHA256};

    use super::*;

    // Clés jetables créées par `openssl` pour le test : aucune clé privée n'est dans le dépôt.
    pub(crate) fn make_key(traditional: bool) -> String {
        let bin = std::env::var("OPENSSL_BIN").unwrap_or_else(|_| "openssl".to_string());
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "platform-github-key-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let (pkcs8, out) = (dir.join("k8.pem"), dir.join("k1.pem"));
        let run = |args: &[&str]| Command::new(&bin).args(args).output();
        let made = run(&[
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
            "-out",
            &pkcs8.display().to_string(),
        ]);
        assert!(
            made.is_ok_and(|o| o.status.success()),
            "openssl is needed for this test"
        );
        if traditional {
            let converted = run(&[
                "rsa",
                "-in",
                &pkcs8.display().to_string(),
                "-traditional",
                "-out",
                &out.display().to_string(),
            ]);
            assert!(converted.is_ok_and(|o| o.status.success()));
            return std::fs::read_to_string(out).unwrap_or_default();
        }
        std::fs::read_to_string(pkcs8).unwrap_or_default()
    }

    fn parts(jwt: &str) -> Vec<String> {
        jwt.split('.').map(str::to_string).collect()
    }

    #[test]
    fn the_jwt_has_the_documented_claims_and_a_valid_rs256_signature() {
        let pem = make_key(true);
        let jwt = app_jwt("123456", &pem, 1_700_000_000).unwrap_or_default();
        let parts = parts(&jwt);
        assert_eq!(parts.len(), 3, "{jwt}");
        let decode = |s: &str| {
            let standard = s.replace('-', "+").replace('_', "/");
            let padded = format!("{standard}{}", "=".repeat((4 - standard.len() % 4) % 4));
            String::from_utf8(base64_decode(&padded).unwrap_or_default()).unwrap_or_default()
        };
        assert_eq!(decode(&parts[0]), r#"{"alg":"RS256","typ":"JWT"}"#);
        assert_eq!(
            decode(&parts[1]),
            r#"{"iat":1699999940,"exp":1700000540,"iss":"123456"}"#
        );
        let pair = key_pair(&pem).unwrap_or_else(|_| panic!("key"));
        let public = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, pair.public().as_ref());
        let signature = base64_decode(&format!(
            "{}{}",
            parts[2].replace('-', "+").replace('_', "/"),
            "=".repeat((4 - parts[2].len() % 4) % 4)
        ))
        .unwrap_or_default();
        let signed = format!("{}.{}", parts[0], parts[1]);
        assert!(public.verify(signed.as_bytes(), &signature).is_ok());
        assert!(public.verify(b"tampered", &signature).is_err());
    }

    #[test]
    fn the_diagnostic_accepts_a_real_key_and_refuses_garbage() {
        assert_eq!(check_private_key(&make_key(true)), Ok(()));
        assert_eq!(check_private_key("nope"), Err(GitHubError::BadKey));
    }

    #[test]
    fn both_key_formats_are_accepted() {
        assert!(app_jwt("1", &make_key(true), 0).is_ok());
        assert!(app_jwt("1", &make_key(false), 0).is_ok());
    }

    #[test]
    fn bad_keys_are_refused_without_a_panic() {
        for bad in [
            "",
            "not a key",
            "-----BEGIN RSA PRIVATE KEY-----\n!!!\n-----END RSA PRIVATE KEY-----",
            "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----",
        ] {
            assert_eq!(app_jwt("1", bad, 0), Err(GitHubError::BadKey), "{bad:?}");
        }
    }
}
