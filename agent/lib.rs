pub mod attachments;
pub mod bridge;
pub mod cli;
mod command_process;
pub mod configuration;
pub mod context;
pub mod eval;
pub mod extensions;
pub mod goals;
pub mod mcp;
pub mod models;
pub mod paging;
pub mod provider;
pub mod report;
pub mod runtime;
pub mod store;
pub mod tools;
pub mod util;

#[cfg(test)]
mod tests;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
