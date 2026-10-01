//! TLS for the internal listener (homeserver audit 3, B145).
//!
//! The internal door (ADR 0006) carries bearer tokens and what they open —
//! people, groups, the entries of somebody's bell — and until 0.12.0 it spoke
//! plain HTTP, also where "internal" meant another machine or another zone.
//! With a certificate and a key it now speaks TLS, and then **only** TLS: a
//! plain request on that port gets no answer from treff at all.
//!
//! **Both files or neither.** One without the other, a file that cannot be
//! read, or one that is not PEM stops treff at startup. There is no falling
//! back to plain HTTP: somebody who set one of the two meant the door to be
//! encrypted, and a door that quietly is not would be the fault this exists
//! to close.
//!
//! **Nothing from either file reaches a message.** The PEM parser's own
//! errors quote the line they stumbled over, which in a key file is the key —
//! so they are replaced by fixed words here and never printed.
//!
//! The public listener is not touched by any of this: it stays plain HTTP
//! behind the reverse proxy that terminates TLS for the browsers.

use rustls::ServerConfig;
use rustls::pki_types::pem::{Error as PemError, PemObject};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// The certificate the internal listener presents: PEM, the leaf first and
/// then whatever chain belongs to it.
pub const CERT_FILE: &str = "TREFF_INTERNAL_TLS_CERT_FILE";
/// Its private key: PEM, as PKCS#8, PKCS#1 or SEC1.
pub const KEY_FILE: &str = "TREFF_INTERNAL_TLS_KEY_FILE";

/// What treff says at startup when the internal listener has no certificate.
/// It still starts — that is what every installation before 0.12.0 does —
/// but not without saying so.
pub const PLAIN_HTTP_WARNING: &str = "treff: WARNING: the internal listener speaks plain HTTP: \
     its tokens and its answers are readable on the wire. \
     Set TREFF_INTERNAL_TLS_CERT_FILE and TREFF_INTERNAL_TLS_KEY_FILE.";

/// How long a connection may take to finish its handshake before it is
/// dropped. Generous for a neighbour on the same network, short enough that
/// a connection which never speaks does not stay.
const HANDSHAKE: Duration = Duration::from_secs(10);

/// How many handshakes may be under way at once. Beyond that, new
/// connections wait in the kernel's queue rather than in treff's memory.
const HANDSHAKES: usize = 256;

/// The TLS configuration for the internal listener, from the environment.
///
/// `None` when neither variable is set. Exactly one of them set is an error,
/// and so is everything [`load`] refuses.
pub fn from_env() -> anyhow::Result<Option<Arc<ServerConfig>>> {
    match (std::env::var(CERT_FILE).ok(), std::env::var(KEY_FILE).ok()) {
        (None, None) => Ok(None),
        (Some(cert), Some(key)) => Ok(Some(load(&cert, &key)?)),
        (Some(_), None) => Err(only(CERT_FILE, KEY_FILE)),
        (None, Some(_)) => Err(only(KEY_FILE, CERT_FILE)),
    }
}

fn only(set: &str, missing: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{set} is set and {missing} is not; the internal listener takes both or neither, \
         and never falls back to plain HTTP"
    )
}

