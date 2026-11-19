use std::time::Duration;

use serde::Deserialize;

use crate::error::StripeError;

const API: &str = "https://api.stripe.com/v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSession {
    pub id: String,
    pub url: String,
}

pub struct CheckoutParams<'a> {
    pub success_url: &'a str,
    pub cancel_url: &'a str,
    pub currency: &'a str,
    pub product_name: &'a str,
    pub unit_amount_cents: u32,
    // Reportées sur la session ET sur le PaymentIntent (`payment_intent_data[metadata]`) : un
    // litige ne porte que l'identifiant du PaymentIntent, jamais celui de la session (OQ-stripe-001,
    // À VÉRIFIER auprès de la documentation Stripe avant un vrai lancement).
    pub metadata: &'a [(&'a str, &'a str)],
}

pub trait Stripe: Send + Sync + 'static {
    fn create_checkout_session(
        &self,
        params: &CheckoutParams<'_>,
    ) -> Result<CheckoutSession, StripeError>;
}

pub struct NoStripe;

impl Stripe for NoStripe {
    fn create_checkout_session(
        &self,
        _: &CheckoutParams<'_>,
    ) -> Result<CheckoutSession, StripeError> {
        Err(StripeError::NotConfigured)
    }
}

pub struct HttpStripe {
    secret_key: String,
    api_base: String,
    agent: ureq::Agent,
}

impl HttpStripe {
    pub fn new(secret_key: String) -> Self {
        Self::with_base(secret_key, API.to_string())
    }

    // L'adresse est un paramètre pour tester la forme exacte des requêtes contre un faux serveur.
    pub fn with_base(secret_key: String, api_base: String) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            secret_key,
            api_base,
            agent,
        }
    }
}

fn check(response: &ureq::http::Response<ureq::Body>) -> Result<(), StripeError> {
    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 => Err(StripeError::Unauthorized),
        400 | 402 | 422 => Err(StripeError::InvalidRequest(format!(
            "status {}",
            response.status()
        ))),
        other => Err(StripeError::Http(format!("unexpected status {other}"))),
    }
}

fn read_text(response: &mut ureq::http::Response<ureq::Body>) -> Result<String, StripeError> {
    response
        .body_mut()
        .read_to_string()
        .map_err(|e| StripeError::Http(e.to_string()))
}

#[derive(Deserialize)]
struct RawSession {
    id: String,
    url: Option<String>,
}

impl Stripe for HttpStripe {
    fn create_checkout_session(
        &self,
        params: &CheckoutParams<'_>,
    ) -> Result<CheckoutSession, StripeError> {
        let amount = params.unit_amount_cents.to_string();
        let mut form: Vec<(String, String)> = vec![
            ("mode".to_string(), "payment".to_string()),
            ("success_url".to_string(), params.success_url.to_string()),
            ("cancel_url".to_string(), params.cancel_url.to_string()),
            ("line_items[0][quantity]".to_string(), "1".to_string()),
            (
                "line_items[0][price_data][currency]".to_string(),
                params.currency.to_string(),
            ),
            ("line_items[0][price_data][unit_amount]".to_string(), amount),
            (
                "line_items[0][price_data][product_data][name]".to_string(),
                params.product_name.to_string(),
            ),
        ];
        for (key, value) in params.metadata {
            form.push((format!("metadata[{key}]"), (*value).to_string()));
            form.push((
                format!("payment_intent_data[metadata][{key}]"),
                (*value).to_string(),
            ));
        }
        let pairs: Vec<(&str, &str)> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let mut response = self
            .agent
            .post(format!("{}/checkout/sessions", self.api_base))
            .header("Authorization", format!("Bearer {}", self.secret_key))
            .send_form(pairs)
            .map_err(|e| StripeError::Http(e.to_string()))?;
        check(&response)?;
        let body = read_text(&mut response)?;
        let raw: RawSession = serde_json::from_str(&body).map_err(|_| StripeError::Parse)?;
        let url = raw.url.ok_or(StripeError::Parse)?;
        Ok(CheckoutSession { id: raw.id, url })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_stripe_always_refuses() {
        let params = CheckoutParams {
            success_url: "https://x/success",
            cancel_url: "https://x/cancel",
            currency: "usd",
            product_name: "Pro",
            unit_amount_cents: 500,
            metadata: &[],
        };
        assert_eq!(
            NoStripe.create_checkout_session(&params),
            Err(StripeError::NotConfigured)
        );
    }

    fn params() -> CheckoutParams<'static> {
        CheckoutParams {
            success_url: "https://x/success",
            cancel_url: "https://x/cancel",
            currency: "usd",
            product_name: "Pro",
            unit_amount_cents: 500,
            metadata: &[("guild_id", "9")],
        }
    }

