//! Web tools — `WebFetch` (URL → markdown) and `WebSearch` (provider-backed
//! web search).

pub mod web_fetch;
pub mod web_search;

pub use web_fetch::WebFetchTool;
pub use web_search::{Provider, SearchHit, WebSearchTool};
