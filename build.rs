//! Names the one database `AppContext::db` holds, as `cfg(axa_db = "...")`, so the
//! code says which it is in one word. Several database features may be on at once,
//! as `--all-features` turns on every one: `sqlx` wins, because it is not an ORM,
//! then `sea-orm`, `diesel`, and `toasty`.

fn main() {
    let on = |feature: &str| std::env::var_os(format!("CARGO_FEATURE_{feature}")).is_some();
    let db = if on("SQLX") {
        "sqlx"
    } else if on("SEA_ORM") {
        "seaorm"
    } else if on("DIESEL") {
        "diesel"
    } else if on("TOASTY") {
        "toasty"
    } else {
        "none"
    };
    println!(
        "cargo::rustc-check-cfg=cfg(axa_db, values(\"sqlx\", \"seaorm\", \"diesel\", \"toasty\", \"none\"))"
    );
    println!("cargo::rustc-cfg=axa_db=\"{db}\"");
    println!("cargo::rerun-if-changed=build.rs");
}
