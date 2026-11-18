//! Serveur MCP d'enfait : un agent (Claude Code, Claude Desktop, un modèle
//! local avec un client MCP) lit et modifie la session — storyboard,
//! bibliothèque, recherche libre de droits, finition sonore — et demande des
//! générations à l'interface ouverte.
//!
//! - [`server`] : le protocole (JSON-RPC sur l'entrée et la sortie standard) ;
//! - [`tools`] : les outils ;
//! - [`bridge`] : le pont par fichiers avec l'interface.

pub mod bridge;
pub mod server;
pub mod tools;

pub use server::Server;
pub use tools::Tools;
