//! Each store, reading and writing real objects, and S3 signing, which needs no network.

use std::time::Duration;

use axa_storage_with_apache_arrow_and_aws::{
    Error, Key, Storage, install_ring_provider, object_store::aws::AmazonS3Builder,
};
use bytes::Bytes;

async fn round_trip(storage: &Storage) {
    let key = Key::new("avatars/42.png").unwrap();
    assert_eq!(storage.get_limited(&key, 100).await.unwrap(), None);
    assert_eq!(storage.head(&key).await.unwrap(), None);
    storage
        .put(&key, Bytes::from_static(b"twelve bytes"))
        .await
        .unwrap();
    assert_eq!(
        storage.get_limited(&key, 100).await.unwrap().as_deref(),
        Some(&b"twelve bytes"[..])
    );
    assert_eq!(storage.head(&key).await.unwrap().unwrap().size, 12);
    assert!(matches!(
        storage.get_limited(&key, 11).await,
        Err(Error::TooLarge { size: 12 })
    ));
    storage.put(&key, Bytes::from_static(b"two")).await.unwrap();
    assert_eq!(
        storage.head(&key).await.unwrap().unwrap().size,
        3,
        "replaced"
    );
    storage.delete(&key).await.unwrap();
    assert_eq!(storage.head(&key).await.unwrap(), None);
    storage.delete(&key).await.unwrap();
    assert!(matches!(
        storage.presigned_get(&key, Duration::from_mins(5)).await,
        Err(Error::NotSupported)
    ));
}

#[tokio::test]
async fn the_memory_store_reads_and_writes() {
    round_trip(&Storage::from_url("memory:", false).unwrap()).await;
}

#[tokio::test]
async fn the_local_store_reads_and_writes_inside_its_directory() {
    let dir = tempfile::tempdir().unwrap();
    let inside = dir.path().join("files");
    std::fs::create_dir(&inside).unwrap();
    let storage = Storage::from_url(&format!("file://{}", inside.display()), true).unwrap();
    round_trip(&storage).await;
    storage
        .put(&Key::new("a/b.txt").unwrap(), Bytes::from_static(b"x"))
        .await
        .unwrap();
    assert!(inside.join("a/b.txt").is_file(), "under its directory");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "and nowhere beside it"
    );
}

#[tokio::test]
async fn s3_signs_a_url_for_one_object_for_a_time() {
    install_ring_provider();
    let s3 = AmazonS3Builder::new()
        .with_bucket_name("files")
        .with_region("eu-west-2")
        .with_access_key_id("AKIDEXAMPLE")
        .with_secret_access_key("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY")
        .build()
        .unwrap();
    let prefix = Key::new("tenant-a").unwrap();
    let storage = Storage::s3(s3, Some(&prefix));
    let key = Key::new("avatars/42.png").unwrap();
    for (url, method) in [
        (
            storage
                .presigned_get(&key, Duration::from_mins(5))
                .await
                .unwrap(),
            "GET",
        ),
        (
            storage
                .presigned_put(&key, Duration::from_mins(5))
                .await
                .unwrap(),
            "PUT",
        ),
    ] {
        assert!(url.starts_with("https://"), "{method}: {url}");
        assert!(
            url.contains("/tenant-a/avatars/42.png?"),
            "{method}: under the prefix: {url}"
        );
        assert!(url.contains("X-Amz-Expires=300"), "{method}: {url}");
        assert!(url.contains("X-Amz-Signature="), "{method}: {url}");
        assert!(
            !url.contains("wJalrXUtnFEMI"),
            "{method}: the secret stays out"
        );
    }
    assert!(!format!("{storage:?}").contains("AKIDEXAMPLE"));
}

#[tokio::test]
async fn s3_over_tls_with_no_server_is_an_error_and_not_a_panic() {
    // rustls panics at the first connection when it has two providers and none chosen,
    // so this reaches the TLS client: nothing listens on the discard port.
    install_ring_provider();
    let s3 = AmazonS3Builder::new()
        .with_bucket_name("files")
        .with_region("eu-west-2")
        .with_endpoint("https://127.0.0.1:9")
        .with_access_key_id("AKIDEXAMPLE")
        .with_secret_access_key("secret")
        .with_retry(object_store_no_retries())
        .build()
        .unwrap();
    let storage = Storage::s3(s3, None);
    let result = storage.head(&Key::new("a").unwrap()).await;
    assert!(matches!(result, Err(Error::Store(_))), "{result:?}");
}

fn object_store_no_retries() -> axa_storage_with_apache_arrow_and_aws::object_store::RetryConfig {
    axa_storage_with_apache_arrow_and_aws::object_store::RetryConfig {
        max_retries: 0,
        ..Default::default()
    }
}
