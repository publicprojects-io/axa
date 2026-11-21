//! Mail for Axum services, over [`lettre`]: an SMTP, file, or memory transport chosen by
//! a URL, with production refusing SMTP that does not use TLS.
//!
//! ```no_run
//! use axa_mail::{Mailer, Message};
//!
//! # async fn example(url: &str) -> Result<(), axa_mail::Error> {
//! let mailer = Mailer::from_url(url, true)?;
//! let message = Message::builder()
//!     .from("Accounts <accounts@example.com>".parse()?)
//!     .to("someone@example.org".parse()?)
//!     .subject("Your sign-in link")
//!     .body(String::from("Follow the link to sign in."))?;
//! mailer.send(&message).await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Transports
//!
//! | URL | Transport |
//! |---|---|
//! | `smtps://user:password@smtp.example.com:465` | SMTP over TLS from the start |
//! | `smtp://user:password@smtp.example.com:587?tls=required` | SMTP that must upgrade with `STARTTLS` |
//! | `smtp://localhost:1025` | SMTP in the clear, for a local catcher such as Mailpit |
//! | `file:///var/tmp/mail` | One `.eml` file per message, in that directory |
//! | `memory:` | Kept in memory, for tests: see [`Mailer::memory`] |
//!
//! In production, [`Mailer::from_url`] refuses SMTP in the clear, `tls=opportunistic`,
//! which sends in the clear when the server offers no `STARTTLS`, and the file and
//! memory transports, from which mail would reach no one. TLS uses rustls, with the
//! web's root certificates.
//!
//! # What it does not do
//!
//! It does not queue mail. [`Mailer::send`] talks to the server while it is awaited,
//! so a request handler that sends mail waits for it, and a failure is the caller's to
//! retry. Send from a job, queued in the transaction that made the reason to send, so
//! that mail goes out once the record is there and is retried with the job.
//!
//! # Personal data
//!
//! A message holds addresses and text about a person. Nothing here logs a message or
//! an address. An [`Error`] says what failed and never quotes the message, and the
//! `Debug` text of a [`Mailer`] leaves out the URL, which may hold a password.

use std::{
    fmt,
    path::Path,
    sync::{Arc, Mutex, PoisonError},
};

pub use lettre;
use lettre::{
    AsyncFileTransport, AsyncSmtpTransport, AsyncTransport, Tokio1Executor, message::header,
};
pub use lettre::{
    Message,
    message::{Mailbox, header::ContentType},
};

/// Why mail could not be set up or sent.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The URL names no transport this crate has, or does not parse.
    Url,
    /// Production refuses this transport: SMTP without TLS, or the file or memory
    /// transport.
    Refused(&'static str),
    /// An address or a message could not be built.
    Message(String),
    /// The server refused the message, or could not be reached.
    Smtp(lettre::transport::smtp::Error),
    /// The file transport could not write the message.
    File(lettre::transport::file::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url => f.write_str(
                "the mail URL must be smtp://, smtps://, file://, or memory:, and parse",
            ),
            Self::Refused(why) => write!(f, "production refuses this mail transport: {why}"),
            Self::Message(text) => write!(f, "the message could not be built: {text}"),
            Self::Smtp(error) => write!(f, "the mail server: {error}"),
            Self::File(error) => write!(f, "the mail file: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Smtp(error) => Some(error),
            Self::File(error) => Some(error),
            Self::Url | Self::Refused(_) | Self::Message(_) => None,
        }
    }
}

impl Error {
    /// Whether the same message may go if sent again later, as a job retries it: the
    /// server answered "try later" (a `4xx`), timed out, or could not be reached. A
    /// refusal for good (a `5xx`), a TLS failure, a message that cannot be built, and
    /// the file transport's failures are not, as sending again would fail the same way.
    #[must_use]
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Smtp(error) => {
                error.is_transient()
                    || error.is_timeout()
                    || !(error.is_permanent()
                        || error.is_client()
                        || error.is_response()
                        || error.is_tls())
            }
            Self::Url | Self::Refused(_) | Self::Message(_) | Self::File(_) => false,
        }
    }
}

/// Whether `address` is one plain address, safe to hand on as a recipient or sender:
/// a local part and a domain around one `@`, at most 254 bytes, with no space, control
/// character, or `,` `;` `<` `>` `"` `(` `)`. So text that would add a header or a second
/// recipient, as a name a person typed can, is refused before any message is built.
#[must_use]
pub fn address_is_safe(address: &str) -> bool {
    let Some((local, domain)) = address.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && address.len() <= 254
        && !address.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, ',' | ';' | '<' | '>' | '"' | '(' | ')')
        })
}

