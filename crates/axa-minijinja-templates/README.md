# axa-minijinja-templates

MiniJinja templates with safe escaping, strict undefined values, and generic errors for
Axum services, from the [axa](https://github.com/publicprojects-io/axa) family.

MiniJinja is the Jinja2 template language in Rust, with few dependencies. HTML escaping
is on by default, a missing value is an error rather than blank output, every template
is parsed when the service boots, and a template failure is logged rather than shown to
the visitor.

```toml
[dependencies]
axa = { version = "0.17", features = ["minijinja"] }
```

```rust
use axa::minijinja::{Error, Page, Templates};
use axum::extract::State;
use serde_json::json;

async fn hello(State(templates): State<Templates>) -> Result<Page, Error> {
    templates.html("hello.html", &json!({ "name": "Alice" }))
}

let templates = Templates::from_dir("templates")?;
```

Enable it through the `minijinja` feature of `axa`, or depend on this crate alone.
See the crate documentation for the rest.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
