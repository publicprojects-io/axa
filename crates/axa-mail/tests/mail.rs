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
