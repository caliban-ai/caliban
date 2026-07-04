//! Wire-format schema types for the Ollama adapter.

pub mod probe;
pub mod request;
pub mod response;

pub use probe::{ModelShow, RunningModel, RunningModelList, TagDetails, TagEntry, TagList};
pub use request::*;
pub use response::*;
