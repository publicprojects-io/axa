//! Real responses checked against the `OpenAPI` document, with the `openapi` feature.
//!
//! A drift check, such as `axa_openapi_docs::drift`, says each route is documented. This
//! says each answer is: that its status is one the operation lists, its content type is
//! one that status lists, and a JSON body is valid against the schema documented for it.
//!
//! ```ignore
//! use axa_test::openapi::Conformance;
//!
//! request::<App, _, _>(|server, _ctx| async move {
//!     let conformance = Conformance::new(serde_json::to_value(ApiDoc::openapi()).unwrap());
//!     conformance.assert(&server.get("/api/places/1").await);
//!     conformance.assert(&server.get("/api/places/missing").await);
//! })
//! .await;
//! ```
//!
//! The document is `OpenAPI` 3.1, as `utoipa` writes, whose schemas are JSON Schema
//! 2020-12. A 3.0 document's schemas are another dialect, so it is refused rather than
//! checked wrongly. A `$ref` must point inside the document: nothing is fetched.

use std::fmt::Write as _;

use axum_test::TestResponse;
use serde_json::Value;

/// The most errors reported for one body, so that a long list stays readable.
const MAX_ERRORS: usize = 10;

/// How many `$ref`s are followed in a row before a loop is assumed.
const MAX_REFS: usize = 16;

/// An `OpenAPI` 3.1 document to check responses against.
#[derive(Clone, Debug)]
pub struct Conformance {
    document: Value,
}

impl Conformance {
    /// Check against `document`.
    ///
    /// # Panics
    ///
    /// If `document` is not `OpenAPI` 3.1.
    #[must_use]
    pub fn new(document: Value) -> Self {
        let version = document["openapi"].as_str().unwrap_or_default();
        assert!(
            version.starts_with("3.1."),
            "axa_test::openapi checks OpenAPI 3.1 documents, and this is {version:?}"
        );
        Self { document }
    }

    /// Panic, naming each mismatch, unless `response` is one its operation documents.
    ///
    /// # Panics
    ///
    /// When it is not.
    pub fn assert(&self, response: &TestResponse) {
        let method = response.request_method();
        let url = response.request_url();
        let status = response.status_code().as_u16();
        let content_type = response.maybe_content_type();
        if let Err(mismatches) = self.check(
            method.as_str(),
            url.path(),
            status,
            content_type.as_deref(),
            response.as_bytes(),
        ) {
            panic!(
                "{method} {} answered {status}, which the OpenAPI document does not allow:\n  {}",
                url.path(),
                mismatches.join("\n  ")
            );
        }
    }

    /// Whether a response to `method` on `path`, a request's path such as
    /// `/api/places/1`, with `status`, `content_type`, and `body`, is one the operation
    /// documents.
    ///
    /// # Errors
    ///
    /// Each way it is not.
    pub fn check(
        &self,
        method: &str,
        path: &str,
        status: u16,
        content_type: Option<&str>,
        body: &[u8],
    ) -> Result<(), Vec<String>> {
        let fail = |mismatch: String| Err(vec![mismatch]);
        let Some(template) = self.template(path) else {
            return fail(format!("no path in the document matches {path}"));
        };
        let mut at = format!("/paths/{}", escape(&template));
        let Some(operation) = self.follow(&mut at) else {
            return fail(format!("{template} cannot be read"));
        };
        let method = method.to_ascii_lowercase();
        if operation.get(&method).is_none() {
            return fail(format!("{template} documents no {method} operation"));
        }
        at = format!("{at}/{method}/responses");
        let Some(responses) = self.follow(&mut at) else {
            return fail(format!("{method} {template} documents no responses"));
        };
        let exact = status.to_string();
        let range = format!("{}XX", status / 100);
        let Some(key) = [exact.as_str(), range.as_str(), "default"]
            .into_iter()
            .find(|key| responses.get(key).is_some())
        else {
            return fail(format!("{method} {template} does not document {status}"));
        };
        at = format!("{at}/{key}");
        let Some(response) = self.follow(&mut at) else {
            return fail(format!("{method} {template} {key} cannot be read"));
        };
        let Some(content) = response.get("content").and_then(Value::as_object) else {
            return if body.is_empty() {
                Ok(())
            } else {
                fail(format!(
                    "{method} {template} {key} documents no body, and one was sent"
                ))
            };
        };
        let essence = content_type
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let Some(media) = media_key(content, &essence) else {
            let documented: Vec<&str> = content.keys().map(String::as_str).collect();
            return fail(format!(
                "{method} {template} {key} documents {documented:?}, and the answer was {essence:?}"
            ));
        };
        at = format!("{at}/content/{}", escape(media));
        if content[media].get("schema").is_none() || !is_json(&essence) {
            return Ok(());
        }
        let instance: Value = match serde_json::from_slice(body) {
            Ok(instance) => instance,
            Err(error) => return fail(format!("the body is not JSON: {error}")),
        };
        self.validate(&format!("{at}/schema"), &instance)
    }

