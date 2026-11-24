//! `enfait-cli mcp`: enfait's MCP server over standard input and output,
//! for Claude Code or any MCP client. `--install` registers it in Claude
//! Code (`claude mcp add`).

use anyhow::{Context as _, Result};
use enfait_mcp::bridge::session_dir;
use enfait_mcp::{Server, Tools};

#[derive(Debug, clap::Args)]
pub struct McpArgs {
    /// Registers the server in Claude Code (for all your projects), then
    /// stops.
    #[arg(long)]
    pub install: bool,
}

pub async fn run(args: McpArgs) -> Result<()> {
    let dir = session_dir();
    if args.install {
        return install(&dir);
    }
    tracing::info!("MCP server on session {}", dir.display());
    // Without ENFAIT_SESSION the server follows the project open in the app.
    let tools = if std::env::var_os("ENFAIT_SESSION").is_some() {
        Tools::new(dir)
    } else {
        Tools::following()
    };
    let mut server = Server::new(tools);
    server
        .serve(
            tokio::io::BufReader::new(tokio::io::stdin()),
            tokio::io::stdout(),
        )
        .await?;
    Ok(())
}

fn install(dir: &std::path::Path) -> Result<()> {
    let claude = enfait_config::process::find_claude()
        .context("Claude Code (`claude`) not found in the PATH")?;
    let exe = std::env::current_exe().context("chemin de enfait-cli")?;
    let mut cmd = enfait_config::process::command(claude);
    cmd.args(["mcp", "add", "--scope", "user"]);
    // A session chosen by ENFAIT_SESSION is passed on to the server.
    if std::env::var_os("ENFAIT_SESSION").is_some() {
        cmd.arg("-e")
            .arg(format!("ENFAIT_SESSION={}", dir.display()));
    }
    let status = cmd
        .args(["enfait", "--"])
        .arg(&exe)
        .arg("mcp")
        .status()
        .context("lancement de `claude mcp add`")?;
    anyhow::ensure!(status.success(), "`claude mcp add` failed");
    tracing::info!("\"enfait\" server added to Claude Code; in Claude Code, `/mcp` shows it");
    Ok(())
}
