//! enfait's MCP server: an agent (Claude Code, Claude Desktop, a local
//! model with an MCP client) reads and changes the session — storyboard,
//! library, royalty-free search, sound finish — and asks the open app for
//! generations.
//!
//! - [`server`]: the protocol (JSON-RPC over standard input and output);
//! - [`tools`]: the tools;
//! - [`bridge`]: the file bridge with the app.

pub mod bridge;
pub mod server;
pub mod tools;

pub use server::Server;
pub use tools::Tools;
