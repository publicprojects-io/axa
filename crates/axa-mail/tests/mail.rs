#![allow(clippy::unwrap_used, reason = "RCA-SCOPE-4: tests may unwrap")]
//! Each transport, sending a real message. The SMTP test talks to a small server on a
//! local port, in the clear, as development allows.

use axa_mail::{Mailer, Message};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

fn message(subject: &str) -> Message {
    Message::builder()
        .from("Accounts <accounts@example.com>".parse().unwrap())
        .to("someone@example.org".parse().unwrap())
        .subject(subject)
        .body(String::from("Follow the link to sign in."))
        .unwrap()
}

#[tokio::test]
async fn the_memory_transport_keeps_each_message_for_a_test() {
    let (mailer, outbox) = Mailer::memory();
    mailer.send(&message("First")).await.unwrap();
    mailer.clone().send(&message("Second")).await.unwrap();
    let sent = outbox.sent();
    assert_eq!(sent.len(), 2, "clones share the outbox");
    assert_eq!(sent[0].subject.as_deref(), Some("First"));
    assert_eq!(sent[0].from.as_deref(), Some("accounts@example.com"));
    assert_eq!(sent[0].to, vec!["someone@example.org".to_string()]);
    assert!(sent[1].text().contains("Follow the link to sign in."));
    assert_eq!(outbox.take().len(), 2);
    assert!(outbox.sent().is_empty(), "take leaves it empty");
    mailer.ping().await.unwrap();
}

#[tokio::test]
async fn a_memory_mailer_from_a_url_hands_its_outbox_to_a_test() {
    let mailer = Mailer::from_url("memory:", false).unwrap();
    let outbox = mailer.outbox().expect("a memory mailer has an outbox");
    mailer.send(&message("From the service")).await.unwrap();
    assert_eq!(
        outbox.sent()[0].subject.as_deref(),
        Some("From the service")
    );
    let dir = tempfile::tempdir().unwrap();
    assert!(Mailer::file(dir.path()).outbox().is_none());
}

#[tokio::test]
async fn the_file_transport_writes_one_file_per_message() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("file://{}", dir.path().display());
    let mailer = Mailer::from_url(&url, false).unwrap();
    mailer.send(&message("Kept on disk")).await.unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].extension().unwrap(), "eml");
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("Subject: Kept on disk"), "{text}");
}

/// A server that takes one message, and hands back what it was sent.
async fn smtp_server() -> (u16, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        write.write_all(b"220 test ESMTP\r\n").await.unwrap();
        let mut data = String::new();
        let mut in_data = false;
        while let Some(line) = lines.next_line().await.unwrap() {
            if in_data {
                if line == "." {
                    in_data = false;
                    write.write_all(b"250 queued\r\n").await.unwrap();
                } else {
                    data.push_str(&line);
                    data.push('\n');
                }
                continue;
            }
            let verb = line.get(..4).unwrap_or("").to_ascii_uppercase();
            let reply: &[u8] = match verb.as_str() {
                "EHLO" => b"250-test\r\n250 8BITMIME\r\n",
                "DATA" => {
                    in_data = true;
                    b"354 go on\r\n"
                }
                "QUIT" => {
                    write.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                }
                _ => b"250 ok\r\n",
            };
            write.write_all(reply).await.unwrap();
            data.push_str(&line);
            data.push('\n');
        }
        data
    });
    (port, server)
}

#[tokio::test]
async fn the_smtp_transport_sends_to_the_server() {
    let (port, server) = smtp_server().await;
    let mailer = Mailer::from_url(&format!("smtp://127.0.0.1:{port}"), false).unwrap();
    mailer.send(&message("Over SMTP")).await.unwrap();
    drop(mailer);
    let seen = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("the server finished")
        .unwrap();
    assert!(seen.contains("MAIL FROM:<accounts@example.com>"), "{seen}");
    assert!(seen.contains("RCPT TO:<someone@example.org>"), "{seen}");
    assert!(seen.contains("Subject: Over SMTP"), "{seen}");
}

#[tokio::test]
async fn ping_says_whether_the_transport_can_be_reached() {
    use std::error::Error as _;
    let (port, server) = smtp_server().await;
    let mailer = Mailer::from_url(&format!("smtp://127.0.0.1:{port}"), false).unwrap();
    mailer.ping().await.unwrap();
    drop(mailer);
    let seen = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .expect("the server finished")
        .unwrap();
    assert!(seen.to_ascii_uppercase().contains("NOOP"), "{seen}");

    // Nothing listens on the discard port.
    let down = Mailer::from_url("smtp://127.0.0.1:9", false).unwrap();
    let error = down.ping().await.unwrap_err();
    assert!(
        error.to_string().starts_with("the mail server: "),
        "{error}"
    );
    assert!(error.source().is_some());

    let (memory, _) = Mailer::memory();
    memory.ping().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    Mailer::from_url(&format!("file://{}", dir.path().display()), false)
        .unwrap()
        .ping()
        .await
        .unwrap();
}

