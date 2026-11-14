//! A service that receives signed webhooks, and sends them to a subscriber, with
//! Axa's `webhooks` and `http-client` features.
//!
//! `/hooks/billing` takes a delivery only when it is signed by the key in
//! `WEBHOOK_SECRET`, within five minutes. `/api/notify` signs an event and sends it to
//! `SUBSCRIBER_URL` with a client that will not reach a private address, whatever that
//! URL says.
//!
//! ```sh
//! export WEBHOOK_SECRET="whsec_$(openssl rand -base64 32)"
//! export SUBSCRIBER_URL=https://example.com/hooks
//! cargo run
//! curl -X POST localhost:1241/api/notify -H 'content-type: application/json' -d '{"type":"invoice.paid"}'
//! ```

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use axa::{
    Environment, Result,
    config::{Config, Defaults, Settings},
    error::ErrorDetail,
    extract::Json,
    http_client::{self, Client, refusal},
    logger,
    prelude::*,
    server,
    webhooks::{Signer, SigningKey, Verified, Verifier},
};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

/// The most of a subscriber's answer this reads.
const MAX_ANSWER_BYTES: usize = 64_000;

/// What the webhook routes share, kept in `AppContext::shared_store`. The verifier goes
/// there on its own as well, where `Verified` looks for it.
struct Hooks {
    signer: Signer,
    verifier: Verifier,
    client: Client,
    subscriber: Option<String>,
}

/// The service's own settings, read with Axa's rules.
struct HookSettings {
    key: SigningKey,
    subscriber: Option<String>,
}

/// `WEBHOOK_SECRET`, or the file `WEBHOOK_SECRET_FILE` names, and `SUBSCRIBER_URL`.
/// Production refuses to boot without a valid secret.
fn hook_settings(settings: &Settings<impl Fn(&str) -> Option<String>>) -> Option<HookSettings> {
    let secret = settings.required_secret("WEBHOOK_SECRET")?;
    let key = secret.parse::<SigningKey>().map_err(|error| {
        settings.problem(format!("WEBHOOK_SECRET is not a webhook key: {error}"));
    });
    Some(HookSettings {
        key: key.ok()?,
        subscriber: settings.optional("SUBSCRIBER_URL"),
    })
}

/// A billing event, as a subscriber or the billing provider sends it.
#[derive(Debug, Serialize, Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
}

/// Take a delivery from the billing provider. `Verified` runs this only for a delivery
/// signed by the key in `WEBHOOK_SECRET`, so it parses only what was signed.
async fn billing(Verified { delivery, body }: Verified) -> Result<Response> {
    let Ok(event) = serde_json::from_slice::<Event>(&body) else {
        return bad_request("the delivery is not a billing event");
    };
    // A real service records `delivery.id` for the five-minute window, and does
    // nothing for an id it has seen, as a delivery may come twice.
    tracing::info!(id = %delivery.id, kind = %event.kind, "webhook_received");
    format::json(serde_json::json!({ "received": delivery.id }))
}

/// An id for each delivery this process sends.
fn delivery_id() -> String {
    static SENT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("msg_{nanos:x}_{}", SENT.fetch_add(1, Ordering::Relaxed))
}

/// Sign `event` and send it to the subscriber. A subscriber URL the client refuses is a
/// `422`, and one that fails or answers badly is a `502`.
async fn notify(State(ctx): State<AppContext>, Json(event): Json<Event>) -> Result<Response> {
    let Some(hooks) = ctx.shared_store.get_ref::<Hooks>() else {
        return Err(axa::Error::InternalServerError);
    };
    let Some(subscriber) = &hooks.subscriber else {
        return bad_request("no SUBSCRIBER_URL is set");
    };
    let body = serde_json::to_vec(&event)?;
    let id = delivery_id();
    let headers = hooks
        .signer
        .headers_now(&id, &body)
        .map_err(|_| axa::Error::InternalServerError)?;
    let refused = |detail: &str| {
        Err(axa::Error::CustomError(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorDetail::new("subscriber_refused", detail),
        ))
    };
    let request = match hooks.client.post(subscriber) {
        Ok(request) => request,
        Err(_) => return refused("SUBSCRIBER_URL may not be reached"),
    };
    let sent = request
        .headers(headers)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await;
    let response = match sent {
        Ok(response) => response,
        Err(error) if refusal(&error).is_some() => {
            return refused("SUBSCRIBER_URL may not be reached");
        }
        Err(_) => return bad_gateway(),
    };
    let status = response.status();
    if http_client::bytes_limited(response, MAX_ANSWER_BYTES)
        .await
        .is_err()
        || !status.is_success()
    {
        return bad_gateway();
    }
    format::json(serde_json::json!({ "sent": id }))
}

fn bad_gateway() -> Result<Response> {
    Err(axa::Error::CustomError(
        StatusCode::BAD_GATEWAY,
        ErrorDetail::new(
            "subscriber_failed",
            "the subscriber did not take the delivery",
        ),
    ))
}

/// The service's routes, with the health routes Axa adds.
fn routes() -> AppRoutes {
    AppRoutes::with_default_routes()
        .add_route(
            Routes::new()
                .prefix("/hooks")
                .add("/billing", post(billing)),
        )
        .add_route(Routes::new().prefix("/api").add("/notify", post(notify)))
}

