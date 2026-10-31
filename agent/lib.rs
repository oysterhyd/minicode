pub mod attachments;
pub mod background;
pub mod bridge;
pub mod cli;
mod command_process;
pub mod configuration;
pub mod context;
mod delegate_workspace;
mod editing;
pub mod eval;
mod extension_cache;
pub mod extensions;
pub mod goals;
pub mod mcp;
mod mcp_dispatch;
pub mod models;
pub mod paging;
pub mod permissions;
pub mod provider;
pub mod report;
pub mod runtime;
pub mod sandbox;
mod settings_file;
mod sse;
pub mod store;
mod store_writer;
mod subagent_result;
mod textfile;
pub mod tools;
pub mod util;

#[cfg(test)]
mod tests;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
