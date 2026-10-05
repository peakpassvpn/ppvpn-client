//! PPVPN account client: the backend HTTP API a signed-in app uses and the
//! browser device login that issues its credentials.
//!
//! The crate has no platform code. A host provides:
//! - an [`api::ApiConfig`] (backend base URL, product audiences, language,
//!   local-backend test relaxations);
//! - an [`auth::AuthConfig`] (where the browser authorization page lives);
//! - a [`auth::CredentialStore`] over its secret store (Keychain, Credential
//!   Manager, Secret Service).
//!
//! Errors stay structured ([`api::ApiError`], [`auth::AuthError`]); hosts map
//! them to their own user-facing codes.

pub mod api;
pub mod auth;