impl From<lettre::address::AddressError> for Error {
    fn from(error: lettre::address::AddressError) -> Self {
        Self::Message(error.to_string())
    }
}

impl From<lettre::error::Error> for Error {
    fn from(error: lettre::error::Error) -> Self {
        Self::Message(error.to_string())
    }
}

/// One message the memory transport kept.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Sent {
    /// The envelope's sender, if it had one.
    pub from: Option<String>,
    /// The envelope's recipients.
    pub to: Vec<String>,
    /// The `Subject` header, as given.
    pub subject: Option<String>,
    /// The whole message, as it would go on the wire.
    pub formatted: Vec<u8>,
}

impl Sent {
    /// The whole message as text, for a test to look in.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.formatted).into_owned()
    }
}

/// What the memory transport has kept, shared with the [`Mailer`] that sends to it.
#[derive(Debug, Clone, Default)]
pub struct Outbox {
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl Outbox {
    /// Every message sent so far, oldest first.
    #[must_use]
    pub fn sent(&self) -> Vec<Sent> {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Every message sent so far, oldest first, leaving the outbox empty.
    #[must_use]
    pub fn take(&self) -> Vec<Sent> {
        std::mem::take(&mut *self.sent.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn keep(&self, message: &Message) {
        let envelope = message.envelope();
        let sent = Sent {
            from: envelope.from().map(ToString::to_string),
            to: envelope.to().iter().map(ToString::to_string).collect(),
            subject: message
                .headers()
                .get::<header::Subject>()
                .map(|subject| subject.as_ref().to_string()),
            formatted: message.formatted(),
        };
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(sent);
    }
}

#[derive(Clone)]
enum Transport {
    Smtp(AsyncSmtpTransport<Tokio1Executor>),
    File(AsyncFileTransport<Tokio1Executor>),
    Memory(Outbox),
}

/// Sends mail through one transport. Cheap to clone: an SMTP mailer keeps a pool of
/// connections, which its clones share.
#[derive(Clone)]
pub struct Mailer {
    transport: Transport,
}

impl fmt::Debug for Mailer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.transport {
            Transport::Smtp(_) => "smtp",
            Transport::File(_) => "file",
            Transport::Memory(_) => "memory",
        };
        f.debug_struct("Mailer")
            .field("transport", &kind)
            .finish_non_exhaustive()
    }
}

/// What a mail URL asks for, before anything is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `smtps://`, or `smtp://` with `tls=required`.
    SmtpTls,
    /// `smtp://` in the clear, or with `tls=opportunistic`.
    SmtpClear,
    File,
    Memory,
}

/// Read the kind of transport a URL names. The SMTP forms are left to `lettre` to
/// parse in full.
fn kind(url: &str) -> Result<Kind, Error> {
    let (scheme, rest) = url.split_once(':').ok_or(Error::Url)?;
    let tls = rest.split_once('?').map_or("", |(_, query)| query);
    let tls = tls
        .split('&')
        .find_map(|pair| pair.strip_prefix("tls="))
        .unwrap_or("");
    match (scheme.to_ascii_lowercase().as_str(), tls) {
        ("smtps", "") | ("smtp", "required") => Ok(Kind::SmtpTls),
        ("smtp", "" | "opportunistic") => Ok(Kind::SmtpClear),
        ("file", "") if rest.starts_with("//") => Ok(Kind::File),
        ("memory", "") => Ok(Kind::Memory),
        _ => Err(Error::Url),
    }
}

impl Mailer {
    /// The mailer a URL names: see [Transports](crate#transports). With
    /// `production` set, SMTP without TLS and the file and memory transports are
    /// refused. A mailer from `memory:` keeps its mail where no test can read it: use
    /// [`Mailer::memory`] in a test. An SMTP mailer keeps a pool of connections, which
    /// needs a Tokio runtime, so make and drop it inside one.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Url`] for a URL this crate cannot read, with
    /// [`Error::Refused`] for a transport production refuses, and with
    /// [`Error::Smtp`] for an SMTP URL `lettre` cannot read. None quotes the URL.
    pub fn from_url(url: &str, production: bool) -> Result<Self, Error> {
        let kind = kind(url)?;
        if production {
            match kind {
                Kind::SmtpTls => {}
                Kind::SmtpClear => {
                    return Err(Error::Refused(
                        "SMTP must use TLS: smtps://, or smtp:// with tls=required",
                    ));
                }
                Kind::File => return Err(Error::Refused("the file transport reaches no one")),
                Kind::Memory => {
                    return Err(Error::Refused("the memory transport reaches no one"));
                }
            }
        }
        match kind {
            Kind::SmtpTls | Kind::SmtpClear => Ok(Self {
                transport: Transport::Smtp(
                    AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
                        .map_err(Error::Smtp)?
                        .build(),
                ),
            }),
            Kind::File => {
                let path = url.get("file://".len()..).ok_or(Error::Url)?;
                Ok(Self::file(path))
            }
            Kind::Memory => Ok(Self::memory().0),
        }
    }

