use ring::hmac;
use serde::Deserialize;

use crate::encoding::hex_decode;
use crate::error::GitHubError;
use crate::types::{valid_branch, valid_repo, valid_sha};

// GitHub signe chaque livraison : `X-Hub-Signature-256: sha256=<HMAC-SHA256 hexadécimal>`.
// La comparaison est à temps constant (`hmac::verify`), jamais un `==`.
pub fn verify_signature(secret: &[u8], body: &[u8], header: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Some(signature) = header.strip_prefix("sha256=").and_then(hex_decode) else {
        return false;
    };
    hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, secret), body, &signature).is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushEvent {
    pub installation: u64,
    pub repo: String,
    pub branch: String,
    pub sha: String,
}

#[derive(Deserialize)]
struct RawPush {
    #[serde(rename = "ref")]
    git_ref: String,
    after: String,
    #[serde(default)]
    deleted: bool,
    repository: RawRepository,
    installation: Option<RawInstallation>,
}

#[derive(Deserialize)]
struct RawRepository {
    full_name: String,
}

#[derive(Deserialize)]
struct RawInstallation {
    id: u64,
}

// `None` : un push qui ne déploie rien (étiquette, branche supprimée).
pub fn parse_push(body: &[u8]) -> Result<Option<PushEvent>, GitHubError> {
    let raw: RawPush = serde_json::from_slice(body).map_err(|_| GitHubError::Parse)?;
    let Some(branch) = raw.git_ref.strip_prefix("refs/heads/") else {
        return Ok(None);
    };
    if raw.deleted {
        return Ok(None);
    }
    let installation = raw.installation.ok_or(GitHubError::Parse)?.id;
    if !valid_repo(&raw.repository.full_name) || !valid_branch(branch) || !valid_sha(&raw.after) {
        return Err(GitHubError::Parse);
    }
    Ok(Some(PushEvent {
        installation,
        repo: raw.repository.full_name,
        branch: branch.to_string(),
        sha: raw.after,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exemple tiré de la documentation GitHub.
    const SECRET: &[u8] = b"It's a Secret to Everybody";
    const PAYLOAD: &[u8] = b"Hello, World!";
    const SIGNATURE: &str =
        "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17";
    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn the_documented_example_verifies() {
        assert!(verify_signature(SECRET, PAYLOAD, SIGNATURE));
    }

    #[test]
    fn any_change_breaks_the_signature() {
        assert!(!verify_signature(SECRET, b"Hello, World?", SIGNATURE));
        assert!(!verify_signature(b"other", PAYLOAD, SIGNATURE));
        assert!(!verify_signature(
            SECRET,
            PAYLOAD,
            &SIGNATURE.replace("75", "76")
        ));
    }

    #[test]
    fn malformed_headers_and_empty_secrets_are_refused() {
        for header in [
            "",
            "sha256=",
            "sha1=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17",
            &SIGNATURE[7..],
            "sha256=zz",
            "sha256=757107ea",
        ] {
            assert!(!verify_signature(SECRET, PAYLOAD, header), "{header:?}");
        }
        assert!(!verify_signature(b"", PAYLOAD, SIGNATURE));
    }

    fn push(git_ref: &str, deleted: bool) -> String {
        format!(
            r#"{{"ref":"{git_ref}","before":"0","after":"{SHA}","deleted":{deleted},"repository":{{"full_name":"alice/bot"}},"installation":{{"id":42}}}}"#
        )
    }

    #[test]
    fn a_branch_push_gives_the_installation_repo_branch_and_commit() {
        assert_eq!(
            parse_push(push("refs/heads/feature/x", false).as_bytes()),
            Ok(Some(PushEvent {
                installation: 42,
                repo: "alice/bot".to_string(),
                branch: "feature/x".to_string(),
                sha: SHA.to_string(),
            }))
        );
    }

    #[test]
    fn tags_and_deleted_branches_deploy_nothing() {
        assert_eq!(parse_push(push("refs/tags/v1", false).as_bytes()), Ok(None));
        assert_eq!(
            parse_push(push("refs/heads/main", true).as_bytes()),
            Ok(None)
        );
    }

    #[test]
    fn malformed_pushes_are_parse_errors() {
        let no_installation =
            push("refs/heads/main", false).replace(r#","installation":{"id":42}"#, "");
        for bad in [
            "not json".to_string(),
            "{}".to_string(),
            no_installation,
            push("refs/heads/main", false).replace(SHA, "short"),
            push("refs/heads/main", false).replace("alice/bot", "../etc"),
            push("refs/heads/a..b", false),
        ] {
            assert_eq!(parse_push(bad.as_bytes()), Err(GitHubError::Parse), "{bad}");
        }
    }
}
