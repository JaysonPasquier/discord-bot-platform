use crate::base64::base64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageError {
    Empty,
    TooLarge,
    UnsupportedType,
}

// Le type vient du contenu, jamais du nom ni du type annoncé par le navigateur.
fn mime_of(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

// Un fichier envoyé pour la page publique : type reconnu par le contenu, taille bornée.
pub(crate) fn checked(bytes: &[u8], max: usize) -> Result<&'static str, ImageError> {
    if bytes.is_empty() {
        return Err(ImageError::Empty);
    }
    if bytes.len() > max {
        return Err(ImageError::TooLarge);
    }
    mime_of(bytes).ok_or(ImageError::UnsupportedType)
}

// Format « Image Data » de l'API Discord : `data:image/png;base64,...`.
// `max` est la limite de la plateforme (réglable), pas celle de Discord.
pub(crate) fn data_uri(bytes: &[u8], max: usize) -> Result<String, ImageError> {
    if bytes.is_empty() {
        return Err(ImageError::Empty);
    }
    if bytes.len() > max {
        return Err(ImageError::TooLarge);
    }
    let mime = mime_of(bytes).ok_or(ImageError::UnsupportedType)?;
    Ok(format!("data:{mime};base64,{}", base64(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: usize = 1000;
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2];

    #[test]
    fn the_three_supported_formats_are_recognised_by_content() {
        assert_eq!(mime_of(PNG), Some("image/png"));
        assert_eq!(mime_of(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(mime_of(b"GIF89a.."), Some("image/gif"));
        assert_eq!(mime_of(b"GIF87a.."), Some("image/gif"));
    }

    #[test]
    fn other_content_is_refused_even_with_a_picture_name() {
        for bad in [
            &b"<svg xmlns='http://www.w3.org/2000/svg'/>"[..],
            b"<html></html>",
            b"GIF90a",
            b"GIF8",
            &[0xff, 0xd8],
            &[0x89, b'P', b'N', b'G'],
            b"MZ\x90\x00",
        ] {
            assert_eq!(
                data_uri(bad, LIMIT),
                Err(ImageError::UnsupportedType),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_data_uri_has_the_documented_shape() {
        let uri = data_uri(PNG, LIMIT).unwrap_or_default();
        assert_eq!(uri, format!("data:image/png;base64,{}", base64(PNG)));
        assert!(uri.starts_with("data:image/png;base64,iVBORw0KGgo"));
    }

    #[test]
    fn empty_and_oversized_files_are_refused_with_their_own_reason() {
        assert_eq!(data_uri(&[], LIMIT), Err(ImageError::Empty));
        let mut at_limit = PNG.to_vec();
        at_limit.resize(LIMIT, 0);
        assert!(data_uri(&at_limit, LIMIT).is_ok());
        at_limit.push(0);
        assert_eq!(data_uri(&at_limit, LIMIT), Err(ImageError::TooLarge));
        assert!(data_uri(&at_limit, LIMIT + 1).is_ok());
    }
}
