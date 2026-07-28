//! Server-authoritative membership primitives.
//!
//! This crate deliberately contains no local production unlock path. The desktop
//! application may cache server-signed leases, but only an ES256 signature from a
//! release-pinned public key can grant a capability.

pub mod audit;
pub mod auth_client;
pub mod device_identity;
pub mod entitlement;
pub mod entitlement_guard;
pub mod error;
mod es256_provider;
pub mod license_api;
pub mod models;
pub mod release_integrity;
pub mod secure_store;
pub mod service;
pub mod trusted_time;
pub mod version_policy;

pub use audit::*;
pub use auth_client::*;
pub use device_identity::*;
pub use entitlement::*;
pub use entitlement_guard::*;
pub use error::*;
pub use license_api::*;
pub use models::*;
pub use release_integrity::*;
pub use secure_store::*;
pub use service::*;
pub use trusted_time::*;
pub use version_policy::*;