    /// A mailer that writes each message to `dir`, one `.eml` file each, named by a
    /// random id. The directory must exist.
    pub fn file(dir: impl AsRef<Path>) -> Self {
        Self {
            transport: Transport::File(AsyncFileTransport::new(dir)),
        }
    }

    /// A mailer that keeps each message in memory, and the [`Outbox`] to read them
    /// from, for tests. Production refuses it from a URL.
    #[must_use]
    pub fn memory() -> (Self, Outbox) {
        let outbox = Outbox::default();
        (
            Self {
                transport: Transport::Memory(outbox.clone()),
            },
            outbox,
        )
    }

    /// The [`Outbox`] of a memory mailer, from [`Mailer::memory`] or `memory:`, so a
    /// test can read what a service sent through the mailer it built itself. `None`
    /// for any other transport.
    #[must_use]
    pub fn outbox(&self) -> Option<Outbox> {
        match &self.transport {
            Transport::Memory(outbox) => Some(outbox.clone()),
            _ => None,
        }
    }

    /// Whether the transport can be reached: for SMTP, a connection and `NOOP`. The
    /// file and memory transports always can. For a readiness check.
    ///
    /// # Errors
    ///
    /// Fails when the SMTP server cannot be reached.
    pub async fn ping(&self) -> Result<(), Error> {
        match &self.transport {
            Transport::Smtp(smtp) => {
                if smtp.test_connection().await.map_err(Error::Smtp)? {
                    Ok(())
                } else {
                    Err(Error::Refused("the mail server did not answer NOOP"))
                }
            }
            Transport::File(_) | Transport::Memory(_) => Ok(()),
        }
    }

    /// Send `message`, waiting for the server to take it.
    ///
    /// # Errors
    ///
    /// Fails when the server refuses the message or cannot be reached, or the file
    /// cannot be written. Nothing retries: send from a job to have it retried.
    pub async fn send(&self, message: &Message) -> Result<(), Error> {
        match &self.transport {
            Transport::Smtp(smtp) => {
                smtp.send_raw(message.envelope(), &message.formatted())
                    .await
                    .map_err(Error::Smtp)?;
            }
            Transport::File(file) => {
                file.send_raw(message.envelope(), &message.formatted())
                    .await
                    .map_err(Error::File)?;
            }
            Transport::Memory(outbox) => outbox.keep(message),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_names_its_kind_of_transport() {
        for (url, want) in [
            ("smtps://u:p@smtp.example.com:465", Kind::SmtpTls),
            ("SMTPS://smtp.example.com", Kind::SmtpTls),
            ("smtp://smtp.example.com:587?tls=required", Kind::SmtpTls),
            ("smtp://localhost:1025", Kind::SmtpClear),
            ("smtp://smtp.example.com?tls=opportunistic", Kind::SmtpClear),
            ("file:///var/tmp/mail", Kind::File),
            ("memory:", Kind::Memory),
        ] {
            assert_eq!(kind(url).unwrap(), want, "{url}");
        }
        for url in [
            "",
            "smtp",
            "http://example.com",
            "smtps://smtp.example.com?tls=required",
            "smtp://smtp.example.com?tls=wrapper",
            "file:relative",
            "memory:?tls=required",
        ] {
            assert!(matches!(kind(url), Err(Error::Url)), "{url}");
        }
    }

    #[tokio::test]
    async fn production_refuses_mail_without_tls_or_that_reaches_no_one() {
        for url in [
            "smtp://u:secret@smtp.example.com",
            "smtp://smtp.example.com?tls=opportunistic",
            "file:///var/tmp/mail",
            "memory:",
        ] {
            let error = Mailer::from_url(url, true).unwrap_err();
            assert!(matches!(error, Error::Refused(_)), "{url}");
            assert!(
                !error.to_string().contains("secret"),
                "the URL is not quoted"
            );
        }
        for url in [
            "smtps://u:secret@smtp.example.com",
            "smtp://smtp.example.com:587?tls=required",
        ] {
            let mailer = Mailer::from_url(url, true).unwrap();
            assert!(!format!("{mailer:?}").contains("secret"), "{mailer:?}");
        }
        // Development takes every transport.
        for url in ["smtp://localhost:1025", "file:///tmp", "memory:"] {
            assert!(Mailer::from_url(url, false).is_ok(), "{url}");
        }
    }
}