/// Reads a certificate chain and its key and builds the configuration:
/// TLS 1.3 and 1.2 and nothing older, `ring` for the cryptography, no client
/// certificates — the bearer token stays what opens a route.
///
/// Every failure names the variable and the path, and nothing of the content.
pub fn load(cert_path: &str, key_path: &str) -> anyhow::Result<Arc<ServerConfig>> {
    let read = |name: &str, path: &str| -> anyhow::Result<Vec<u8>> {
        std::fs::read(path).map_err(|e| anyhow::anyhow!("{name} ({path}) cannot be read: {e}"))
    };
    let cert_pem = read(CERT_FILE, cert_path)?;
    let key_pem = read(KEY_FILE, key_path)?;

    // THE PARSER'S ERROR IS DROPPED ON PURPOSE, here and below: it quotes the
    // offending line, and the two paths are easily swapped.
    let not_pem = |name: &str, path: &str| anyhow::anyhow!("{name} ({path}) is not valid PEM");

    let mut chain = Vec::new();
    for cert in CertificateDer::pem_slice_iter(&cert_pem) {
        chain.push(cert.map_err(|_| not_pem(CERT_FILE, cert_path))?);
    }
    if chain.is_empty() {
        anyhow::bail!("{CERT_FILE} ({cert_path}) holds no certificate");
    }

    let key = match PrivateKeyDer::from_pem_slice(&key_pem) {
        Ok(key) => key,
        Err(PemError::NoItemsFound) => {
            anyhow::bail!("{KEY_FILE} ({key_path}) holds no private key")
        }
        Err(_) => return Err(not_pem(KEY_FILE, key_path)),
    };

    // The provider is named rather than taken from a process-wide default:
    // lettre and the OIDC client bring rustls with `ring` as well, and a
    // second provider in the binary would make "the default" a matter of
    // which crate asked first.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|builder| builder.with_no_client_auth().with_single_cert(chain, key))
        // rustls' own error is an enum without key material in it: an
        // unsupported key type, or a key that does not belong to the
        // certificate.
        .map_err(|e| {
            anyhow::anyhow!(
                "the certificate in {CERT_FILE} and the key in {KEY_FILE} cannot be used: {e}"
            )
        })?;
    // axum serves HTTP/1.1 here (its `http2` feature is off), so that is the
    // one protocol offered.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// A listener that hands axum connections whose TLS handshake is done.
///
/// The handshakes run in tasks of their own, [`HANDSHAKES`] at a time and
/// each with [`HANDSHAKE`] to finish, so one connection that never says
/// anything cannot keep the next caller waiting. What does not finish — a
/// plain HTTP request, a scanner, a client that does not trust the
/// certificate — is closed without a byte of HTTP.
pub struct TlsListener {
    local: SocketAddr,
    ready: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
}

impl TlsListener {
    /// Starts accepting. Has to be called inside the runtime.
    pub fn new(listener: TcpListener, config: Arc<ServerConfig>) -> std::io::Result<Self> {
        let local = listener.local_addr()?;
        let (done, ready) = mpsc::channel(HANDSHAKES);
        tokio::spawn(accept_forever(listener, TlsAcceptor::from(config), done));
        Ok(Self { local, ready })
    }
}

async fn accept_forever(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    done: mpsc::Sender<(TlsStream<TcpStream>, SocketAddr)>,
) {
    let slots = Arc::new(Semaphore::new(HANDSHAKES));
    loop {
        // The semaphore is never closed; if it ever were, there is nothing
        // left to accept for.
        let Ok(slot) = slots.clone().acquire_owned().await else {
            return;
        };
        let (tcp, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                // As axum does for its own listener: a connection that broke
                // before it was accepted is nothing; anything else (no file
                // descriptors left, say) is said and waited out.
                if !matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                ) {
                    eprintln!("treff: the internal listener cannot accept: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                continue;
            }
        };
        let acceptor = acceptor.clone();
        let done = done.clone();
        tokio::spawn(async move {
            let _slot = slot;
            match tokio::time::timeout(HANDSHAKE, acceptor.accept(tcp)).await {
                Ok(Ok(tls)) => {
                    // An error here means axum stopped serving; the
                    // connection is dropped with everything else.
                    let _ = done.send((tls, peer)).await;
                }
                // Connected and left without a word: a port probe.
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
                // The caller that still speaks plain HTTP shows up HERE, and
                // nowhere else — it never gets far enough to be refused by a
                // route. The address is the only thing known about it.
                Ok(Err(e)) => eprintln!(
                    "treff: the internal listener dropped a connection from {} \
                     before TLS was established: {e}",
                    peer.ip()
                ),
                Err(_) => eprintln!(
                    "treff: the internal listener dropped a connection from {} \
                     before TLS was established: no handshake within {}s",
                    peer.ip(),
                    HANDSHAKE.as_secs()
                ),
            }
        });
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(connection) => connection,
            // The accepting task is gone; there will be no further
            // connection, and the trait has no way to say so.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE WARNING IS SOMETHING AN OPERATOR GREPS FOR, and the README quotes
    /// it. Spelled out here once; change it only together with the README.
    #[test]
    fn the_warning_is_the_line_the_readme_promises() {
        assert_eq!(
            PLAIN_HTTP_WARNING,
            "treff: WARNING: the internal listener speaks plain HTTP: its tokens and its \
             answers are readable on the wire. Set TREFF_INTERNAL_TLS_CERT_FILE and \
             TREFF_INTERNAL_TLS_KEY_FILE."
        );
        let readme = include_str!("../../README.md");
        assert!(
            readme.lines().any(|l| l.trim() == PLAIN_HTTP_WARNING),
            "the README does not quote the warning"
        );
        for name in [CERT_FILE, KEY_FILE] {
            assert!(readme.contains(&format!("| `{name}` |")), "{name}");
        }
    }
}
