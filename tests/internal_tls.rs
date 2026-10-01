//! TLS on the internal listener (homeserver audit 3, B145).
//!
//! These tests start **the program**, not a router: what is promised here is
//! what a listener does on a socket and what the process says and refuses at
//! startup, and neither exists below `main`. Each test gets its own process,
//! its own data directory and its own ports.
//!
//! The certificate is made up at run time (`rcgen`); no key is checked in.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use treff::web::tls::{CERT_FILE, KEY_FILE, PLAIN_HTTP_WARNING};

mod common;

const SCIM_TOKEN: &str = "scim-token-0123456789";

/// A certificate for `localhost` and its key, both as PEM.
struct Pair {
    cert: String,
    key: String,
}

fn pair() -> Pair {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("a self-signed certificate");
    Pair {
        cert: made.cert.pem(),
        key: made.signing_key.serialize_pem(),
    }
}

/// Two ports nobody is listening on right now.
fn free_ports() -> (u16, u16) {
    let a = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let b = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    (
        a.local_addr().expect("address").port(),
        b.local_addr().expect("address").port(),
    )
}

/// One treff process and everything it wrote to stderr so far.
struct Treff {
    child: Child,
    said: Arc<Mutex<String>>,
    public: u16,
    internal: u16,
    dir: tempfile::TempDir,
}

impl Drop for Treff {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Treff {
    fn said(&self) -> String {
        self.said.lock().expect("stderr").clone()
    }

