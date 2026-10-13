//! Domain types shared by `webb` and the application: authorization data
//! (`AuthData`, `TokenSet`, `AuthorizeInfo`, `AuthClaims`) and the EVE entities
//! `Character`, `Corporation` and `Alliance`.
//!
//! Each entity lives in its own submodule; everything is re-exported here so
//! the public path (`webb::objects::Character`, ...) does not change.

mod alliance;
mod auth;
mod catalog;
mod character;
mod corporation;

pub use alliance::Alliance;
pub use auth::{AuthClaims, AuthData, AuthorizeInfo, TokenSet};
pub use catalog::BasicCatalog;
pub use character::{Character, CharacterPublicInfo};
pub use corporation::Corporation;
