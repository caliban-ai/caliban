//! Native wire-format types for the `OpenAI` Chat Completions API.

pub mod events;
pub mod request;
pub mod response;

pub use request::*;
pub use response::*;
