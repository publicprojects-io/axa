//! Real requests through the layer. The Postgres test is ignored by default. Run it with:
//!
//! ```sh
//! AXA_TEST_DB_URL=postgres://... cargo test -p axa-web-session --all-features -- --ignored
//! ```

use std::net::SocketAddr;

use axa_web_session::{
    Error, MIN_KEY_BYTES, Session, SessionNullPool, SessionOptions, csrf_token, csrf_verify, layer,
    memory_store, sign_in, sign_out,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, RawQuery},
    http::{Request, Response, header},
    routing::get,
};
use tower::ServiceExt;

const SIGNING: [u8; MIN_KEY_BYTES] = [7; MIN_KEY_BYTES];
const DATABASE: [u8; MIN_KEY_BYTES] = [9; MIN_KEY_BYTES];

async fn put(session: Session<SessionNullPool>) -> &'static str {
    sign_in(&session);
    session.set("name", "ada");
    "stored"
}

/// Sets a value and does not sign in.
async fn anonymous(session: Session<SessionNullPool>) -> &'static str {
    session.set("name", "ada");
    "kept for this request only"
}

async fn token(session: Session<SessionNullPool>) -> String {
    csrf_token(&session)
}

async fn check(session: Session<SessionNullPool>, query: RawQuery) -> String {
    csrf_verify(&session, &query.0.unwrap_or_default()).to_string()
}

async fn read(session: Session<SessionNullPool>) -> String {
    session.get::<String>("name").unwrap_or_default()
}

async fn id(session: Session<SessionNullPool>) -> String {
    session.get_session_id()
}

async fn renew(session: Session<SessionNullPool>) -> String {
    session.renew();
    session.get_session_id()
}

async fn destroy(session: Session<SessionNullPool>) -> &'static str {
    sign_out(&session);
    "gone"
}

async fn app(options: &SessionOptions) -> Router {
    let store = memory_store(options).await.unwrap();
    Router::new()
        .route("/put", get(put))
        .route("/read", get(read))
        .route("/id", get(id))
        .route("/renew", get(renew))
        .route("/destroy", get(destroy))
        .route("/anonymous", get(anonymous))
        .route("/token", get(token))
        .route("/check", get(check))
        .layer(layer(store))
}

fn production() -> SessionOptions {
    SessionOptions::production(&SIGNING, &DATABASE).unwrap()
}

/// A browser: it sends the cookies it was given, from one address.
#[derive(Clone)]
struct Browser {
    cookies: Vec<String>,
    ip: [u8; 4],
}

impl Browser {
    fn new() -> Self {
        Self {
            cookies: Vec::new(),
            ip: [203, 0, 113, 1],
        }
    }

