//! Run one test in a process of its own.
//!
//! A service reads some state once per process: a `OnceLock` of settings, a global
//! `tracing` subscriber, a TLS provider, a key. A test that needs that state set its
//! own way, or that sets process environment variables, cannot share a process with
//! the others. With every test in one binary, [`own_process!`](crate::own_process!)
//! runs the test again, alone, in a fresh process of the same binary, and fails it
//! unless that run passed exactly one test:
//!
//! ```ignore
//! #[test]
//! fn the_policy_is_read_once_from_its_file() {
//!     axa_test::own_process!();
//!     // From here on, this runs only in the fresh process.
//!     unsafe { std::env::set_var("POLICY_FILE", "tests/policy.json") };
//!     assert!(service::policy().allows("read"));
//! }
//! ```

use std::process::Command;

/// The variable that tells a fresh process which test it runs alone.
pub const VAR: &str = "AXA_TEST_OWN_PROCESS";

/// The test's name as the test harness knows it, from the type name of a function
/// declared in its body: the path below the crate, without the function itself and
/// any closure or async block between.
#[must_use]
pub fn test_name(type_name: &str) -> String {
    let mut parts: Vec<&str> = type_name.split("::").collect();
    parts.pop();
    while parts.last().is_some_and(|part| part.starts_with('{')) {
        parts.pop();
    }
    parts.into_iter().skip(1).collect::<Vec<_>>().join("::")
}

/// Whether this process is the fresh one running `name` alone.
#[must_use]
pub fn is_alone(name: &str) -> bool {
    std::env::var(VAR).is_ok_and(|alone| alone == name)
}

/// Run the test `name` alone in a fresh process of this test binary, ignored or not.
///
/// # Panics
///
/// Unless that process ran exactly one test, and it passed: so a test that failed, or a
/// name that matched none, fails the test that asked.
pub fn run_alone(name: &str) {
    let binary = std::env::current_exe().expect("the test binary's path");
    let output = Command::new(binary)
        .args([name, "--exact", "--include-ignored", "--test-threads=1"])
        .env(VAR, name)
        .output()
        .expect("the test binary starts again");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed"),
        "{name} did not pass alone in its own process:\n{stdout}\n{stderr}"
    );
}

/// Run the rest of this test alone in a process of its own, as the module says. Call it
/// first, in a test that returns `()`.
#[macro_export]
macro_rules! own_process {
    () => {{
        fn here() {}
        let name = $crate::own_process::test_name(::std::any::type_name_of_val(&here));
        if !$crate::own_process::is_alone(&name) {
            $crate::own_process::run_alone(&name);
            return;
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::test_name;

    #[test]
    fn a_test_is_named_below_its_crate_and_past_any_closure() {
        assert_eq!(
            test_name("my_service::cache::tests::a_miss::here"),
            "cache::tests::a_miss"
        );
        assert_eq!(
            test_name("main::requests::a_miss::{{closure}}::here"),
            "requests::a_miss"
        );
        assert_eq!(test_name("main::top_level::here"), "top_level");
    }
}
