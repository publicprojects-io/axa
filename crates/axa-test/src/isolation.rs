//! Checks that one tenant cannot reach another's resources.
//!
//! A service that many tenants share keeps them apart in every route that takes an
//! id. The test that proves it creates a resource as one tenant, then tries every way
//! in as another: each must answer `404`, as a missing resource does, so that a caller
//! cannot learn which ids other tenants hold, and a list must not show it.
//!
//! ```ignore
//! use axa_test::isolation::{Probe, assert_hidden};
//!
//! let created = server.post("/api/todos").add_header("authorization", &acme).json(&todo).await;
//! let id = created.json::<serde_json::Value>()["id"].to_string();
//! let path = format!("/api/todos/{id}");
//! assert_hidden(
//!     &server,
//!     &[("authorization", globex.as_str())],
//!     &[
//!         Probe::get(&path),
//!         Probe::patch(&path, r#"{"completed":true}"#),
//!         Probe::delete(&path),
//!         Probe::list("/api/todos", "Acme's secret plan"),
//!     ],
//! )
//! .await;
//! ```

use axum_test::{
    TestServer,
    http::{Method, StatusCode},
};

/// One way in to a resource, tried as the other tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    method: Method,
    path: String,
    body: Option<String>,
    /// For a list: text the other tenant's answer must not hold.
    absent: Option<String>,
}

impl Probe {
    /// A `GET` of the resource, which must answer `404`.
    #[must_use]
    pub fn get(path: &str) -> Self {
        Self::one(Method::GET, path, None)
    }

    /// A `PATCH` of the resource with a JSON `body`, which must answer `404`.
    #[must_use]
    pub fn patch(path: &str, body: &str) -> Self {
        Self::one(Method::PATCH, path, Some(body))
    }

    /// A `PUT` of the resource with a JSON `body`, which must answer `404`.
    #[must_use]
    pub fn put(path: &str, body: &str) -> Self {
        Self::one(Method::PUT, path, Some(body))
    }

    /// A `DELETE` of the resource, which must answer `404`.
    #[must_use]
    pub fn delete(path: &str) -> Self {
        Self::one(Method::DELETE, path, None)
    }

    /// A `POST` to a route under the resource, such as `/api/todos/7/archive`, with a
    /// JSON `body`, which must answer `404`.
    #[must_use]
    pub fn post(path: &str, body: &str) -> Self {
        Self::one(Method::POST, path, Some(body))
    }

    /// A `GET` of a list, which must succeed and must not hold `absent`, such as the
    /// resource's id in its JSON form or a title only the owner wrote.
    #[must_use]
    pub fn list(path: &str, absent: &str) -> Self {
        Self {
            method: Method::GET,
            path: path.to_string(),
            body: None,
            absent: Some(absent.to_string()),
        }
    }

    fn one(method: Method, path: &str, body: Option<&str>) -> Self {
        Self {
            method,
            path: path.to_string(),
            body: body.map(str::to_string),
            absent: None,
        }
    }

    /// What is wrong with the answer to this probe, or `None` when it is right.
    fn fault(&self, status: StatusCode, body: &str) -> Option<String> {
        let name = format!("{} {}", self.method, self.path);
        match &self.absent {
            Some(absent) if !status.is_success() => Some(format!(
                "{name} answered {status}, where a list should succeed"
            )),
            Some(absent) if body.contains(absent.as_str()) => {
                Some(format!("{name} shows {absent:?} to another tenant"))
            }
            Some(_) => None,
            None if status == StatusCode::NOT_FOUND => None,
            None => Some(format!(
                "{name} answered {status} to another tenant, where it should be 404"
            )),
        }
    }
}

/// Send every probe to `server` with `headers`, the other tenant's credentials, and
/// panic naming each one that reached the resource, answered other than `404`, or
/// listed it. Other than `404` includes `403`, which tells a caller the id exists.
///
/// # Panics
///
/// When any probe's answer is wrong, listing every wrong one.
pub async fn assert_hidden(server: &TestServer, headers: &[(&str, &str)], probes: &[Probe]) {
    let mut faults = Vec::new();
    for probe in probes {
        let mut request = server.method(probe.method.clone(), &probe.path);
        for (name, value) in headers {
            request = request.add_header(*name, *value);
        }
        if let Some(body) = &probe.body {
            request = request.content_type("application/json").text(body.clone());
        }
        let response = request.await;
        if let Some(fault) = probe.fault(response.status_code(), &response.text()) {
            faults.push(fault);
        }
    }
    assert!(
        faults.is_empty(),
        "another tenant reached a resource:\n{}",
        faults.join("\n")
    );
}
