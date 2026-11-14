# axa-maud-templates

Maud compile-time HTML templates with safe escaping and generic errors for Axum services,
from the [Axa](https://github.com/publicprojects-io/axa) family.

A template is Rust code written with Maud's `html!` macro, so a typo or a missing value is
a compile error rather than a failure at run time. Values are escaped by default, a page
answers with its own content security policy so its stylesheets and images load, and a
failure is logged rather than shown to the visitor.

`html!` expands to code that names the `maud` crate, so a service depends on `maud` too,
at the version this crate uses:

```toml
[dependencies]
axa = { version = "0.13", features = ["maud"] }
maud = "0.27"
```

```rust
use axa::maud::{Error, Page, html};

async fn hello() -> Result<Page, Error> {
    let name = "<b>Alice</b>";
    Ok(Page::from(html! { p { "Hello, " (name) "!" } }))
}
```

It does for Maud what [`axa-tera-templates`](https://crates.io/crates/axa-tera-templates)
does for Tera: choose Maud for templates checked by the compiler, and Tera for templates
in files that change without a rebuild.

Enable it through the `maud` feature of `axa`, as `axa::maud`, or depend on this crate
alone. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
