//! The study's parts, exposed so the tests can hold them to account.
//!
//! The binary in `main.rs` is the four steps wired together; everything that
//! can be wrong in a way that changes the published number lives here.

pub mod analyze;
pub mod fetch;
pub mod github;
pub mod paths;
