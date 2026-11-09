//! OTL-4: an `https` endpoint trusts the system's roots, proved by a handshake.
//!
//! The system's roots are read from `SSL_CERT_FILE` when it is set, so the test makes
//! a root of its own, and runs this same binary again as a child with `SSL_CERT_FILE`
//! naming only that root. The child exports to two collectors on `https`: one whose
//! certificate that root signed, which must receive the span, and one whose certificate
//! another root signed, which must not. A process sets a variable for a child alone,
//! and so this needs no `unsafe`.

use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axa_opentelemetry::{Config, start};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
    trace_service_server::{TraceService, TraceServiceServer},
};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair};
use tokio::net::TcpListener;
use tonic::transport::{Identity, Server, ServerTlsConfig};
use tracing_subscriber::{Registry, layer::SubscriberExt};

/// Being set makes a process the child, and names the directory of certificates.
const ROLE: &str = "AXA_OTEL_TLS_TEST_DIR";

/// The spans a collector has received.
#[derive(Clone, Default)]
struct Collector(Arc<Mutex<Vec<ExportTraceServiceRequest>>>);

impl Collector {
    fn received(&self) -> bool {
        !self.0.lock().unwrap().is_empty()
    }
}

#[tonic::async_trait]
impl TraceService for Collector {
    async fn export(
        &self,
        request: tonic::Request<ExportTraceServiceRequest>,
    ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
        self.0.lock().unwrap().push(request.into_inner());
        Ok(tonic::Response::new(ExportTraceServiceResponse::default()))
    }
}

/// A root, and a certificate it signed for `127.0.0.1`, written to `dir` as
/// `<name>-root.pem`, `<name>-cert.pem`, and `<name>-key.pem`.
fn certificates(dir: &Path, name: &str) {
    let mut root = CertificateParams::new(Vec::<String>::new()).unwrap();
    root.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root.distinguished_name
        .push(DnType::CommonName, format!("axa test root {name}"));
    let root = CertifiedIssuer::self_signed(root, KeyPair::generate().unwrap()).unwrap();
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["127.0.0.1".to_string()])
        .unwrap()
        .signed_by(&key, &root)
        .unwrap();
    std::fs::write(dir.join(format!("{name}-root.pem")), root.pem()).unwrap();
    std::fs::write(dir.join(format!("{name}-cert.pem")), cert.pem()).unwrap();
    std::fs::write(dir.join(format!("{name}-key.pem")), key.serialize_pem()).unwrap();
}

/// A collector on `https`, with the certificate `name` from `dir`.
async fn collector(dir: &Path, name: &str) -> (String, Collector) {
    let read = |part: &str| std::fs::read(dir.join(format!("{name}-{part}.pem"))).unwrap();
    let identity = Identity::from_pem(read("cert"), read("key"));
    let listener = TcpListener::bind::<SocketAddr>("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let collector = Collector::default();
    let service = collector.clone();
    tokio::spawn(async move {
        Server::builder()
            .tls_config(ServerTlsConfig::new().identity(identity))
            .unwrap()
            .add_service(TraceServiceServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .ok();
    });
    (format!("https://{address}"), collector)
}

/// Export one span to `endpoint`, and wait up to `wait` for `collector` to have it.
async fn export_to(endpoint: String, collector: &Collector, wait: Duration) -> bool {
    let config = Config {
        endpoint: Some(endpoint),
        ..Config::new("tls-test", "1.0.0")
    };
    let telemetry = start(&config).unwrap();
    let subscriber = Registry::default().with(telemetry.layers());
    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!("over-tls").in_scope(|| {});
    });
    telemetry.flush();
    let deadline = Instant::now() + wait;
    while Instant::now() < deadline && !collector.received() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    telemetry.shutdown();
    collector.received()
}

/// What the child does: export over `https` to a collector the system trusts and to
/// one it does not.
async fn child(dir: PathBuf) {
    let (trusted, received) = collector(&dir, "trusted").await;
    assert!(
        export_to(trusted, &received, Duration::from_secs(10)).await,
        "nothing reached a collector whose root the system trusts"
    );
    let (untrusted, received) = collector(&dir, "untrusted").await;
    assert!(
        !export_to(untrusted, &received, Duration::from_secs(2)).await,
        "a collector whose root the system does not trust received a span"
    );
}

#[test]
fn an_https_endpoint_completes_a_handshake_with_a_root_the_system_trusts() {
    if let Some(dir) = std::env::var_os(ROLE) {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(child(PathBuf::from(dir)));
        return;
    }
    let dir = std::env::temp_dir().join(format!("axa-otel-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    certificates(&dir, "trusted");
    certificates(&dir, "untrusted");
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "an_https_endpoint_completes_a_handshake_with_a_root_the_system_trusts",
            "--exact",
            "--nocapture",
        ])
        .env(ROLE, &dir)
        .env("SSL_CERT_FILE", dir.join("trusted-root.pem"))
        .env_remove("SSL_CERT_DIR")
        .output()
        .unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert!(
        output.status.success(),
        "the child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