#[tokio::test]
async fn a_file_that_cannot_be_written_says_so_and_keeps_its_cause() {
    use std::error::Error as _;
    let mailer = Mailer::file("/axa-no-such-folder/mail");
    assert_eq!(format!("{mailer:?}"), "Mailer { transport: \"file\", .. }");
    let error = mailer.send(&message("Lost")).await.unwrap_err();
    assert!(matches!(error, axa_mail::Error::File(_)), "{error:?}");
    let cause = error.source().expect("the file error").to_string();
    assert_eq!(error.to_string(), format!("the mail file: {cause}"));
}

#[test]
fn an_address_or_message_that_cannot_be_built_is_a_message_error_with_no_source() {
    use std::error::Error as _;
    let address = "not an address"
        .parse::<axa_mail::lettre::Address>()
        .unwrap_err();
    let error = axa_mail::Error::from(address);
    assert!(
        error
            .to_string()
            .starts_with("the message could not be built: "),
        "{error}"
    );
    assert!(error.source().is_none());
    // A message with no recipient cannot be built.
    let built = Message::builder()
        .from("accounts@example.com".parse().unwrap())
        .body(String::new())
        .unwrap_err();
    assert!(matches!(
        axa_mail::Error::from(built),
        axa_mail::Error::Message(_)
    ));
    let url = Mailer::from_url("http://example.com", false).unwrap_err();
    assert_eq!(
        url.to_string(),
        "the mail URL must be smtp://, smtps://, file://, or memory:, and parse"
    );
    assert!(url.source().is_none());
    let (memory, _) = Mailer::memory();
    assert_eq!(
        format!("{memory:?}"),
        "Mailer { transport: \"memory\", .. }"
    );
}

/// A server that answers `RCPT` with `refusal`, and everything before it with `250`.
async fn refusing_server(refusal: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        write.write_all(b"220 test ESMTP\r\n").await.unwrap();
        while let Ok(Some(line)) = lines.next_line().await {
            let verb = line.get(..4).unwrap_or("").to_ascii_uppercase();
            let reply = match verb.as_str() {
                "EHLO" => "250-test\r\n250 8BITMIME\r\n",
                "RCPT" => refusal,
                "QUIT" => "221 bye\r\n",
                _ => "250 ok\r\n",
            };
            if write.write_all(reply.as_bytes()).await.is_err() || verb == "QUIT" {
                break;
            }
        }
    });
    port
}

#[tokio::test]
async fn a_failure_says_whether_sending_again_later_may_work() {
    let send = async |url: String| {
        Mailer::from_url(&url, false)
            .unwrap()
            .send(&message("Retry?"))
            .await
            .unwrap_err()
    };
    let later = send(format!(
        "smtp://127.0.0.1:{}",
        refusing_server("451 4.3.0 busy\r\n").await
    ))
    .await;
    assert!(later.is_transient(), "a 4xx: {later}");
    let never = send(format!(
        "smtp://127.0.0.1:{}",
        refusing_server("550 5.1.1 no such user\r\n").await
    ))
    .await;
    assert!(!never.is_transient(), "a 5xx: {never}");
    let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let down = send(format!("smtp://127.0.0.1:{port}")).await;
    assert!(down.is_transient(), "a relay that is down: {down}");
    let file = Mailer::file("/axa-no-such-folder/mail")
        .send(&message("Lost"))
        .await
        .unwrap_err();
    assert!(!file.is_transient(), "{file}");
    assert!(
        !Mailer::from_url("http://example.com", false)
            .unwrap_err()
            .is_transient()
    );
}

#[test]
fn only_one_plain_address_is_safe() {
    for safe in [
        "someone@example.org",
        "a.b+tag@sub.example.org",
        "üser@example.org",
    ] {
        assert!(axa_mail::address_is_safe(safe), "{safe}");
    }
    let long = format!("{}@example.org", "a".repeat(250));
    for unsafe_ in [
        "",
        "someone",
        "@example.org",
        "someone@",
        "a@b@example.org",
        "someone@example.org\r\nBcc: x@evil.example",
        "a@example.org, b@example.org",
        "a@example.org;b@example.org",
        "Name <a@example.org>",
        "a b@example.org",
        "\"a\"@example.org",
        "a@example.org\0",
        long.as_str(),
    ] {
        assert!(!axa_mail::address_is_safe(unsafe_), "{unsafe_:?}");
    }
}
