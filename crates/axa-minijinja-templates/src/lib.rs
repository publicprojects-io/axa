//! MiniJinja templates with safe escaping, strict undefined values, and generic
//! errors.
//!
//! ```
//! use axa_minijinja_templates::Templates;
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
//! - **HTML is escaped by default.** Templates named `.html`, `.htm`, or `.xml`,
//!   or those names followed by `.j2`, `.jinja`, or `.jinja2`, escape every
//!   value unless the template says `safe`. Only write `safe` on a value your
//!   own code produced, because it turns the protection off.
//! - **A missing value is an error, not blank output.** Printing, looping over,
//!   or reading a field of an undefined value fails the render. Testing one, as
//!   in `{% if error %}`, is allowed and is false, and `default` still works,
//!   so an optional value needs no placeholder.
//! - **Every template is parsed at boot.** [`Templates::from_dir`] reads and
//!   compiles every file up front, so a syntax error stops the service from
//!   starting rather than failing a visitor's request later.
//! - **A failed render shows nothing.** The failure is logged and the visitor
//!   gets a generic `500`, because MiniJinja's errors quote template source.
//! - **A page can load its own files.** `axa`'s production default,
//!   `default-src 'none'`, suits a JSON API and would block a page's stylesheets
//!   and images. [`Page`] sends [`PAGE_CSP`] in its place, and a handler or
//!   `HTTP_CONTENT_SECURITY_POLICY` can name another. It allows no inline
//!   script, and files only from the page's own origin.
//!
//! A template name without one of those extensions is not escaped, so never
//! render user input into a `.txt` template that ends up in a page.

use std::path::Path;

use axum::{
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
};
use minijinja::{Environment, ErrorKind, UndefinedBehavior, value::Serde};
use serde::Serialize;

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

/// A template failure: one that does not parse, a failed render, or a
/// template directory that cannot be read.
#[derive(Debug)]
pub struct Error(minijinja::Error);

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

impl From<minijinja::Error> for Error {
    fn from(error: minijinja::Error) -> Self {
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
    env: Environment<'static>,
}

impl Templates {
    fn environment() -> Environment<'static> {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
        env
    }

