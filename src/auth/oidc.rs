//! The OIDC conversation: discovery at startup, the redirect out, the
//! callback back.
//!
//! What this module does **not** do is re-implement token verification.
//! Checking the ID token's signature against the provider's keys, and checking
//! the nonce, is `openidconnect`'s job; imitating it here with home-made
//! fixtures would test the library rather than this code. What is tested here
//! is what we decide: that the outgoing request carries PKCE, `state` and
//! `nonce`, and that a callback whose `state` does not match is refused before
//! anything else happens.

use crate::auth::{OidcSettings, claims_to_identity};
use crate::authz::Identity;
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet, EndpointNotSet,
    EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse,
};

/// The client type after `set_redirect_uri`. The endpoint markers are part of
/// the type in openidconnect 4, so storing a configured client means naming
/// them.
type Client = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

pub struct Provider {
    client: Client,
    http: openidconnect::reqwest::Client,
    group_claim: String,
}

/// What a sign-in needs to remember between the redirect out and the callback
/// back. It travels in a short-lived private cookie, never in a URL.
#[derive(Debug, Clone)]
pub struct PendingLogin {
    pub state: String,
    pub nonce: String,
    pub pkce_verifier: String,
}

pub struct LoginStart {
    pub url: String,
    pub pending: PendingLogin,
}

/// Compares two `state` values without an early exit.
///
/// An empty stored state is **not** a wildcard: a callback that arrives
/// without a login having started is refused, which is what makes this a CSRF
/// defence rather than a formality.
pub fn states_match(stored: &str, returned: &str) -> bool {
    if stored.is_empty() || stored.len() != returned.len() {
        return false;
    }
    stored
        .bytes()
        .zip(returned.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

impl Provider {
    /// Discovery happens once, at startup: a provider that cannot be reached
    /// is a reason not to serve, not something to retry per request.
    pub async fn discover(settings: &OidcSettings, redirect_uri: &str) -> anyhow::Result<Self> {
        // Following redirects here would open the door to SSRF, so the client
        // does not follow any.
        let http = openidconnect::reqwest::ClientBuilder::new()
            .redirect(openidconnect::reqwest::redirect::Policy::none())
            .build()?;

        let metadata =
            CoreProviderMetadata::discover_async(IssuerUrl::new(settings.issuer.clone())?, &http)
                .await?;

        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(settings.client_id.clone()),
            Some(ClientSecret::new(settings.client_secret.clone())),
        )
        .set_redirect_uri(RedirectUrl::new(redirect_uri.to_string())?);

        Ok(Self {
            client,
            http,
            group_claim: settings.group_claim.clone(),
        })
    }

    pub fn begin_login(&self) -> anyhow::Result<LoginStart> {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, csrf, nonce) = self
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("profile".to_string()))
            .set_pkce_challenge(challenge)
            .url();

        Ok(LoginStart {
            url: url.to_string(),
            pending: PendingLogin {
                state: csrf.into_secret(),
                nonce: nonce.secret().clone(),
                pkce_verifier: verifier.into_secret(),
            },
        })
    }

    /// Exchanges the code for tokens and turns the ID token's claims into an
    /// identity.
    ///
    /// The `state` is checked **first**, before the provider is contacted at
    /// all: a callback that nobody started is not worth a network request.
    pub async fn finish_login(
        &self,
        pending: PendingLogin,
        code: &str,
        returned_state: &str,
    ) -> anyhow::Result<Identity> {
        if !states_match(&pending.state, returned_state) {
            anyhow::bail!("the callback state does not match the one this session started with");
        }

        let tokens = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_string()))?
            .set_pkce_verifier(PkceCodeVerifier::new(pending.pkce_verifier))
            .request_async(&self.http)
            .await?;

        let id_token = tokens
            .id_token()
            .ok_or_else(|| anyhow::anyhow!("the provider returned no ID token"))?;
        let claims =
            id_token.claims(&self.client.id_token_verifier(), &Nonce::new(pending.nonce))?;

        // The claims we need beyond the standard ones (the groups) live in the
        // additional claims, which reach us as plain JSON.
        let extra = serde_json::to_value(claims.additional_claims())?;
        let name = claims
            .name()
            .and_then(|n| n.get(None))
            .map(|n| n.as_str().to_string());
        let preferred = claims.preferred_username().map(|u| u.as_str().to_string());

        Ok(claims_to_identity(
            claims.subject().as_str(),
            name.as_deref().or(preferred.as_deref()),
            &extra,
            &self.group_claim,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn provider_at(server: &MockServer) -> Provider {
        let issuer = server.uri();
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": issuer,
                "authorization_endpoint": format!("{issuer}/authorize"),
                "token_endpoint": format!("{issuer}/token"),
                "jwks_uri": format!("{issuer}/jwks"),
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"],
            })))
            .mount(server)
            .await;

        // Discovery fetches the signing keys in the same breath; without this
        // second stub the call fails with a 404 that looks like a bad URL.
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "keys": []
            })))
            .mount(server)
            .await;

        let settings = crate::auth::OidcSettings {
            issuer,
            client_id: "treff".into(),
            client_secret: "s3cret".into(),
            group_claim: "groups".into(),
        };
        Provider::discover(&settings, "https://forum.example.org/auth/callback")
            .await
            .expect("discovery")
    }

    #[tokio::test]
    async fn the_login_url_carries_pkce_state_and_nonce() {
        let server = MockServer::start().await;
        let provider = provider_at(&server).await;

        let start = provider.begin_login().expect("begin");
        let url = start.url;

        assert!(
            url.starts_with(&format!("{}/authorize", server.uri())),
            "{url}"
        );
        for expected in [
            "code_challenge_method=S256",
            "code_challenge=",
            "state=",
            "nonce=",
            "client_id=treff",
            "response_type=code",
            "scope=openid+profile",
        ] {
            assert!(url.contains(expected), "{expected} missing from {url}");
        }
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fforum.example.org%2Fauth%2Fcallback"),
            "{url}"
        );
        // The verifier is the half that must NOT travel.
        assert!(!url.contains(start.pending.pkce_verifier.as_str()), "{url}");
    }

    #[tokio::test]
    async fn two_logins_do_not_share_their_secrets() {
        let server = MockServer::start().await;
        let provider = provider_at(&server).await;
        let a = provider.begin_login().expect("begin");
        let b = provider.begin_login().expect("begin");
        assert_ne!(a.pending.state, b.pending.state);
        assert_ne!(a.pending.nonce, b.pending.nonce);
        assert_ne!(a.pending.pkce_verifier, b.pending.pkce_verifier);
    }

    #[tokio::test]
    async fn a_callback_with_the_wrong_state_is_refused_without_asking_the_provider() {
        // The token endpoint is deliberately NOT mocked: if the state check
        // did not come first, this test would hang or fail on a connection
        // error instead of the assertion below.
        let server = MockServer::start().await;
        let provider = provider_at(&server).await;
        let start = provider.begin_login().expect("begin");

        let err = provider
            .finish_login(start.pending, "any-code", "not-the-state")
            .await
            .expect_err("must refuse");
        assert!(
            err.to_string().contains("state"),
            "refused for the wrong reason: {err}"
        );
    }

    #[test]
    fn state_comparison_rejects_near_misses() {
        assert!(states_match("abc", "abc"));
        assert!(!states_match("abc", "abd"));
        assert!(!states_match("abc", "abc "));
        assert!(!states_match("abc", "ab"));
        assert!(!states_match("", "abc"));
        // An empty stored state must never be a wildcard.
        assert!(!states_match("", ""));
    }
}
