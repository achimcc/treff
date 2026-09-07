//! The way out, and why it does not ask anybody to sign in.
//!
//! A notification that can only be stopped by signing in does not have an
//! unsubscribe link, it has a sign-in link. The person who wanted it to stop
//! will reach for their mail client's spam button instead — and that costs
//! the whole domain, not one subscription.
//!
//! So the link carries its own proof: an HMAC over the outbox row it came
//! from. From that row follow the person and the topic, which means **the
//! address of the link says nothing** — no subject, no name, nothing that
//! ends up legible in a proxy log.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Sixteen bytes of tag. Not thirty-two: this authenticates a row id against
/// forgery, and a URL people paste into a browser is worth keeping short. The
/// key is 64 bytes, which is what carries the strength.
const TAG_BYTES: usize = 16;

/// A key of its own, never the cookie key.
///
/// TWO PURPOSES, TWO KEYS. Somebody who obtained this one could unsubscribe
/// strangers; if it were also the cookie key they could sign in as them. The
/// blast radius of a leak is a design decision, and this is where it is made.
pub fn load_or_create_key(dir: &std::path::Path) -> anyhow::Result<Vec<u8>> {
    let path = dir.join("unsubscribe.key");
    if let Ok(bytes) = std::fs::read(&path)
        && bytes.len() >= 32
    {
        return Ok(bytes);
    }
    let mut key = vec![0u8; 64];
    rand::rngs::SysRng.try_fill_bytes(&mut key)?;
    std::fs::write(&path, &key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(key)
}

pub fn token(key: &[u8], outbox_id: i64) -> String {
    use base64::Engine as _;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(b"treff-unsubscribe-v1:");
    mac.update(outbox_id.to_string().as_bytes());
    let tag = mac.finalize().into_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&tag[..TAG_BYTES])
}

/// Constant-time by construction: the comparison happens inside `verify_slice`,
/// which is what `hmac` provides it for. A `==` on the strings would leak the
/// tag one byte at a time to anybody willing to make enough requests.
pub fn verify(key: &[u8], outbox_id: i64, presented: &str) -> bool {
    use base64::Engine as _;
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(presented) else {
        return false;
    };
    if bytes.len() != TAG_BYTES {
        return false;
    }
    // `verify_truncated_left` compares the first `bytes.len()` bytes of the
    // real tag against what was presented, in constant time. That is exactly
    // the shape of this token: a truncated MAC.
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(b"treff-unsubscribe-v1:");
    mac.update(outbox_id.to_string().as_bytes());
    mac.verify_truncated_left(&bytes).is_ok()
}

use rand::TryRng as _;

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        vec![7u8; 64]
    }

    #[test]
    fn a_token_belongs_to_one_row_and_no_other() {
        let k = key();
        assert!(verify(&k, 42, &token(&k, 42)));
        assert!(
            !verify(&k, 43, &token(&k, 42)),
            "a token for another row is not a token"
        );
    }

    #[test]
    fn a_token_from_another_key_is_refused() {
        assert!(!verify(&[9u8; 64], 42, &token(&key(), 42)));
    }

    /// Everything unparseable answers the same way. A truncated token that
    /// said "wrong length" and a wrong token that said "wrong tag" would
    /// together be a way to learn how the token is built.
    #[test]
    fn garbage_is_refused_without_a_hint() {
        let k = key();
        for wrong in [
            "",
            "!!!!",
            "AAAA",
            &token(&k, 42)[..8],
            "a".repeat(200).as_str(),
        ] {
            assert!(!verify(&k, 42, wrong), "{wrong:?} was accepted");
        }
    }
}
