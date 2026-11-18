//! `enfait-cli mcp` : le serveur MCP d'enfait sur l'entrée et la sortie
//! standard, pour Claude Code ou tout client MCP. `--installer` l'enregistre
//! dans Claude Code (`claude mcp add`).

use anyhow::{Context as _, Result};
use enfait_mcp::bridge::session_dir;
use enfait_mcp::{Server, Tools};

#[derive(Debug, clap::Args)]
pub struct McpArgs {
    /// Enregistre le serveur dans Claude Code (pour tous tes projets), puis
    /// s'arrête.
    #[arg(long)]
    pub installer: bool,
}

pub async fn run(args: McpArgs) -> Result<()> {
    let dir = session_dir();
    if args.installer {
        return install(&dir);
    }
    tracing::info!("serveur MCP sur la session {}", dir.display());
    let mut server = Server::new(Tools::new(dir));
    server
        .serve(
            tokio::io::BufReader::new(tokio::io::stdin()),
            tokio::io::stdout(),
        )
        .await?;
    Ok(())
}

fn install(dir: &std::path::Path) -> Result<()> {
    let claude = enfait_config::process::which("claude")
        .context("Claude Code (`claude`) introuvable dans le PATH")?;
    let exe = std::env::current_exe().context("chemin de enfait-cli")?;
    let mut cmd = enfait_config::process::command(claude);
    cmd.args(["mcp", "add", "--scope", "user"]);
    // Une session choisie par ENFAIT_SESSION est transmise au serveur.
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
    anyhow::ensure!(status.success(), "`claude mcp add` a échoué");
    tracing::info!("serveur « enfait » ajouté à Claude Code ; dans Claude Code, `/mcp` le montre");
    Ok(())
}
