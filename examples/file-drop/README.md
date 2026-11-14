# File drop

A service that takes one file at a time and serves it back, with Axa's `upload` feature
and `format::download`. Files are kept in memory, so they go when it stops.

## Run it

```sh
cd examples/file-drop
cargo run
curl -F 'file=@notes.txt;type=text/plain' localhost:1241/api/files
curl -i localhost:1241/api/files/1
```

## Uploading: `POST /api/files`

`upload::read` holds the body to `Limits` as the bytes arrive:

```rust,ignore
Limits {
    total_bytes: MAX_FILE_BYTES + 64_000,
    file_bytes: MAX_FILE_BYTES,
    files: 1,
    parts: 4,
    accept: Some(vec!["text/plain", "image/png", "application/pdf", "text/html"]),
    ..Limits::default()
}
```

| The upload | The answer |
|---|---|
| One file of an accepted type, up to 1 MB | `200`, `{"id": 1, "name": "notes.txt", "bytes": 4}` |
| A file over 1 MB | `413`, code `payload_too_large`, refused when it passes the limit |
| A type not in `accept` | `415`, code `unsupported_media_type` |
| No part named `file` | `400` |

The route raises the standard body limit to fit, with
`middleware::body_limit(post(create), bytes)`, and no other route's. The file arrives
in a temporary file, which is deleted when the `Upload` drops, so the handler reads it
first. A service that keeps files on disk calls `File::persist` instead.

The file's name is cleaned, so `../../notes.txt` is kept as `notes.txt`, and the media
type is what the caller declared, not a check of the bytes.

## Downloading: `GET /api/files/{id}`

`format::download` sets `content-disposition`. A text file, an image, audio, or video is
shown in place. HTML, SVG, PDF, and anything else is downloaded, so a page someone
uploaded never runs as this site.

## Test it

```sh
cargo test
```