/// The settings for `environment`, read through `lookup`: Axa's, and the service's own.
fn settings(
    environment: &Environment,
    lookup: impl Fn(&str) -> Option<String> + Copy,
) -> Result<(Config, Hooks)> {
    let defaults = Defaults {
        database: "",
        ..Defaults::default()
    };
    let config = Config::from_lookup(environment, &defaults, lookup)?;
    let settings = Settings::from_lookup(environment, lookup);
    let own = hook_settings(&settings);
    let warnings = settings.finish()?;
    for warning in warnings {
        eprintln!("settings: {warning}");
    }
    let Some(own) = own else {
        return Err(axa::Error::string("WEBHOOK_SECRET is required"));
    };
    let client = Client::builder()
        .https_only(environment.is_production_like())
        .user_agent("webhooks-example")
        .build()
        .map_err(axa::Error::wrap)?;
    let hooks = Hooks {
        signer: Signer::new(own.key.clone()),
        verifier: Verifier::new(own.key.verifying_key()),
        client,
        subscriber: own.subscriber,
    };
    Ok((config, hooks))
}

/// The app for `environment`, with `hooks` in its shared store.
fn app(environment: Environment, config: Config, hooks: Hooks) -> Result<axum::Router> {
    let ctx = AppContext::new(environment, (), config);
    ctx.shared_store.insert(hooks.verifier.clone());
    ctx.shared_store.insert(hooks);
    routes().into_app(ctx)
}

#[tokio::main]
async fn main() -> Result<()> {
    let environment = Environment::from_env();
    let (config, hooks) = settings(&environment, |name| std::env::var(name).ok())?;
    logger::init("webhooks", &config.logger);
    let server_config = config.server.clone();
    let router = app(environment, config, hooks)?;
    server::serve(router, &server_config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{HeaderMap, Request},
    };
    use tower::ServiceExt;

    const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

    fn test_app(subscriber: Option<&'static str>) -> axum::Router {
        let lookup = move |name: &str| match name {
            "WEBHOOK_SECRET" => Some(SECRET.to_string()),
            "SUBSCRIBER_URL" => subscriber.map(str::to_string),
            _ => None,
        };
        let (config, hooks) = settings(&Environment::Test, lookup).unwrap();
        app(Environment::Test, config, hooks).unwrap()
    }

    async fn send(app: axum::Router, request: Request<Body>) -> (StatusCode, String) {
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn delivery(headers: HeaderMap, body: &'static str) -> Request<Body> {
        let mut request = Request::post("/hooks/billing")
            .body(Body::from(body))
            .unwrap();
        *request.headers_mut() = headers;
        request
    }

    #[tokio::test]
    async fn a_signed_delivery_is_taken() {
        let key: SigningKey = SECRET.parse().unwrap();
        let body = r#"{"type":"invoice.paid"}"#;
        let headers = Signer::new(key)
            .headers_now("msg_1", body.as_bytes())
            .unwrap();
        let (status, answer) = send(test_app(None), delivery(headers, body)).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        assert_eq!(answer, r#"{"received":"msg_1"}"#);
    }

    #[tokio::test]
    async fn a_delivery_signed_by_another_key_or_changed_is_refused() {
        let other: SigningKey = "whsec_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".parse().unwrap();
        let body = r#"{"type":"invoice.paid"}"#;
        let headers = Signer::new(other)
            .headers_now("msg_1", body.as_bytes())
            .unwrap();
        let (status, answer) = send(test_app(None), delivery(headers, body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(answer.contains("webhook_signature_invalid"), "{answer}");

        let key: SigningKey = SECRET.parse().unwrap();
        let headers = Signer::new(key)
            .headers_now("msg_1", body.as_bytes())
            .unwrap();
        let (status, _) = send(test_app(None), delivery(headers, r#"{"type":"refund"}"#)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, answer) = send(test_app(None), delivery(HeaderMap::new(), body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(answer.contains("webhook_headers_invalid"), "{answer}");
    }

    #[tokio::test]
    async fn a_subscriber_on_a_private_address_is_never_reached() {
        for subscriber in [
            "http://127.0.0.1:9/hooks",
            "http://169.254.169.254/latest/meta-data/",
            "http://localhost:9/hooks",
            "http://[::1]:9/hooks",
        ] {
            let request = Request::post("/api/notify")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"type":"invoice.paid"}"#))
                .unwrap();
            let (status, answer) = send(test_app(Some(subscriber)), request).await;
            assert_eq!(
                status,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{subscriber}: {answer}"
            );
            assert!(answer.contains("subscriber_refused"), "{answer}");
        }
    }

    #[test]
    fn production_refuses_to_boot_without_a_valid_secret() {
        for secret in [None, Some("not-a-key")] {
            let lookup = move |name: &str| match name {
                "WEBHOOK_SECRET" => secret.map(str::to_string),
                _ => None,
            };
            let error = settings(&Environment::Production, lookup).err().unwrap();
            assert!(error.to_string().contains("WEBHOOK_SECRET"), "{error}");
            assert!(!error.to_string().contains("not-a-key"), "{error}");
        }
    }
}
