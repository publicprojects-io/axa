# axa-storage-with-apache-arrow-and-aws

Object storage for Axum services, over Apache Arrow's
[`object_store`](https://crates.io/crates/object_store): S3, a directory on local disk, or
memory, chosen by a URL, with keys that cannot climb out of the store and presigned URLs
on S3. In `axa` it is the `storage` feature, as `axa::storage`.

```toml
[dependencies]
axa-storage-with-apache-arrow-and-aws = "0.1"
```

```rust,no_run
use axa_storage_with_apache_arrow_and_aws::{Key, Storage};

# async fn example(url: &str, user_id: u64) -> Result<(), axa_storage_with_apache_arrow_and_aws::Error> {
let storage = Storage::from_url(url, true)?;
let key = Key::new(format!("avatars/{user_id}.png"))?;
storage.put(&key, bytes::Bytes::from_static(b"...")).await?;
let link = storage.presigned_get(&key, std::time::Duration::from_mins(5)).await?;
# let _ = link;
# Ok(())
# }
```

## Stores

| URL | Store | In production |
|---|---|---|
| `s3://bucket` or `s3://bucket/prefix` | S3, configured from the `AWS_` variables | Yes, over `https` only |
| `file:///var/lib/service/files` | A directory on local disk, which must exist | Yes |
| `memory:` | In memory, for tests | Refused |

## Keys

A `Key` names every object, and refuses an empty key or segment, `.` and `..`, a leading
or trailing `/`, a `\`, a control character, and more than 1024 bytes. Build keys from ids
the service made, never from a file name someone sent, and keep a name to show in the
service's own table.

## Presigned URLs

`presigned_get` and `presigned_put` sign a URL for one S3 object, for a time, so a browser
can download or upload straight to S3. Anyone holding one can use it until it expires:
keep the time short, hand it only to the person the object is for, and never log it. A
presigned `PUT` does not limit the size of what is sent.

## TLS

S3 is reached over rustls with ring, as the other axa crates reach their servers.
`Storage::from_url` installs ring as rustls's process-wide provider when none is
installed. A service that builds its own `AmazonS3` for `Storage::s3` calls
`install_ring_provider()` first.
