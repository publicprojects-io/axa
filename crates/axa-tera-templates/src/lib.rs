//! Tera templates with safe escaping and generic errors.
//!
//! ```
//! use axa_tera_templates::Templates;
//! use serde_json::json;
//!
//! let templates =
//!     Templates::from_raw([("hello.html", "<p>Hello, {{ name }}!</p>")]).unwrap();
//!
//! let page = templates
//!     .render("hello.html", &json!({ "name": "<b>Alice</b>" }))
//!     .unwrap();
//! assert!(page.starts_with("<p>Hello, &lt;b&gt;Alice"));
//! ```
//!
//! # What this crate promises
//!
//! - **HTML is escaped by default.** Templates named `.html`, `.htm`, or `.xml`
//!   escape every value unless the template says `safe`. Only write `safe` on
//!   a value your own code produced, because it turns the protection off.
//! - **A failed render shows nothing.** The failure is logged and the visitor
//!   gets a generic `500`, because Tera's errors quote template source.
//! - **A page can load its own files.** `axa`'s production default,
//!   `default-src 'none'`, suits a JSON API and would block a page's stylesheets
//!   and images. [`Page`] sends [`PAGE_CSP`] in its place, and a handler or
//!   `HTTP_CSP` can name another. It allows no inline script, and files only from
//!   the page's own origin.
//!
//! A template name without one of those extensions is not escaped, so never
//! render user input into a `.txt` template that ends up in a page.

use axum::{
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use serde::Serialize;
use tera::{Context, Tera};

/// The policy a [`Page`] sends: its own origin's files, and no inline script.
pub const PAGE_CSP: &str = "default-src 'self'; object-src 'none'; base-uri 'self'; \
     form-action 'self'; frame-ancestors 'none'";

/// A rendered HTML page, which answers with [`PAGE_CSP`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page(pub String);

impl IntoResponse for Page {
    fn into_response(self) -> Response {
        ([(header::CONTENT_SECURITY_POLICY, PAGE_CSP)], Html(self.0)).into_response()
    }
}

/// A template failure.
#[derive(Debug)]
pub struct Error(tera::Error);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<tera::Error> for Error {
    fn from(error: tera::Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        tracing::error!(error = %self, "template failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html("Internal Server Error"),
        )
            .into_response()
    }
}

/// A set of compiled templates. Build it once at boot and share it.
#[derive(Debug, Clone)]
pub struct Templates {
    tera: Tera,
}

impl Templates {
    /// Load every template matching `glob`, such as `templates/**/*.html`.
    ///
    /// # Errors
    ///
    /// Fails when a template does not parse.
    pub fn from_glob(glob: &str) -> Result<Self, Error> {
        let mut tera = Tera::new();
        tera.load_from_glob(glob)?;
        Ok(Self { tera })
    }

    /// Compile templates from `(name, source)` pairs, such as ones embedded
    /// with `include_str!`.
    ///
    /// # Errors
    ///
    /// Fails when a template does not parse.
    pub fn from_raw<'a>(
        templates: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Result<Self, Error> {
        let mut tera = Tera::new();
        tera.add_raw_templates(templates)?;
        Ok(Self { tera })
    }

    /// Whether a template exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tera.contains_template(name)
    }

    /// Render a template to text. `data` must serialize to a JSON object.
    ///
    /// # Errors
    ///
    /// Fails when the template is missing, `data` is not an object, or the
    /// template fails, for example on an undefined variable.
    pub fn render<T: Serialize + ?Sized>(&self, name: &str, data: &T) -> Result<String, Error> {
        let context = Context::from_serialize(data)?;
        Ok(self.tera.render(name, &context)?)
    }

    /// Render a template as an HTML response with [`PAGE_CSP`].
    ///
    /// # Errors
    ///
    /// See [`Templates::render`].
    pub fn html<T: Serialize + ?Sized>(&self, name: &str, data: &T) -> Result<Page, Error> {
        self.render(name, data).map(Page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn templates() -> Templates {
        Templates::from_raw([
            ("page.html", "<p>{{ name }}</p>"),
            ("trusted.html", "<p>{{ name | safe }}</p>"),
            ("note.txt", "{{ name }}"),
        ])
        .unwrap()
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn values_render() {
        let page = templates().render("page.html", &json!({ "name": "Alice" }));
        assert_eq!(page.unwrap(), "<p>Alice</p>");
    }

    #[test]
    fn html_templates_escape_values() {
        let data = json!({ "name": "<script>alert(1)</script>" });
        let page = templates().render("page.html", &data).unwrap();
        assert!(!page.contains("<script>"), "{page}");
        assert!(page.contains("&lt;script&gt;"), "{page}");
    }

    #[test]
    fn safe_turns_escaping_off() {
        let page = templates().render("trusted.html", &json!({ "name": "<b>" }));
        assert_eq!(page.unwrap(), "<p><b></p>");
    }

    #[test]
    fn other_extensions_are_not_escaped() {
        let note = templates().render("note.txt", &json!({ "name": "<b>" }));
        assert_eq!(note.unwrap(), "<b>");
    }

    #[test]
    fn a_missing_variable_is_an_error_rather_than_blank() {
        assert!(templates().render("page.html", &json!({})).is_err());
    }

    #[test]
    fn a_missing_template_is_an_error() {
        assert!(templates().render("nope.html", &json!({})).is_err());
        assert!(!templates().contains("nope.html"));
        assert!(templates().contains("page.html"));
    }

    #[test]
    fn data_must_be_an_object() {
        assert!(templates().render("page.html", &json!(["list"])).is_err());
    }

    #[test]
    fn a_syntax_error_is_found_at_boot() {
        assert!(Templates::from_raw([("bad.html", "{{ unclosed")]).is_err());
    }

    #[tokio::test]
    async fn the_html_response_has_the_html_media_type() {
        let response = templates()
            .html("page.html", &json!({ "name": "Alice" }))
            .unwrap()
            .into_response();
        let content_type = response.headers()["content-type"].to_str().unwrap();
        assert!(content_type.starts_with("text/html"), "{content_type}");
    }

    #[tokio::test]
    async fn a_failed_render_shows_nothing_to_the_visitor() {
        let template = "<!-- secret-comment -->{{ missing }}";
        let templates = Templates::from_raw([("t.html", template)]).unwrap();
        let error = templates.html("t.html", &json!({})).unwrap_err();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(body_text(response).await, "Internal Server Error");
    }
}
