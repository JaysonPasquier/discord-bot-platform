use platform_testkit::{fake_routes, Reply};
use botplatform_sdk::{Config, FetchResult, PlatformClient};

fn ok_reply(access: bool) -> Reply {
    Reply::json(
        200,
        &format!(
            r#"{{"guild_id":"1","status":"{}","access":{}}}"#,
            if access { "active" } else { "expired" },
            access
        ),
    )
}

fn setup(replies: Vec<(&str, Reply)>, default_on_failure: bool) -> (PlatformClient, String) {
    let routes: Vec<(String, Reply)> = replies
        .into_iter()
        .map(|(p, r)| (p.to_string(), r))
        .collect();
    let (base, _handle) = fake_routes(routes, 100);
    let token = "tok".to_string();
    let base_clone = base.clone();
    let tok_clone = token.clone();
    let client = PlatformClient::with_config_and_injectors(
        Config {
            base_url: base.clone(),
            token,
            default_on_failure,
            ..Default::default()
        },
        Box::new(move |guild_id: &str, _| {
            let url = format!("{base_clone}/v1/premium/{guild_id}");
            let agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into();
            let Ok(mut response) = agent
                .get(&url)
                .header("Authorization", format!("Bearer {tok_clone}"))
                .call()
            else {
                return Ok(botplatform_sdk::FetchResult::ServerError);
            };
            let status = response.status().as_u16();
            if status == 401 {
                return Ok(FetchResult::AuthError);
            }
            if status >= 500 {
                return Ok(FetchResult::ServerError);
            }
            if status != 200 {
                return Ok(FetchResult::RequestError(status));
            }
            #[derive(serde::Deserialize)]
            struct Body {
                access: bool,
            }
            match response.body_mut().read_json::<Body>() {
                Ok(b) => Ok(FetchResult::Access(b.access)),
                Err(_) => Ok(FetchResult::ServerError),
            }
        }),
        Box::new(|| 1_000_000),
    );
    (client, base)
}

#[test]
fn an_active_subscription_returns_true() {
    let (client, _) = setup(
        vec![("GET /v1/premium/42", ok_reply(true))],
        false,
    );
    assert_eq!(client.is_premium(42).unwrap(), true);
}

#[test]
fn an_expired_subscription_returns_false() {
    let (client, _) = setup(
        vec![("GET /v1/premium/42", ok_reply(false))],
        false,
    );
    assert_eq!(client.is_premium(42).unwrap(), false);
}

#[test]
fn a_401_is_an_auth_error() {
    let (client, _) = setup(
        vec![("GET /v1/premium/1", Reply::json(401, "{}"))],
        false,
    );
    assert!(matches!(
        client.is_premium(1),
        Err(botplatform_sdk::PlatformError::Auth)
    ));
}

#[test]
fn a_server_error_falls_back_to_default_on_failure() {
    let (client, _) = setup(
        vec![("GET /v1/premium/1", Reply::json(503, "{}"))],
        true,
    );
    assert_eq!(client.is_premium(1).unwrap(), true);
}

#[test]
fn different_guilds_are_independent() {
    let (client, _) = setup(
        vec![
            ("GET /v1/premium/1", ok_reply(true)),
            ("GET /v1/premium/2", ok_reply(false)),
        ],
        false,
    );
    assert_eq!(client.is_premium(1).unwrap(), true);
    assert_eq!(client.is_premium(2).unwrap(), false);
}
