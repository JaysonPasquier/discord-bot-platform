use ring::hmac;
use serde::Deserialize;

// Stripe tolère un envoi en retard mais pas un rejoué des jours plus tard : au-delà de cette
// fenêtre, la signature est refusée même si elle est mathématiquement correcte.
const DEFAULT_TOLERANCE_SECS: i64 = 300;

fn parse_header(header: &str) -> Option<(i64, Vec<&str>)> {
    let mut timestamp = None;
    let mut sigs = Vec::new();
    for part in header.split(',') {
        let (key, value) = part.split_once('=')?;
        match key {
            "t" => timestamp = value.parse().ok(),
            "v1" => sigs.push(value),
            _ => {}
        }
    }
    Some((timestamp?, sigs))
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

// Vérifie `Stripe-Signature` : HMAC-SHA256 du secret de signature du webhook sur `{t}.{payload}`,
// comparée en temps constant, avec une fenêtre de tolérance contre le rejeu.
pub fn verify_signature(secret: &str, payload: &[u8], header: &str, now: i64) -> bool {
    verify_signature_with_tolerance(secret, payload, header, now, DEFAULT_TOLERANCE_SECS)
}

pub fn verify_signature_with_tolerance(
    secret: &str,
    payload: &[u8],
    header: &str,
    now: i64,
    tolerance_secs: i64,
) -> bool {
    if secret.is_empty() {
        return false;
    }
    let Some((timestamp, sigs)) = parse_header(header) else {
        return false;
    };
    if (now - timestamp).abs() > tolerance_secs {
        return false;
    }
    let mut signed = timestamp.to_string().into_bytes();
    signed.push(b'.');
    signed.extend_from_slice(payload);
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    sigs.iter()
        .any(|sig| hex_decode(sig).is_some_and(|bytes| hmac::verify(&key, &signed, &bytes).is_ok()))
}

#[derive(Debug, Clone, Deserialize)]
pub struct StripeEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: EventData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventData {
    pub object: serde_json::Value,
}

pub fn parse_event(payload: &[u8]) -> Option<StripeEvent> {
    serde_json::from_slice(payload).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_for(secret: &str, payload: &[u8], timestamp: i64) -> String {
        let mut signed = timestamp.to_string().into_bytes();
        signed.push(b'.');
        signed.extend_from_slice(payload);
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
        let tag = hmac::sign(&key, &signed);
        let hex: String = tag.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        format!("t={timestamp},v1={hex}")
    }

    #[test]
    fn a_correctly_signed_recent_payload_is_accepted() {
        let header = header_for("whsec_test", b"{}", 1000);
        assert!(verify_signature("whsec_test", b"{}", &header, 1000));
        assert!(verify_signature("whsec_test", b"{}", &header, 1250));
    }

    #[test]
    fn the_tolerance_boundary_itself_is_still_accepted() {
        let header = header_for("whsec_test", b"{}", 1000);
        // Exactement à la limite (300s) : encore accepté. Une seconde de plus : refusé.
        // Distingue `>` (bon) de `>=` (refuserait la limite elle-même).
        assert!(verify_signature("whsec_test", b"{}", &header, 1000 + 300));
        assert!(verify_signature("whsec_test", b"{}", &header, 1000 - 300));
        assert!(!verify_signature("whsec_test", b"{}", &header, 1000 + 301));
        assert!(!verify_signature("whsec_test", b"{}", &header, 1000 - 301));
    }

    #[test]
    fn a_wrong_secret_or_tampered_body_is_refused() {
        let header = header_for("whsec_test", b"{}", 1000);
        assert!(!verify_signature("whsec_other", b"{}", &header, 1000));
        assert!(!verify_signature("whsec_test", b"{\"x\":1}", &header, 1000));
    }

    #[test]
    fn a_replayed_old_signature_is_refused() {
        let header = header_for("whsec_test", b"{}", 1000);
        assert!(!verify_signature("whsec_test", b"{}", &header, 1000 + 301));
    }

    #[test]
    fn an_empty_secret_or_malformed_header_is_refused() {
        let header = header_for("whsec_test", b"{}", 1000);
        assert!(!verify_signature("", b"{}", &header, 1000));
        assert!(!verify_signature("whsec_test", b"{}", "garbage", 1000));
        assert!(!verify_signature(
            "whsec_test",
            b"{}",
            "t=notanumber,v1=aa",
            1000
        ));
    }

    #[test]
    fn events_are_parsed_with_their_raw_object_kept_intact() {
        let payload = br#"{"id":"evt_1","type":"checkout.session.completed","data":{"object":{"metadata":{"guild_id":"9"}}}}"#;
        let event = parse_event(payload).unwrap_or_else(|| panic!("should parse"));
        assert_eq!(event.id, "evt_1");
        assert_eq!(event.kind, "checkout.session.completed");
        assert_eq!(event.data.object["metadata"]["guild_id"], "9");
        assert!(parse_event(b"not json").is_none());
    }
}
