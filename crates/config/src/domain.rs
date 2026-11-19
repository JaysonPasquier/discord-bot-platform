#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainError {
    Empty,
    TooLong,
    Wildcard,
    ForbiddenCharacter,
    SingleLabel,
    BadLabel,
    NumericTld,
    InternalName,
}

const INTERNAL_SUFFIXES: [&str; 5] = ["localhost", "local", "internal", "lan", "home.arpa"];

// Un nom public qui résout vers une IP privée (rebinding DNS) n'est pas détectable ici :
// le filtrage réseau du runner le bloque à l'exécution.
pub fn check_domain(domain: &str) -> Result<(), DomainError> {
    if domain.is_empty() {
        return Err(DomainError::Empty);
    }
    if domain.len() > 253 {
        return Err(DomainError::TooLong);
    }
    if domain.contains('*') {
        return Err(DomainError::Wildcard);
    }
    let allowed = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.';
    if !domain.bytes().all(allowed) {
        return Err(DomainError::ForbiddenCharacter);
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return Err(DomainError::SingleLabel);
    }
    let bad_label =
        |l: &&str| l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-');
    if labels.iter().any(bad_label) {
        return Err(DomainError::BadLabel);
    }
    if labels
        .last()
        .is_some_and(|tld| tld.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(DomainError::NumericTld);
    }
    let internal = |s: &&str| {
        domain == *s
            || domain
                .strip_suffix(*s)
                .is_some_and(|rest| rest.ends_with('.'))
    };
    if INTERNAL_SUFFIXES.iter().any(internal) {
        return Err(DomainError::InternalName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_public_names() {
        assert_eq!(check_domain("api.example.com"), Ok(()));
        assert_eq!(check_domain("a-b.example.co.uk"), Ok(()));
        assert_eq!(check_domain("xn--nxasmq6b.example"), Ok(()));
    }

    #[test]
    fn rejects_empty() {
        assert_eq!(check_domain(""), Err(DomainError::Empty));
    }

    #[test]
    fn total_length_limit_is_253() {
        let label = "a".repeat(61);
        let ok = format!("{label}.{label}.{label}.{label}.ab"); // 61*4 + 4 dots + 2 = 250
        assert_eq!(check_domain(&ok), Ok(()));
        let at_limit = format!("{label}.{label}.{label}.{label}.{}", "b".repeat(5)); // 253
        assert_eq!(at_limit.len(), 253);
        assert_eq!(check_domain(&at_limit), Ok(()));
        let over = format!("{at_limit}b");
        assert_eq!(check_domain(&over), Err(DomainError::TooLong));
    }

    #[test]
    fn rejects_wildcards() {
        assert_eq!(check_domain("*.example.com"), Err(DomainError::Wildcard));
        assert_eq!(check_domain("a.*.com"), Err(DomainError::Wildcard));
    }

    #[test]
    fn rejects_schemes_ports_paths_and_credentials() {
        for bad in [
            "https://example.com",
            "example.com:443",
            "example.com/x",
            "u@example.com",
            "exa mple.com",
        ] {
            assert_eq!(
                check_domain(bad),
                Err(DomainError::ForbiddenCharacter),
                "{bad}"
            );
        }
    }

    #[test]
    fn rejects_uppercase_to_keep_one_spelling() {
        assert_eq!(
            check_domain("Example.com"),
            Err(DomainError::ForbiddenCharacter)
        );
    }

    #[test]
    fn rejects_ipv6_literals() {
        assert_eq!(check_domain("::1"), Err(DomainError::ForbiddenCharacter));
    }

    #[test]
    fn rejects_single_label() {
        assert_eq!(check_domain("example"), Err(DomainError::SingleLabel));
        assert_eq!(check_domain("localhost"), Err(DomainError::SingleLabel));
    }

    #[test]
    fn label_length_limit_is_63() {
        let ok = format!("{}.com", "a".repeat(63));
        assert_eq!(check_domain(&ok), Ok(()));
        let over = format!("{}.com", "a".repeat(64));
        assert_eq!(check_domain(&over), Err(DomainError::BadLabel));
    }

    #[test]
    fn rejects_empty_labels_and_edge_hyphens() {
        for bad in [
            ".example.com",
            "example..com",
            "example.com.",
            "-a.example.com",
            "a-.example.com",
        ] {
            assert_eq!(check_domain(bad), Err(DomainError::BadLabel), "{bad}");
        }
    }

    #[test]
    fn rejects_ipv4_literals() {
        assert_eq!(check_domain("127.0.0.1"), Err(DomainError::NumericTld));
        assert_eq!(check_domain("10.0.0.1"), Err(DomainError::NumericTld));
        assert_eq!(
            check_domain("169.254.169.254"),
            Err(DomainError::NumericTld)
        );
    }

    #[test]
    fn a_numeric_label_before_a_real_tld_is_fine() {
        assert_eq!(check_domain("123.example.com"), Ok(()));
    }

    #[test]
    fn rejects_internal_names() {
        for bad in [
            "db.internal",
            "printer.local",
            "x.localhost",
            "nas.lan",
            "router.home.arpa",
        ] {
            assert_eq!(check_domain(bad), Err(DomainError::InternalName), "{bad}");
        }
    }

    #[test]
    fn internal_suffix_must_match_a_whole_label() {
        assert_eq!(check_domain("notlocal.com"), Ok(()));
        assert_eq!(check_domain("mylocal.example"), Ok(()));
        assert_eq!(check_domain("localhost.example.com"), Ok(()));
        assert_eq!(check_domain("foo.notlocal"), Ok(()));
    }
}
