//! Route listing and drift checks: run [`drift`] and [`dangling_refs`] in a test, so a route cannot
//! be added, removed, or renamed without the document following.
//!
//! ```
//! use axa_openapi::{dangling_refs, drift};
//! use serde_json::json;
//!
//! // What `AppRoutes::mounted` gives.
//! let routes = [("GET".to_string(), "/api/things/{pid}".to_string())];
//! let document = json!({ "paths": { "/api/things/{id}": { "get": {} } } });
//!
//! assert!(drift(&routes, &document, &[]).is_empty());
//! assert!(dangling_refs(&document).is_empty());
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// A path with its parameter names removed: `/things/{pid}` and `/things/{id}` are one route.
#[must_use]
pub fn shape(path: &str) -> String {
    let mut shaped = String::with_capacity(path.len());
    let mut inside = false;
    for character in path.chars() {
        match character {
            '{' => {
                inside = true;
                shaped.push_str("{}");
            }
            '}' => inside = false,
            _ if !inside => shaped.push(character),
            _ => {}
        }
    }
    shaped
}

/// Where a document and the mounted routes disagree. Each entry reads `get /things/{}`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Drift {
    /// Mounted, and missing from the document.
    pub undocumented: Vec<String>,
    /// In the document, and not mounted.
    pub unmounted: Vec<String>,
}

impl Drift {
    /// Whether the two agree.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.undocumented.is_empty() && self.unmounted.is_empty()
    }
}

/// Keys of an `OpenAPI` path item that are not operations.
const NOT_OPERATIONS: [&str; 5] = ["description", "parameters", "servers", "summary", "$ref"];

type Operations = BTreeMap<String, BTreeSet<String>>;

fn missing_from(from: &Operations, other: &Operations) -> Vec<String> {
    from.iter()
        .flat_map(|(path, methods)| methods.iter().map(move |method| (path, method)))
        .filter(|(path, method)| !other.get(*path).is_some_and(|m| m.contains(*method)))
        .map(|(path, method)| format!("{method} {path}"))
        .collect()
}

/// Compare the mounted `routes`, as `(method, full path)` from `AppRoutes::mounted`, with
/// `document`'s `paths`, ignoring parameter names. `not_documented` lists paths a service keeps
/// out on purpose, such as its health probes.
#[must_use]
pub fn drift(routes: &[(String, String)], document: &Value, not_documented: &[&str]) -> Drift {
    let skipped: BTreeSet<String> = not_documented.iter().map(|path| shape(path)).collect();
    let mut mounted = Operations::new();
    for (method, path) in routes {
        let path = shape(path);
        if !skipped.contains(&path) {
            mounted
                .entry(path)
                .or_default()
                .insert(method.to_lowercase());
        }
    }
    let mut documented = Operations::new();
    for (path, item) in document["paths"].as_object().into_iter().flatten() {
        let methods = item.as_object().into_iter().flatten().map(|(key, _)| key);
        documented.entry(shape(path)).or_default().extend(
            methods
                .filter(|key| !NOT_OPERATIONS.contains(&key.as_str()))
                .cloned(),
        );
    }
    Drift {
        undocumented: missing_from(&mounted, &documented),
        unmounted: missing_from(&documented, &mounted),
    }
}

fn walk(value: &Value, document: &Value, dangling: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key == "$ref"
                    && let Some(target) = child.as_str()
                    && target
                        .strip_prefix('#')
                        .is_some_and(|pointer| document.pointer(pointer).is_none())
                {
                    dangling.push(target.to_string());
                }
                walk(child, document, dangling);
            }
        }
        Value::Array(items) => {
            for child in items {
                walk(child, document, dangling);
            }
        }
        _ => {}
    }
}

/// Every `$ref` inside `document` that points at nothing in it. A reference to another file is
/// not followed, and not reported.
#[must_use]
pub fn dangling_refs(document: &Value) -> Vec<String> {
    let mut dangling = Vec::new();
    walk(document, document, &mut dangling);
    dangling
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn route(path: &str, methods: &[&str]) -> Vec<(String, String)> {
        methods
            .iter()
            .map(|method| ((*method).to_string(), path.to_string()))
            .collect()
    }

    #[test]
    fn parameter_names_do_not_change_a_shape() {
        assert_eq!(shape("/a/{pid}/b/{entity_pid}"), "/a/{}/b/{}");
        assert_eq!(shape("/a/b"), "/a/b");
    }

    #[test]
    fn agreement_is_empty_whatever_the_parameters_are_called() {
        let document = json!({ "paths": { "/a/{id}": { "get": {}, "post": {} } } });
        let routes = route("/a/{pid}", &["GET", "POST"]);
        assert!(drift(&routes, &document, &[]).is_empty());
    }

    #[test]
    fn a_mounted_route_missing_from_the_document_is_reported() {
        let document = json!({ "paths": { "/a": { "get": {} } } });
        let routes = [route("/a", &["GET", "PUT"]), route("/b", &["GET"])].concat();
        let found = drift(&routes, &document, &[]);
        assert_eq!(found.undocumented, ["put /a", "get /b"]);
        assert_eq!(found.unmounted, Vec::<String>::new());
    }

    #[test]
    fn a_documented_operation_with_no_route_is_reported() {
        let document =
            json!({ "paths": { "/a": { "get": {}, "put": {} }, "/gone": { "get": {} } } });
        let found = drift(&route("/a", &["GET"]), &document, &[]);
        assert_eq!(found.unmounted, ["put /a", "get /gone"]);
        assert_eq!(found.undocumented, Vec::<String>::new());
    }

    #[test]
    fn a_path_kept_out_on_purpose_is_not_reported() {
        let document = json!({ "paths": {} });
        let routes = route("/_health", &["GET"]);
        assert!(drift(&routes, &document, &["/_health"]).is_empty());
    }

    #[test]
    fn a_path_items_other_members_are_not_operations() {
        let document =
            json!({ "paths": { "/a": { "summary": "A", "parameters": [], "get": {} } } });
        assert!(drift(&route("/a", &["GET"]), &document, &[]).is_empty());
    }

    #[test]
    fn a_document_with_no_paths_reports_every_route() {
        let found = drift(&route("/a", &["GET"]), &json!({}), &[]);
        assert_eq!(found.undocumented, ["get /a"]);
    }

    #[test]
    fn a_reference_that_resolves_is_not_reported() {
        let document = json!({
            "components": { "schemas": { "Thing": {} } },
            "paths": { "/a": { "get": { "schema": { "$ref": "#/components/schemas/Thing" } } } }
        });
        assert_eq!(dangling_refs(&document), Vec::<String>::new());
    }

    #[test]
    fn a_reference_to_nothing_is_reported_wherever_it_is() {
        let document = json!({
            "components": { "schemas": {} },
            "list": [{ "nested": { "$ref": "#/components/schemas/Gone" } }]
        });
        assert_eq!(dangling_refs(&document), ["#/components/schemas/Gone"]);
    }

    #[test]
    fn a_reference_to_another_file_is_left_alone() {
        let document = json!({ "a": { "$ref": "other.json#/Thing" } });
        assert_eq!(dangling_refs(&document), Vec::<String>::new());
    }
}
