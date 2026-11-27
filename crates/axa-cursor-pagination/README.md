# axa-cursor-pagination

Signed cursor pagination for Axum services, from the [axa](https://github.com/publicprojects-io/axa) family, with `LIKE` escaping.

A list takes `?limit=` and an opaque `?cursor=`, and answers a bare JSON array with `Link: <...>; rel="next"` while more rows follow. There is no total, because a total would count rows the caller's record policy hides. A cursor is signed with the service's key over its position and its query, so a caller cannot make one up, edit one, or carry one to another list or filter. A bad limit or cursor is refused with `422`.

For a list a record policy filters row by row, `CursorPage::fill` reads on in batches until the page is full, within a read budget, so a page is not short for the rows the caller may not see, and the next page starts right after the last row it returned. `CursorPage::next_link_at` signs a next link at any offset past the page, for a service that reads ahead its own way.

Enable it through the `cursor-pagination` feature of `axa`, or depend on this crate alone. See the crate documentation for usage.

## License

See [LICENSE.md](https://github.com/publicprojects-io/axa/blob/main/LICENSE.md).
