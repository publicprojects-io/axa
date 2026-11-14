//! Maud compile-time HTML templates with safe escaping and generic errors.
//!
//! A template is Rust code, written with [`html!`], so the compiler checks it.
//! `html!` expands to code that names the `maud` crate, so a service depends on
//! `maud` too, at the version this crate uses, 0.27, so that its `Markup` is this
//! crate's:
//!
//! ```toml
//! [dependencies]
//! axa = { version = "0.13", features = ["maud"] }
//! maud = "0.27"
//! ```
//!
//! ```
//! use axa_maud_templates::{Page, html};
//!
//! let name = "<b>Alice</b>";
//! let page = Page::from(html! { p { "Hello, " (name) "!" } });
//! assert_eq!(page.0.into_string(), "<p>Hello, &lt;b&gt;Alice&lt;/b&gt;!</p>");
//! ```
//!
//! # What this crate promises
//!
//! - **HTML is escaped by default.** Every value spliced into [`html!`] is escaped,
//!   in text and in attributes, unless it is wrapped in [`PreEscaped`]. Only wrap
//!   markup your own code produced, because it turns the protection off.
//! - **A template is checked when it compiles.** A syntax error, a missing value,
//!   or a value of the wrong type is a compile error, not a failure at run time or
//!   blank output.
//! - **A failed handler shows nothing.** A handler that returns
//!   `Result<Page, Error>` turns any error into [`Error`] with `?`. The failure is
//!   logged and the visitor gets a generic `500`, as an error can quote what it was
//!   working on.
//! - **A page can load its own files.** `axa`'s production default,
//!   `default-src 'none'`, suits a JSON API and would block a page's stylesheets
//!   and images. [`Page`] sends [`PAGE_CSP`] in its place, and a handler or
//!   `HTTP_CONTENT_SECURITY_POLICY` can name another. It allows no inline
//!   script, and files only from the page's own origin.
//!
//! [`Markup`] alone does not answer as a response here: wrap it in [`Page`], so
//! that every page sends its policy.
//!
//! ```
//! use axa_maud_templates::{DOCTYPE, Error, Markup, Page, html};
//!
//! fn layout(title: &str, body: Markup) -> Markup {
//!     html! {
//!         (DOCTYPE)
//!         html lang="en" {
//!             head { meta charset="utf-8"; title { (title) } }
//!             body { (body) }
//!         }
//!     }
//! }
//!
//! async fn hello() -> Result<Page, Error> {
//!     let count: u32 = "3".parse()?;
//!     Ok(Page::from(layout("Hello", html! { p { (count) " visits" } })))
//! }
//! ```

pub use maud::{self, DOCTYPE, Markup, PreEscaped, Render, html};

use axum::{
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};

/// The policy a [`Page`] sends: its own origin's files, and no inline script.
pub const PAGE_CSP: &str = "default-src 'self'; object-src 'none'; base-uri 'self'; \
     form-action 'self'; frame-ancestors 'none'";

/// A rendered HTML page, which answers with [`PAGE_CSP`].
#[derive(Debug, Clone)]
pub struct Page(pub Markup);

impl From<Markup> for Page {
    fn from(markup: Markup) -> Self {
        Self(markup)
    }
}

impl IntoResponse for Page {
    fn into_response(self) -> Response {
        (
            [(header::CONTENT_SECURITY_POLICY, PAGE_CSP)],
            Html(self.0.into_string()),
        )
            .into_response()
    }
}

/// A failure while making a page, from any error, through `?`.
///
/// It answers a generic `500` in HTML and logs what failed. Like `anyhow::Error`,
/// it does not itself implement [`std::error::Error`], so that `?` can turn any
/// error into it.
pub struct Error(Box<dyn std::error::Error + Send + Sync>);

impl Error {
    /// An error with this message, for a failure that is not already an error.
    #[must_use]
    pub fn msg(message: impl Into<String>) -> Self {
        Self(message.into().into())
    }
}

impl<E: Into<Box<dyn std::error::Error + Send + Sync>>> From<E> for Error {
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        tracing::error!(error = %self, "page failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("Internal Server Error"),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn values_render() {
        let name = "Alice";
        assert_eq!(html! { p { (name) } }.into_string(), "<p>Alice</p>");
    }

    #[test]
    fn values_are_escaped_in_text_and_attributes() {
        let value = "\"><script>alert(1)</script>";
        let page = html! { a title=(value) { (value) } }.into_string();
        assert!(!page.contains("<script>"), "{page}");
        assert!(page.contains("&lt;script&gt;"), "{page}");
        assert!(page.contains("&quot;&gt;"), "{page}");
    }

    #[test]
    fn pre_escaped_turns_escaping_off() {
        let page = html! { p { (PreEscaped("<b>")) } }.into_string();
        assert_eq!(page, "<p><b></p>");
    }

    #[tokio::test]
    async fn a_page_is_html_with_its_own_policy() {
        let response = Page::from(html! { p { "hi" } }).into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response.headers()[header::CONTENT_TYPE].to_str().unwrap();
        assert!(content_type.starts_with("text/html"), "{content_type}");
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            PAGE_CSP
        );
        assert_eq!(body_text(response).await, "<p>hi</p>");
    }

    #[tokio::test]
    async fn a_failed_page_shows_nothing_to_the_visitor() {
        fn handler() -> Result<Page, Error> {
            let secret: u32 = "secret-value".parse()?;
            Ok(Page::from(html! { (secret) }))
        }
        let error = handler().unwrap_err();
        assert!(error.to_string().contains("invalid digit"), "{error}");
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        let body = body_text(response).await;
        assert_eq!(body, "Internal Server Error");
    }

    #[test]
    fn an_error_can_be_a_message() {
        assert_eq!(Error::msg("no such page").to_string(), "no such page");
    }
}
