const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

// Base64 « URL » sans remplissage, comme l'exige un JWT.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    let sextet = |n: u32, shift: u32| {
        let c = char::from(TABLE[usize::try_from((n >> shift) & 63).unwrap_or(0)]);
        match c {
            '+' => '-',
            '/' => '_',
            other => other,
        }
    };
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let n = (byte(0) << 16) + (byte(1) << 8) + byte(2);
        out.push(sextet(n, 18));
        out.push(sextet(n, 12));
        if chunk.len() > 1 {
            out.push(sextet(n, 6));
        }
        if chunk.len() > 2 {
            out.push(sextet(n, 0));
        }
    }
    out
}

// Base64 standard, espaces et retours à la ligne ignorés (contenu d'un fichier PEM).
pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut count = 0u32;
    let mut out = Vec::new();
    let mut padding = 0;
    for c in text.bytes().filter(|b| !b.is_ascii_whitespace()) {
        if c == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return None;
        }
        let value = u32::try_from(TABLE.iter().position(|&t| t == c)?).ok()?;
        bits = (bits << 6) + value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(u8::try_from((bits >> count) & 0xff).ok()?);
        }
    }
    (padding <= 2).then_some(out)
}

pub(crate) fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let digit = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    text.as_bytes()
        .chunks(2)
        .map(|pair| Some(digit(*pair.first()?)? * 16 + digit(*pair.get(1)?)?))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_matches_the_rfc_vectors_without_padding() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url(input.as_bytes()), expected, "{input:?}");
        }
        assert_eq!(base64url(&[0xfb, 0xff, 0xfe]), "-__-");
    }

    #[test]
    fn base64_decode_reverses_the_standard_alphabet() {
        for (text, expected) in [
            ("", &b""[..]),
            ("Zg==", b"f"),
            ("Zm8=", b"fo"),
            ("Zm9v", b"foo"),
            ("Zm9vYg==", b"foob"),
            ("Zm9v\nYmFy\r\n", b"foobar"),
            ("+//+", &[0xfb, 0xff, 0xfe]),
        ] {
            assert_eq!(base64_decode(text).as_deref(), Some(expected), "{text:?}");
        }
    }

    #[test]
    fn base64_decode_refuses_bad_input() {
        for bad in ["Zm9v!", "Z=m9", "Zg===", "-__-", "Zm9v=Zm9v"] {
            assert_eq!(base64_decode(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn hex_decoding_is_strict() {
        assert_eq!(hex_decode("00ff1A"), Some(vec![0x00, 0xff, 0x1a]));
        assert_eq!(hex_decode(""), Some(vec![]));
        for bad in ["0", "zz", "0g", "+1", "00 "] {
            assert_eq!(hex_decode(bad), None, "{bad:?}");
        }
    }
}