    /// The documented path that `path` is, the one with the most literal segments when
    /// more than one matches, as `/places/new` is before `/places/{id}`.
    fn template(&self, path: &str) -> Option<String> {
        let wanted: Vec<&str> = path.split('/').collect();
        self.document["paths"]
            .as_object()?
            .keys()
            .filter_map(|template| {
                let parts: Vec<&str> = template.split('/').collect();
                if parts.len() != wanted.len() {
                    return None;
                }
                let mut literal = 0;
                for (part, segment) in parts.iter().zip(&wanted) {
                    if part.starts_with('{') && part.ends_with('}') {
                        if segment.is_empty() {
                            return None;
                        }
                    } else if part == segment {
                        literal += 1;
                    } else {
                        return None;
                    }
                }
                Some((literal, template))
            })
            .max_by_key(|(literal, _)| *literal)
            .map(|(_, template)| template.clone())
    }

    /// The value at the JSON pointer `at`, following any `$ref`s, which move `at` to
    /// where they point, so a schema inside is found where it really is.
    fn follow(&self, at: &mut String) -> Option<&Value> {
        let mut value = self.document.pointer(at)?;
        for _ in 0..MAX_REFS {
            let Some(target) = value.get("$ref").and_then(Value::as_str) else {
                return Some(value);
            };
            let pointer = decode(target.strip_prefix('#')?)?;
            value = self.document.pointer(&pointer)?;
            *at = pointer;
        }
        None
    }

    /// Validate `instance` against the schema at `at`. The schema is the whole document
    /// with a `$ref` to it, so its own `$ref`s into `#/components` resolve.
    fn validate(&self, at: &str, instance: &Value) -> Result<(), Vec<String>> {
        let mut root = self.document.clone();
        let Some(object) = root.as_object_mut() else {
            return Err(vec!["the document is not an object".to_string()]);
        };
        object.insert(
            "$ref".to_string(),
            Value::String(format!("#{}", fragment(at))),
        );
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .should_validate_formats(true)
            .build(&root)
            .map_err(|error| vec![format!("the schema at {at} cannot be used: {error}")])?;
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .take(MAX_ERRORS)
            .map(|error| {
                let path = error.instance_path().to_string();
                let path = if path.is_empty() { "/" } else { &path };
                format!("{path}: {error}")
            })
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// The documented media type an answer of `essence` matches: itself, its type with `*`,
/// or `*/*`.
fn media_key<'a>(content: &'a serde_json::Map<String, Value>, essence: &str) -> Option<&'a str> {
    let wildcard = essence
        .split_once('/')
        .map(|(kind, _)| format!("{kind}/*"))
        .unwrap_or_default();
    [essence, wildcard.as_str(), "*/*"]
        .into_iter()
        .find_map(|wanted| {
            content
                .keys()
                .find(|key| key.eq_ignore_ascii_case(wanted))
                .map(String::as_str)
        })
}

/// Whether a media type is JSON: `application/json`, or a `+json` type such as
/// `application/problem+json`.
fn is_json(essence: &str) -> bool {
    essence == "application/json" || essence.ends_with("+json")
}