    /// Load and compile every file under `dir`, named by its path inside
    /// `dir` with `/` between parts, such as `admin/users.html`. A file or
    /// folder whose name starts with a dot is skipped.
    ///
    /// # Errors
    ///
    /// Fails when `dir` or a file in it cannot be read, a file is not UTF-8,
    /// or a template does not parse.
    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let mut env = Self::environment();
        load_dir(&mut env, dir.as_ref(), "")?;
        Ok(Self { env })
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
        let mut env = Self::environment();
        for (name, source) in templates {
            env.add_template_owned(name.to_string(), source.to_string())?;
        }
        Ok(Self { env })
    }

    /// Whether a template exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.env.get_template(name).is_ok()
    }

    /// Render a template to text. `data` must serialize to a map, such as a
    /// struct or a JSON object.
    ///
    /// # Errors
    ///
    /// Fails when the template is missing, `data` is not a map, or the
    /// template fails, for example on an undefined value.
    pub fn render<T: Serialize + ?Sized>(&self, name: &str, data: &T) -> Result<String, Error> {
        let context = minijinja::Value::from(Serde(data));
        if context.kind() != minijinja::value::ValueKind::Map {
            return Err(minijinja::Error::new(
                ErrorKind::InvalidOperation,
                "template data must serialize to a map",
            )
            .into());
        }
        Ok(self.env.get_template(name)?.render(context)?)
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

fn load_dir(env: &mut Environment<'static>, dir: &Path, prefix: &str) -> Result<(), Error> {
    let unreadable = |path: &Path, error: std::io::Error| {
        minijinja::Error::new(
            ErrorKind::InvalidOperation,
            format!("cannot read {}", path.display()),
        )
        .with_source(error)
    };
    let mut entries = std::fs::read_dir(dir)
        .map_err(|e| unreadable(dir, e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| unreadable(dir, e))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            return Err(minijinja::Error::new(
                ErrorKind::InvalidOperation,
                format!("{} is not a UTF-8 name", path.display()),
            )
            .into());
        };
        if file_name.starts_with('.') {
            continue;
        }
        let name = format!("{prefix}{file_name}");
        if path.is_dir() {
            load_dir(env, &path, &format!("{name}/"))?;
        } else {
            let source = std::fs::read_to_string(&path).map_err(|e| unreadable(&path, e))?;
            env.add_template_owned(name, source)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn templates() -> Templates {
        Templates::from_raw([
            ("page.html", "<p>{{ name }}</p>"),
            ("page.html.j2", "<p>{{ name }}</p>"),
            ("trusted.html", "<p>{{ name | safe }}</p>"),
            ("note.txt", "{{ name }}"),
            (
                "optional.html",
                "{% if error %}<p>{{ error }}</p>{% endif %}ok",
            ),
            ("fallback.html", "{{ name | default('nobody') }}"),
            ("list.html", "{% for item in items %}{{ item }}{% endfor %}"),
            ("field.html", "{{ user.name }}"),
        ])
        .unwrap()
    }

    async fn body_text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "axa-minijinja-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn values_render() {
        let page = templates().render("page.html", &json!({ "name": "Alice" }));
        assert_eq!(page.unwrap(), "<p>Alice</p>");
    }

    #[test]
    fn html_templates_escape_values() {
        let data = json!({ "name": "<script>alert(1)</script>" });
        for name in ["page.html", "page.html.j2"] {
            let page = templates().render(name, &data).unwrap();
            assert!(!page.contains("<script>"), "{name}: {page}");
            assert!(page.contains("&lt;script&gt;"), "{name}: {page}");
        }
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
    fn a_missing_value_is_an_error_rather_than_blank() {
        let t = templates();
        assert!(t.render("page.html", &json!({})).is_err());
        assert!(t.render("list.html", &json!({})).is_err());
        assert!(t.render("field.html", &json!({})).is_err());
    }

    #[test]
    fn a_missing_value_can_be_tested_or_defaulted() {
        let t = templates();
        assert_eq!(t.render("optional.html", &json!({})).unwrap(), "ok");
        assert_eq!(t.render("fallback.html", &json!({})).unwrap(), "nobody");
    }

    #[test]
    fn a_missing_template_is_an_error() {
        assert!(templates().render("nope.html", &json!({})).is_err());
        assert!(!templates().contains("nope.html"));
        assert!(templates().contains("page.html"));
    }

    #[test]
    fn data_must_be_a_map() {
        assert!(templates().render("page.html", &json!(["list"])).is_err());
        assert!(templates().render("page.html", &json!("text")).is_err());
    }

    #[test]
    fn a_syntax_error_is_found_at_boot() {
        assert!(Templates::from_raw([("bad.html", "{{ unclosed")]).is_err());
    }

    #[test]
    fn a_directory_loads_by_relative_name_and_skips_dot_files() {
        let dir = scratch_dir("load");
        std::fs::create_dir_all(dir.join("admin")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(
            dir.join("base.html"),
            "<h1>{% block t %}{% endblock %}</h1>",
        )
        .unwrap();
        std::fs::write(
            dir.join("admin/users.html"),
            "{% extends 'base.html' %}{% block t %}{{ name }}{% endblock %}",
        )
        .unwrap();
        std::fs::write(dir.join(".hidden.html"), "{{ unclosed").unwrap();
        std::fs::write(dir.join(".git/HEAD"), "{{ unclosed").unwrap();

        let templates = Templates::from_dir(&dir).unwrap();
        let page = templates.render("admin/users.html", &json!({ "name": "<i>" }));
        assert_eq!(page.unwrap(), "<h1>&lt;i&gt;</h1>");
        assert!(!templates.contains(".hidden.html"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_syntax_error_in_a_directory_is_found_at_boot() {
        let dir = scratch_dir("bad");
        std::fs::create_dir_all(dir.join("deep")).unwrap();
        std::fs::write(dir.join("deep/bad.html"), "{% if %}").unwrap();
        assert!(Templates::from_dir(&dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(Templates::from_dir(&dir).is_err(), "a missing directory");
    }

    #[test]
    fn an_error_says_what_minijinja_said_and_keeps_its_cause() {
        use std::error::Error as _;
        let dir = scratch_dir("missing");
        std::fs::remove_dir_all(&dir).unwrap();
        let error = Templates::from_dir(&dir).unwrap_err();
        assert!(error.to_string().contains("cannot read"), "{error}");
        let source = error.source().expect("minijinja's error");
        assert_eq!(source.to_string(), error.to_string());
        let io = source.source().expect("the directory's own error");
        assert!(io.downcast_ref::<std::io::Error>().is_some(), "{io}");
    }

    #[tokio::test]
    async fn the_html_response_has_the_html_media_type_and_page_policy() {
        let response = templates()
            .html("page.html", &json!({ "name": "Alice" }))
            .unwrap()
            .into_response();
        let content_type = response.headers()["content-type"].to_str().unwrap();
        assert!(content_type.starts_with("text/html"), "{content_type}");
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            PAGE_CSP
        );
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