    /// Waits for a line to show up on stderr; panics with everything that
    /// was said if it does not.
    fn wait_for(&mut self, needle: &str) {
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            if self.said().contains(needle) {
                return;
            }
            if let Some(status) = self.child.try_wait().expect("status") {
                // The reader thread may still be draining the pipe.
                std::thread::sleep(Duration::from_millis(200));
                if self.said().contains(needle) {
                    return;
                }
                panic!(
                    "treff ended ({status}) before saying {needle:?}:\n{}",
                    self.said()
                );
            }
            assert!(
                Instant::now() < until,
                "treff never said {needle:?}:\n{}",
                self.said()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Waits for the process to end by itself and says whether it failed.
    /// A process that is still running after ten seconds started — which is
    /// the fault the callers are looking for.
    fn refuses_to_start(mut self) -> String {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().expect("status") {
                std::thread::sleep(Duration::from_millis(200));
                assert!(!status.success(), "treff ended with success");
                return self.said();
            }
            assert!(
                Instant::now() < until,
                "treff started although it had to refuse:\n{}",
                self.said()
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

/// Starts treff with the internal listener, the SCIM token and whatever
/// `extra` adds. `files` are written into the data directory first; a value
/// in `extra` that names one of them is replaced by its path.
fn start(files: &[(&str, &str)], extra: &[(&str, &str)]) -> Treff {
    let dir = tempfile::tempdir().expect("a directory");
    let path = |name: &str| dir.path().join(name).to_string_lossy().into_owned();
    std::fs::write(path("spaces.toml"), common::CONFIGURATION).expect("configuration");
    std::fs::write(path("oidc"), "the-client-secret").expect("secret");
    std::fs::write(path("scim"), SCIM_TOKEN).expect("token");
    for (name, content) in files {
        std::fs::write(path(name), content).expect("file");
    }
    let (public, internal) = free_ports();

    let mut command = Command::new(env!("CARGO_BIN_EXE_treff"));
    // Whatever the machine running the tests has set must not decide them.
    for (name, _) in std::env::vars() {
        if name.starts_with("TREFF_") {
            command.env_remove(name);
        }
    }
    command
        .env("TREFF_CONFIG", path("spaces.toml"))
        .env("TREFF_DATA_DIR", path("data"))
        .env("TREFF_LISTEN", format!("127.0.0.1:{public}"))
        .env("TREFF_OIDC_ISSUER", "https://auth.example.org/")
        .env("TREFF_OIDC_CLIENT_ID", "treff")
        .env("TREFF_OIDC_CLIENT_SECRET_FILE", path("oidc"))
        .env("TREFF_INTERNAL_LISTEN", format!("127.0.0.1:{internal}"))
        .env("TREFF_SCIM_TOKEN_FILE", path("scim"));
    for (name, value) in extra {
        if files.iter().any(|(file, _)| file == value) {
            command.env(name, path(value));
        } else {
            command.env(name, value);
        }
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("treff starts as a process");

    let said = Arc::new(Mutex::new(String::new()));
    let stderr = child.stderr.take().expect("stderr");
    let sink = said.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let mut all = sink.lock().expect("stderr");
            all.push_str(&line);
            all.push('\n');
        }
    });
    Treff {
        child,
        said,
        public,
        internal,
        dir,
    }
}

fn with_tls(pair: &Pair) -> Treff {
    start(
        &[("cert.pem", &pair.cert), ("key.pem", &pair.key)],
        &[(CERT_FILE, "cert.pem"), (KEY_FILE, "key.pem")],
    )
}

fn request(port: u16, path: &str, token: &str) -> String {
    format!(
        "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer {token}\r\n\
         Connection: close\r\n\r\n"
    )
}

/// Reads until the other side is done, however it ends — a server that
/// closes without a word is an answer too.
async fn everything<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> String {
    let mut all = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.read(&mut buffer)).await {
            Ok(Ok(n)) if n > 0 => all.extend_from_slice(&buffer[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&all).into_owned()
}

/// An HTTPS client that trusts exactly `cert` and checks the name
/// `localhost` against it — what a real caller does.
async fn over_tls(port: u16, cert: &str, path: &str, token: &str) -> std::io::Result<String> {
    let mut roots = rustls::RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(cert.as_bytes()) {
        roots
            .add(c.expect("a certificate"))
            .expect("a usable trust anchor");
    }
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let name = ServerName::try_from("localhost").expect("a name");
    let mut tls = connector.connect(name, tcp).await?;
    tls.write_all(request(port, path, token).as_bytes()).await?;
    Ok(everything(&mut tls).await)
}

async fn in_the_clear(port: u16, path: &str, token: &str) -> String {
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("a connection");
    tcp.write_all(request(port, path, token).as_bytes())
        .await
        .expect("written");
    everything(&mut tcp).await
}

/// With both files set the door answers over TLS — to a client that trusts
/// the certificate and checks its name — and the refusal middleware sees the
/// request exactly as it did over plain HTTP.
#[tokio::test]
async fn the_internal_door_answers_over_tls_and_logs_its_refusals() {
    let pair = pair();
    let mut treff = with_tls(&pair);
    treff.wait_for("treff: listening on");

    let wrong = over_tls(
        treff.internal,
        &pair.cert,
        "/scim/v2/Users",
        "not-the-token",
    )
    .await
    .expect("a TLS connection");
    assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong:?}");
    let line = format!(
        "treff: refused kind=token status=401 method=GET host=localhost:{} \
         path=/scim/v2/Users reason=scim",
        treff.internal
    );
    treff.wait_for(&line);

    // And the door opens for the right token, so the 401 above is the
    // route's own answer and not a listener that refuses everything.
    let right = over_tls(
        treff.internal,
        &pair.cert,
        "/scim/v2/ServiceProviderConfig",
        SCIM_TOKEN,
    )
    .await
    .expect("a TLS connection");
    assert!(right.starts_with("HTTP/1.1 200"), "{right:?}");

    let said = treff.said();
    assert!(
        said.contains(&format!(
            "treff: the internal listener is on 127.0.0.1:{}, TLS only",
            treff.internal
        )),
        "{said}"
    );
    assert!(!said.contains("WARNING"), "{said}");

    // THE PUBLIC LISTENER IS NOT TOUCHED: still plain HTTP, still refusing
    // a host it does not serve.
    let public = in_the_clear(treff.public, "/", "-").await;
    assert!(public.starts_with("HTTP/1.1 403"), "{public:?}");
}

/// The same port, asked in plain HTTP, gives no answer from treff: no status
/// line, no refusal — the request never reaches a route.
#[tokio::test]
async fn the_tls_port_does_not_answer_plain_http() {
    let pair = pair();
    let mut treff = with_tls(&pair);
    treff.wait_for("treff: listening on");

    let answer = in_the_clear(treff.internal, "/scim/v2/Users", "not-the-token").await;
    assert!(
        !answer.contains("HTTP/"),
        "plain HTTP was answered: {answer:?}"
    );

    // It is not silent about it either: the caller that still speaks plain
    // HTTP is named in the log.
    treff.wait_for("treff: the internal listener dropped a connection from 127.0.0.1");
    assert!(
        !treff.said().contains("treff: refused"),
        "a route answered: {}",
        treff.said()
    );
}

/// A client that does not trust the certificate gets nothing either — the
/// token is never sent, because there is no connection to send it on.
#[tokio::test]
async fn a_client_that_does_not_trust_the_certificate_gets_no_connection() {
    let pair = pair();
    let other = self::pair();
    let mut treff = with_tls(&pair);
    treff.wait_for("treff: listening on");

    let answer = over_tls(treff.internal, &other.cert, "/scim/v2/Users", SCIM_TOKEN).await;
    // The CLIENT turned the certificate down — which it can only do if the
    // listener presented one.
    let why = format!("{:?}", answer.expect_err("no connection"));
    assert!(why.contains("InvalidCertificate"), "{why}");
}

/// Without the two files the door is what it was — plain HTTP — and treff
/// says so at startup, in one line an operator cannot take for good news.
#[tokio::test]
async fn without_a_certificate_the_door_speaks_plain_http_and_says_so() {
    let mut treff = start(&[], &[]);
    treff.wait_for("treff: listening on");

    let said = treff.said();
    assert!(
        said.lines().any(|l| l == PLAIN_HTTP_WARNING),
        "no warning: {said}"
    );
    assert!(!said.contains("TLS only"), "{said}");

    let wrong = in_the_clear(treff.internal, "/scim/v2/Users", "not-the-token").await;
    assert!(wrong.starts_with("HTTP/1.1 401"), "{wrong:?}");
}

/// The last thing treff said before it ended — the reason.
fn reason(said: &str) -> &str {
    said.lines().last().unwrap_or_default()
}

/// One file without the other is somebody who meant the door to be
/// encrypted. treff does not start — and above all does not start in HTTP.
#[tokio::test]
async fn one_tls_file_without_the_other_stops_the_start() {
    let pair = pair();

    let only_cert = start(&[("cert.pem", &pair.cert)], &[(CERT_FILE, "cert.pem")]);
    let said = only_cert.refuses_to_start();
    assert_eq!(
        reason(&said),
        "treff: TREFF_INTERNAL_TLS_CERT_FILE is set and TREFF_INTERNAL_TLS_KEY_FILE is not; \
         the internal listener takes both or neither, and never falls back to plain HTTP",
        "{said}"
    );
    assert!(!said.contains("the internal listener is on"), "{said}");

    let only_key = start(&[("key.pem", &pair.key)], &[(KEY_FILE, "key.pem")]);
    let said = only_key.refuses_to_start();
    assert_eq!(
        reason(&said),
        "treff: TREFF_INTERNAL_TLS_KEY_FILE is set and TREFF_INTERNAL_TLS_CERT_FILE is not; \
         the internal listener takes both or neither, and never falls back to plain HTTP",
        "{said}"
    );
    assert!(!said.contains("the internal listener is on"), "{said}");
}

/// Every line of a PEM body, to look for in what treff said.
fn body_lines(pem: &str) -> Vec<&str> {
    pem.lines()
        .filter(|l| !l.starts_with("-----") && !l.is_empty())
        .collect()
}

/// A file that is there and is not PEM stops the start, and the message
/// carries the variable and the path — never a line of the file. The PEM
/// parser underneath quotes the line it stumbled over; in a key file that
/// line is the key.
#[tokio::test]
async fn a_broken_pem_stops_the_start_and_nothing_of_it_is_printed() {
    let pair = pair();
    let key_body = body_lines(&pair.key);
    let said_nothing_of_the_key = |said: &str| {
        for line in &key_body {
            assert!(!said.contains(line), "the key is in the message: {said}");
        }
    };

    // A key whose first line ran into its header: an illegal section start,
    // and the offending line IS key material.
    let glued = pair.key.replacen(
        "-----BEGIN PRIVATE KEY-----\n",
        "-----BEGIN PRIVATE KEY-----",
        1,
    );
    assert_ne!(glued, pair.key, "the test key is not PKCS#8 any more");
    let treff = start(
        &[("cert.pem", &pair.cert), ("key.pem", &glued)],
        &[(CERT_FILE, "cert.pem"), (KEY_FILE, "key.pem")],
    );
    let key_path = treff.dir.path().join("key.pem");
    let said = treff.refuses_to_start();
    assert_eq!(
        reason(&said),
        format!(
            "treff: TREFF_INTERNAL_TLS_KEY_FILE ({}) is not valid PEM",
            key_path.display()
        ),
        "{said}"
    );
    said_nothing_of_the_key(&said);

    // A key with something in it that is not base64.
    let spoiled = pair.key.replacen('\n', "\n!!! not base64 !!!\n", 1);
    let treff = start(
        &[("cert.pem", &pair.cert), ("key.pem", &spoiled)],
        &[(CERT_FILE, "cert.pem"), (KEY_FILE, "key.pem")],
    );
    let said = treff.refuses_to_start();
    assert!(
        reason(&said).ends_with("key.pem) is not valid PEM"),
        "{said}"
    );
    said_nothing_of_the_key(&said);

    // A certificate that is no PEM at all.
    let treff = start(
        &[
            ("cert.pem", "this is not a certificate\n"),
            ("key.pem", &pair.key),
        ],
        &[(CERT_FILE, "cert.pem"), (KEY_FILE, "key.pem")],
    );
    let cert_path = treff.dir.path().join("cert.pem");
    let said = treff.refuses_to_start();
    assert_eq!(
        reason(&said),
        format!(
            "treff: TREFF_INTERNAL_TLS_CERT_FILE ({}) holds no certificate",
            cert_path.display()
        ),
        "{said}"
    );

    // THE TWO PATHS SWAPPED — the mistake that puts the key where a
    // certificate is expected.
    let treff = start(
        &[("cert.pem", &pair.cert), ("key.pem", &pair.key)],
        &[(CERT_FILE, "key.pem"), (KEY_FILE, "cert.pem")],
    );
    let said = treff.refuses_to_start();
    assert!(
        reason(&said).ends_with("key.pem) holds no certificate"),
        "{said}"
    );
    said_nothing_of_the_key(&said);
}

/// A path that leads nowhere, and a key that belongs to another certificate.
#[tokio::test]
async fn a_missing_file_or_a_key_for_another_certificate_stops_the_start() {
    let pair = pair();

    let treff = start(
        &[("cert.pem", &pair.cert)],
        &[
            (CERT_FILE, "cert.pem"),
            (KEY_FILE, "/nonexistent/treff/key.pem"),
        ],
    );
    let said = treff.refuses_to_start();
    assert!(
        reason(&said).starts_with(
            "treff: TREFF_INTERNAL_TLS_KEY_FILE (/nonexistent/treff/key.pem) cannot be read: "
        ),
        "{said}"
    );

    let other = self::pair();
    let treff = start(
        &[("cert.pem", &pair.cert), ("key.pem", &other.key)],
        &[(CERT_FILE, "cert.pem"), (KEY_FILE, "key.pem")],
    );
    let said = treff.refuses_to_start();
    assert!(
        reason(&said).starts_with(
            "treff: the certificate in TREFF_INTERNAL_TLS_CERT_FILE and the key in \
             TREFF_INTERNAL_TLS_KEY_FILE cannot be used: "
        ),
        "{said}"
    );
    for line in body_lines(&other.key) {
        assert!(!said.contains(line), "the key is in the message: {said}");
    }
}