    async fn get(&mut self, app: &Router, path: &str) -> String {
        let response = self.send(app, path).await;
        for cookie in response.headers().get_all(header::SET_COOKIE) {
            let cookie = cookie.to_str().unwrap();
            let pair = cookie.split(';').next().unwrap().to_string();
            let name = pair.split('=').next().unwrap().to_string();
            self.cookies
                .retain(|held| !held.starts_with(&format!("{name}=")));
            // As a browser does: this is a deletion.
            if !cookie.contains("Max-Age=0") {
                self.cookies.push(pair);
            }
        }
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    async fn send(&self, app: &Router, path: &str) -> Response<Body> {
        let mut request = Request::builder()
            .uri(path)
            .header(header::USER_AGENT, "test browser")
            .header(header::HOST, "crm.example.org");
        if !self.cookies.is_empty() {
            request = request.header(header::COOKIE, self.cookies.join("; "));
        }
        let mut request = request.body(Body::empty()).unwrap();
        let address = SocketAddr::from((self.ip, 40_000));
        request.extensions_mut().insert(ConnectInfo(address));
        app.clone().oneshot(request).await.unwrap()
    }
}

async fn set_cookies(app: &Router) -> Vec<String> {
    let response = Browser::new().send(app, "/put").await;
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|cookie| cookie.to_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_short_key_is_refused_not_a_panic() {
    let short = [1_u8; MIN_KEY_BYTES - 1];
    assert!(matches!(
        SessionOptions::production(&short, &DATABASE),
        Err(Error::KeyTooShort {
            name: "signing",
            length: 63
        })
    ));
    assert!(matches!(
        SessionOptions::production(&SIGNING, &short),
        Err(Error::KeyTooShort {
            name: "database",
            length: 63
        })
    ));
    assert!(matches!(
        SessionOptions::production(b"", b""),
        Err(Error::KeyTooShort { length: 0, .. })
    ));
}

#[test]
fn the_two_keys_must_differ() {
    assert!(matches!(
        SessionOptions::production(&SIGNING, &SIGNING),
        Err(Error::SameKey)
    ));
}

#[test]
fn debug_output_hides_the_keys() {
    let shown = format!("{:?}", production());
    assert!(shown.contains("signed: true"), "{shown}");
    assert!(!shown.contains("[7,"), "{shown}");
    assert!(!shown.contains("7, 7"), "{shown}");
}

#[tokio::test]
async fn a_production_cookie_is_secure_and_signed() {
    let cookies = set_cookies(&app(&production()).await).await;
    assert_ne!(cookies.len(), 0);
    for cookie in &cookies {
        assert!(cookie.contains("Secure"), "{cookie}");
        assert!(cookie.contains("HttpOnly"), "{cookie}");
        assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    }
}

#[tokio::test]
async fn a_development_cookie_is_neither_secure_nor_signed() {
    let plain = set_cookies(&app(&SessionOptions::development()).await).await;
    let signed = set_cookies(&app(&production()).await).await;
    assert!(
        plain.iter().all(|cookie| !cookie.contains("Secure")),
        "{plain:?}"
    );
    let value = |cookies: &[String]| {
        let cookie = cookies.iter().find(|c| c.starts_with("session=")).unwrap();
        cookie.split(';').next().unwrap().len()
    };
    // A signature makes the value longer than the bare identifier.
    assert!(value(&signed) > value(&plain), "{signed:?} {plain:?}");
}

#[tokio::test]
async fn a_session_survives_between_requests() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/put").await;
    assert_eq!(browser.get(&app, "/read").await, "ada");
    assert_eq!(Browser::new().get(&app, "/read").await, "");
}

/// The browser's session cookie replaced by `value`, keeping any other cookie.
fn with_session_cookie(browser: &Browser, value: &str) -> Browser {
    let mut forged = browser.clone();
    for cookie in &mut forged.cookies {
        if cookie.starts_with("session=") {
            *cookie = format!("session={value}");
        }
    }
    forged
}

/// An attacker who knows a live session's identifier, and so needs only the
/// signature. Only checking it can refuse them.
#[tokio::test]
async fn a_real_id_with_no_signature_reads_nothing() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/put").await;
    let id = browser.get(&app, "/id").await;
    assert_eq!(
        with_session_cookie(&browser, &id).get(&app, "/read").await,
        ""
    );
    assert_eq!(browser.get(&app, "/read").await, "ada");
}

/// Without signing, that same attack works, which shows the test above can fail.
#[tokio::test]
async fn an_unsigned_cookie_can_be_forged_in_development() {
    let app = app(&SessionOptions::development()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/put").await;
    let id = browser.get(&app, "/id").await;
    let mut forged = with_session_cookie(&browser, &id);
    assert_eq!(forged.get(&app, "/read").await, "ada");
}

#[tokio::test]
async fn a_changed_signature_reads_nothing() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/put").await;
    let cookie = browser
        .cookies
        .iter()
        .find(|c| c.starts_with("session="))
        .unwrap();
    let value = cookie.trim_start_matches("session=");
    let first = if value.starts_with('A') { "B" } else { "A" };
    let forged = format!("{first}{}", &value[1..]);
    assert_eq!(
        with_session_cookie(&browser, &forged)
            .get(&app, "/read")
            .await,
        ""
    );
}

#[tokio::test]
async fn renew_changes_the_id_and_keeps_the_data() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/put").await;
    let before = browser.get(&app, "/id").await;
    browser.get(&app, "/renew").await;
    let after = browser.get(&app, "/id").await;
    assert_ne!(before, after);
    assert_eq!(browser.get(&app, "/read").await, "ada");
}

#[tokio::test]
async fn destroy_ends_the_session() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/put").await;
    let stale = browser.clone();
    browser.get(&app, "/destroy").await;
    assert_eq!(browser.get(&app, "/read").await, "");
    assert_eq!(stale.clone().get(&app, "/read").await, "");
}

#[tokio::test]
async fn a_session_survives_a_new_ip_address() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/put").await;
    browser.ip = [198, 51, 100, 9];
    assert_eq!(browser.get(&app, "/read").await, "ada");
}

#[tokio::test]
async fn an_anonymous_visitor_keeps_no_cookie_and_nothing_is_stored() {
    let app = app(&production()).await;
    let mut visitor = Browser::new();
    visitor.get(&app, "/anonymous").await;
    assert_eq!(visitor.cookies, Vec::<String>::new());
    assert_eq!(visitor.get(&app, "/read").await, "");
}