    #[test]
    fn a_successful_response_is_read_as_the_session_id_and_url() {
        let (base, _handle) = platform_testkit::fake_server(vec![platform_testkit::Reply::json(
            200,
            r#"{"id":"cs_test_1","url":"https://checkout.stripe.test/pay/cs_test_1"}"#,
        )]);
        let client = HttpStripe::with_base("sk_test_fake".to_string(), base);
        let session = client.create_checkout_session(&params());
        assert_eq!(
            session,
            Ok(CheckoutSession {
                id: "cs_test_1".to_string(),
                url: "https://checkout.stripe.test/pay/cs_test_1".to_string(),
            })
        );
    }

    #[test]
    fn a_request_carries_the_secret_key_and_the_posted_fields() {
        let (base, handle) = platform_testkit::fake_server(vec![platform_testkit::Reply::json(
            200,
            r#"{"id":"cs_test_1","url":"https://checkout.stripe.test/pay/cs_test_1"}"#,
        )]);
        let client = HttpStripe::with_base("sk_test_fake".to_string(), base);
        assert!(client.create_checkout_session(&params()).is_ok());
        let seen = handle.join().unwrap_or_default();
        let request = seen.first().cloned().unwrap_or_default();
        assert!(request
            .to_lowercase()
            .contains("authorization: bearer sk_test_fake"));
        assert!(request.contains("metadata%5Bguild_id%5D=9"));
        assert!(request.contains("line_items%5B0%5D%5Bprice_data%5D%5Bunit_amount%5D=500"));
    }

    #[test]
    fn an_unauthorized_response_is_reported_as_such() {
        let (base, _handle) =
            platform_testkit::fake_server(vec![platform_testkit::Reply::json(401, "{}")]);
        let client = HttpStripe::with_base("sk_test_fake".to_string(), base);
        assert_eq!(
            client.create_checkout_session(&params()),
            Err(StripeError::Unauthorized)
        );
    }

    #[test]
    fn an_invalid_request_response_is_distinguished_from_other_failures() {
        let (base, _handle) =
            platform_testkit::fake_server(vec![platform_testkit::Reply::json(400, "{}")]);
        let client = HttpStripe::with_base("sk_test_fake".to_string(), base);
        assert_eq!(
            client.create_checkout_session(&params()),
            Err(StripeError::InvalidRequest(
                "status 400 Bad Request".to_string()
            ))
        );
    }

    #[test]
    fn an_unexpected_status_is_reported_with_its_code() {
        let (base, _handle) =
            platform_testkit::fake_server(vec![platform_testkit::Reply::json(500, "{}")]);
        let client = HttpStripe::with_base("sk_test_fake".to_string(), base);
        assert_eq!(
            client.create_checkout_session(&params()),
            Err(StripeError::Http("unexpected status 500".to_string()))
        );
    }

    #[test]
    fn a_response_missing_the_url_field_is_a_parse_error() {
        let (base, _handle) = platform_testkit::fake_server(vec![platform_testkit::Reply::json(
            200,
            r#"{"id":"cs_test_1"}"#,
        )]);
        let client = HttpStripe::with_base("sk_test_fake".to_string(), base);
        assert_eq!(
            client.create_checkout_session(&params()),
            Err(StripeError::Parse)
        );
    }
}
