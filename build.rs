//! Names the one database `AppContext::db` holds, as `cfg(axa_db = "...")`, so the
//! code says which it is in one word, and which database libraries are built at all, as
//! `cfg(has_sqlx)`, `cfg(has_seaorm)`, `cfg(has_diesel)`, and `cfg(has_toasty)`.
//!
//! Several database features may be on at once, as `--all-features` turns on every one.
//! Postgres wins, then MySQL, then SQLite, and for each, sqlx wins, because it is not an
//! ORM, then SeaORM, Diesel, and Toasty.

const DATABASES: [&str; 3] = ["postgres", "mysql", "sqlite"];
const LIBRARIES: [&str; 4] = ["sqlx", "seaorm", "diesel", "toasty"];

fn main() {
    let on = |feature: &str| {
        let name = feature.to_ascii_uppercase().replace('-', "_");
        std::env::var_os(format!("CARGO_FEATURE_{name}")).is_some()
    };
    let mut names = vec!["none".to_string()];
    let mut db = "none".to_string();
    for database in DATABASES {
        for library in LIBRARIES {
            let name = format!("{database}_{library}");
            if db == "none" && on(&format!("{database}-{library}")) {
                db.clone_from(&name);
            }
            names.push(name);
        }
    }
    let values: Vec<String> = names.iter().map(|name| format!("\"{name}\"")).collect();
    println!(
        "cargo::rustc-check-cfg=cfg(axa_db, values({}))",
        values.join(", ")
    );
    println!("cargo::rustc-cfg=axa_db=\"{db}\"");
    // A library is built when any database feature on it is. sqlx is also built for the
    // queue, the outbox, and idempotency keys, and for SeaORM, whose errors are read as
    // sqlx's.
    for library in LIBRARIES {
        let mut built = DATABASES
            .iter()
            .any(|database| on(&format!("{database}-{library}")));
        if library == "sqlx" {
            built |= ["queue", "outbox", "idempotency"]
                .iter()
                .any(|feature| on(feature))
                || DATABASES
                    .iter()
                    .any(|database| on(&format!("{database}-seaorm")));
        }
        println!("cargo::rustc-check-cfg=cfg(has_{library})");
        if built {
            println!("cargo::rustc-cfg=has_{library}");
        }
    }
    // Which of sqlx's drivers are built, so that an error can be read as its own
    // database's: Postgres for the queue, the outbox, and idempotency keys, and each
    // database for its sqlx and SeaORM features.
    for database in DATABASES {
        let mut built = ["sqlx", "seaorm"]
            .iter()
            .any(|library| on(&format!("{database}-{library}")));
        if database == "postgres" {
            built |= ["queue", "outbox", "idempotency"]
                .iter()
                .any(|feature| on(feature));
        }
        println!("cargo::rustc-check-cfg=cfg(has_sqlx_{database})");
        if built {
            println!("cargo::rustc-cfg=has_sqlx_{database}");
        }
    }
    println!("cargo::rerun-if-changed=build.rs");
}