/// A JSON pointer as a URI fragment, with what a fragment cannot hold, such as the `{`
/// of a path template, percent-encoded.
fn fragment(pointer: &str) -> String {
    let mut encoded = String::with_capacity(pointer.len());
    for byte in pointer.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// A URI fragment's percent-encoding undone, or `None` for a bad one.
fn decode(fragment: &str) -> Option<String> {
    let bytes = fragment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// A JSON pointer segment.
fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "openapi": "3.1.0",
            "info": { "title": "Places", "version": "1" },
            "paths": {
                "/api/places/{id}": {
                    "get": {
                        "responses": {
                            "200": {
                                "description": "A place",
                                "content": { "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Place" }
                                } }
                            },
                            "404": { "$ref": "#/components/responses/Problem" }
                        }
                    },
                    "delete": { "responses": { "204": { "description": "Gone" } } }
                },
                "/api/places/new": {
                    "get": { "responses": { "2XX": {
                        "description": "A form",
                        "content": { "text/html": {} }
                    } } }
                }
            },
            "components": {
                "schemas": {
                    "Place": {
                        "type": "object",
                        "required": ["id", "name"],
                        "properties": {
                            "id": { "type": "string", "format": "uuid" },
                            "name": { "type": "string" },
                            "tags": { "type": "array", "items": { "type": "string" } }
                        },
                        "additionalProperties": false
                    }
                },
                "responses": {
                    "Problem": {
                        "description": "A problem",
                        "content": { "application/problem+json": { "schema": {
                            "type": "object",
                            "required": ["status"],
                            "properties": { "status": { "type": "integer" } }
                        } } }
                    }
                }
            }
        })
    }

    const ID: &str = "8f2b0c1e-4a0e-4a39-9a4b-2f1f7c2d9d10";

    fn check(
        method: &str,
        path: &str,
        status: u16,
        kind: Option<&str>,
        body: &Value,
    ) -> Result<(), Vec<String>> {
        let body = if body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(body).unwrap()
        };
        Conformance::new(document()).check(method, path, status, kind, &body)
    }

    const JSON: Option<&str> = Some("application/json; charset=utf-8");

    #[test]
    fn a_documented_answer_conforms() {
        let place = json!({ "id": ID, "name": "Home", "tags": ["a"] });
        assert_eq!(check("GET", "/api/places/1", 200, JSON, &place), Ok(()));
        // Through a `$ref` to a shared response.
        let problem = json!({ "status": 404, "title": "Not Found" });
        let kind = Some("application/problem+json");
        assert_eq!(check("GET", "/api/places/1", 404, kind, &problem), Ok(()));
        assert_eq!(
            check("DELETE", "/api/places/1", 204, None, &Value::Null),
            Ok(())
        );
        // A literal path wins over a template, and a range covers its statuses.
        let html = Conformance::new(document()).check(
            "GET",
            "/api/places/new",
            201,
            Some("text/html"),
            b"<p>",
        );
        assert_eq!(html, Ok(()));
    }

    #[test]
    fn a_body_outside_its_schema_is_named_by_where_it_is_wrong() {
        let wrong = json!({ "id": "not-a-uuid", "tags": [1], "extra": true });
        let errors = check("GET", "/api/places/1", 200, JSON, &wrong).unwrap_err();
        let errors = errors.join("\n");
        for part in ["/id", "/tags/0", "name", "extra"] {
            assert!(errors.contains(part), "{part} in\n{errors}");
        }
    }

    #[test]
    fn an_undocumented_status_path_method_or_type_does_not_conform() {
        let place = json!({ "id": ID, "name": "Home" });
        let mismatch = |result: Result<(), Vec<String>>| result.unwrap_err().join("\n");
        assert!(
            mismatch(check("GET", "/api/places/1", 500, JSON, &place))
                .contains("does not document 500")
        );
        assert!(mismatch(check("GET", "/api/people/1", 200, JSON, &place)).contains("no path"));
        assert!(mismatch(check("PUT", "/api/places/1", 200, JSON, &place)).contains("no put"));
        assert!(
            mismatch(check(
                "GET",
                "/api/places/1",
                200,
                Some("text/plain"),
                &place
            ))
            .contains("text/plain")
        );
        assert!(mismatch(check("DELETE", "/api/places/1", 204, None, &place)).contains("no body"));
        let broken = Conformance::new(document()).check("GET", "/api/places/1", 200, JSON, b"{");
        assert!(mismatch(broken).contains("not JSON"));
    }

    fn server() -> axum_test::TestServer {
        use axum::{Json, Router, routing::get};
        let app = Router::new()
            .route(
                "/api/places/{id}",
                get(|| async { Json(json!({ "id": ID, "name": "Home" })) }),
            )
            .route(
                "/api/places/new",
                get(|| async { Json(json!({ "id": 1 })) }),
            );
        axum_test::TestServer::new(app)
    }

    #[tokio::test]
    async fn a_real_response_is_checked_by_its_request() {
        let server = server();
        Conformance::new(document()).assert(&server.get("/api/places/1").await);
    }

    #[tokio::test]
    #[should_panic(expected = "GET /api/places/new answered 200")]
    async fn a_real_response_that_does_not_conform_fails_the_test() {
        let server = server();
        Conformance::new(document()).assert(&server.get("/api/places/new").await);
    }

    #[test]
    #[should_panic(expected = "checks OpenAPI 3.1 documents")]
    fn an_openapi_3_0_document_is_refused() {
        let _ = Conformance::new(json!({ "openapi": "3.0.3", "paths": {} }));
    }
}
