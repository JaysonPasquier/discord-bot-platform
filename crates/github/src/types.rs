#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub id: u64,
    pub login: String,
    pub selection: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub sha: String,
}

fn part_ok(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 100
        && part != "."
        && part != ".."
        && part
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

// `proprietaire/depot` : seuls les caractères sûrs pour un chemin d'URL et un nom de dossier.
pub fn valid_repo(name: &str) -> bool {
    name.split_once('/')
        .is_some_and(|(owner, repo)| part_ok(owner) && part_ok(repo))
}

// Nom de branche : prudent, plus étroit que Git. Pas de `..`, pas de caractère exotique.
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && !name.starts_with('/')
        && !name.ends_with('/')
        && !name.starts_with('-')
        && !name.contains("..")
        && !name.contains("//")
        && !name.ends_with(".lock")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
}

pub fn valid_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_are_owner_slash_name_with_safe_characters() {
        for good in ["alice/bot", "a-b/c_d.e", "Alice/Bot-2"] {
            assert!(valid_repo(good), "{good}");
        }
        for bad in [
            "",
            "alice",
            "alice/",
            "/bot",
            "a/b/c",
            "../bot",
            "alice/..",
            "alice/.",
            "a b/c",
            "alice/bot;rm",
            "alice/bot?x=1",
            "alice/bö",
        ] {
            assert!(!valid_repo(bad), "{bad}");
        }
        assert!(valid_repo(&format!(
            "{}/{}",
            "a".repeat(100),
            "b".repeat(100)
        )));
        assert!(!valid_repo(&format!("{}/b", "a".repeat(101))));
    }

    #[test]
    fn branches_allow_slashes_but_nothing_that_escapes_a_path() {
        for good in ["main", "feature/new-ui", "release_1.2", "v1.0.0"] {
            assert!(valid_branch(good), "{good}");
        }
        for bad in [
            "", "/main", "main/", "-rf", "a..b", "a//b", "x.lock", "a b", "a?b", "a#b", "a%2fb",
            "é",
        ] {
            assert!(!valid_branch(bad), "{bad}");
        }
        assert!(valid_branch(&"a".repeat(200)));
        assert!(!valid_branch(&"a".repeat(201)));
    }

    #[test]
    fn a_commit_is_forty_lowercase_hex_digits() {
        assert!(valid_sha(&"a".repeat(40)));
        assert!(valid_sha(
            &"0123456789abcdef"
                .repeat(3)
                .chars()
                .take(40)
                .collect::<String>()
        ));
        for bad in [
            &"a".repeat(39),
            &"a".repeat(41),
            &"A".repeat(40),
            &"g".repeat(40),
        ] {
            assert!(!valid_sha(bad), "{bad}");
        }
    }
}
