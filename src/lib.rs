//! treff — a small forum for closed groups, authenticated by your own OIDC
//! provider.
//!
//! The crate is a library with a thin binary on top. That is not decoration:
//! the integration tests drive the router directly, and a binary-only crate
//! has nothing for them to call.

pub mod articles;
pub mod auth;
pub mod authz;
pub mod config;
pub mod db;
pub mod i18n;
pub mod markup;
pub mod media;
pub mod web;
