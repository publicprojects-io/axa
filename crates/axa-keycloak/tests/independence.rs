//! This crate exists to run and be audited alone, so nothing may bring back what it
//! was copied from. Anything transitive would need a direct dependency first.

const MANIFEST: &str = include_str!("../Cargo.toml");

#[test]
fn no_dependency_on_what_this_crate_was_copied_from() {
    for line in MANIFEST.lines().filter(|line| !line.starts_with('#')) {
        let dependency = line.split(['=', ' ']).next().unwrap_or_default();
        assert_ne!(dependency, "authentication-verifier");
        assert!(
            dependency == "axa-keycloak" || !dependency.starts_with("axa"),
            "{dependency}"
        );
    }
}