#[tokio::test]
async fn every_visitor_can_be_stored_when_a_service_asks() {
    let app = app(&SessionOptions::development().store_every_visitor()).await;
    let mut visitor = Browser::new();
    visitor.get(&app, "/anonymous").await;
    assert_eq!(visitor.get(&app, "/read").await, "ada");
}

/// Session fixation: an attacker who gave the victim an identifier before
/// sign-in must not be able to use it afterwards.
#[tokio::test]
async fn sign_in_makes_an_identifier_from_before_it_worthless() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    browser.get(&app, "/token").await;
    let planted = browser.clone();
    let before = browser.get(&app, "/id").await;
    browser.get(&app, "/put").await;
    let after = browser.get(&app, "/id").await;
    assert_ne!(before, after);
    assert_eq!(browser.get(&app, "/read").await, "ada");
    assert_eq!(planted.clone().get(&app, "/read").await, "");
}

#[tokio::test]
async fn a_form_token_is_stable_and_only_it_passes() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    let token = browser.get(&app, "/token").await;
    assert_eq!(token.len(), 64);
    assert_eq!(browser.get(&app, "/token").await, token);
    assert_eq!(browser.get(&app, &format!("/check?{token}")).await, "true");
    assert_eq!(browser.get(&app, "/check?wrong").await, "false");
    assert_eq!(browser.get(&app, "/check").await, "false");
}

#[tokio::test]
async fn a_form_token_belongs_to_one_session() {
    let app = app(&production()).await;
    let token = Browser::new().get(&app, "/token").await;
    let mut other = Browser::new();
    other.get(&app, "/token").await;
    assert_eq!(other.get(&app, &format!("/check?{token}")).await, "false");
}

#[tokio::test]
async fn a_session_with_no_token_refuses_every_post() {
    let app = app(&production()).await;
    assert_eq!(Browser::new().get(&app, "/check?").await, "false");
}

#[tokio::test]
async fn sign_in_replaces_the_form_token() {
    let app = app(&production()).await;
    let mut browser = Browser::new();
    let before = browser.get(&app, "/token").await;
    browser.get(&app, "/put").await;
    assert_eq!(
        browser.get(&app, &format!("/check?{before}")).await,
        "false"
    );
    assert_ne!(browser.get(&app, "/token").await, before);
}

#[cfg(feature = "postgres")]
mod postgres {
    use axa_web_session::{SessionPgSession, postgres_store};

    use super::*;

    async fn put(session: SessionPgSession) -> String {
        session.set_store(true);
        session.set("note", "plainly readable");
        session.get_session_id()
    }

    async fn visit(session: SessionPgSession) -> &'static str {
        session.set("note", "anonymous");
        "seen"
    }

    async fn read(session: SessionPgSession) -> String {
        session.get::<String>("note").unwrap_or_default()
    }

    async fn pg_app(pool: &sqlx::PgPool, options: &SessionOptions) -> Router {
        let store = postgres_store(pool.clone(), options).await.unwrap();
        Router::new()
            .route("/put", get(put))
            .route("/read", get(read))
            .route("/visit", get(visit))
            .layer(layer(store))
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DB_URL"]
    async fn anonymous_visitors_add_no_rows() {
        let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let options = production().table_name("axa_test_anonymous");
        let app = pg_app(&pool, &options).await;
        for _ in 0..20 {
            Browser::new().get(&app, "/visit").await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM axa_test_anonymous")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 0);
        sqlx::query("DROP TABLE axa_test_anonymous")
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[ignore = "needs AXA_TEST_DB_URL"]
    async fn postgres_sessions_are_encrypted_and_survive_a_restart() {
        let url = std::env::var("AXA_TEST_DB_URL").expect("AXA_TEST_DB_URL");
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let options = production().table_name("axa_test_sessions");
        let app = pg_app(&pool, &options).await;
        let mut browser = Browser::new();
        let id = browser.get(&app, "/put").await;

        let mut row: Option<String> = None;
        for _ in 0..100 {
            row = sqlx::query_scalar("SELECT session FROM axa_test_sessions WHERE id = $1")
                .bind(&id)
                .fetch_optional(&pool)
                .await
                .unwrap();
            if row.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let row = row.expect("the session reached the table");
        assert!(!row.contains("plainly readable"), "{row}");

        // A new store has nothing in memory, so this comes from the table.
        let restarted = pg_app(&pool, &options).await;
        assert_eq!(browser.get(&restarted, "/read").await, "plainly readable");
        sqlx::query("DROP TABLE axa_test_sessions")
            .execute(&pool)
            .await
            .unwrap();
    }
}
