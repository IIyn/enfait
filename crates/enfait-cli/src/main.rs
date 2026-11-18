//! `enfait-cli` : prototype en ligne de commande de la chaîne complète.

mod assets;
mod cut;
mod encode;
mod eval;
mod export;
mod make;
mod mcp;
mod media;
mod paths;
mod setup;
mod timeline;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "enfait-cli",
    version,
    about = "Voix off → vidéo en motion design"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Télécharge Chromium et le modèle whisper.
    Setup,
    /// Liste les styles prédéfinis.
    Styles,
    /// Produit la vidéo à partir d'une voix off.
    Make(Box<make::MakeArgs>),
    /// Découpe une prise selon le script (règle « dernière prise »).
    Cut(cut::CutArgs),
    /// Prépare un média pour la lecture (analyse, proxy, son, forme d'onde).
    Media(media::MediaArgs),
    /// Génère le jeu d'évaluation et une planche à noter.
    Eval(Box<eval::EvalArgs>),
    /// Exporte la vidéo finale ou le montage Premiere (XML) de la session de
    /// l'interface.
    Export(export::ExportArgs),
    /// Cherche des images ou des vidéos libres de droits.
    Search(assets::SearchArgs),
    /// Détoure une image (PNG transparent).
    Cutout(assets::CutoutArgs),
    /// Serveur MCP sur la session de l'interface (pour Claude Code et les
    /// agents).
    Mcp(mcp::McpArgs),
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        // La sortie standard est réservée aux résultats (et au protocole MCP).
        .with_writer(std::io::stderr)
        .without_time()
        .init();
    let cli = Cli::parse();
    let result = async {
        let dirs = paths::Dirs::new()?;
        match cli.command {
            Command::Setup => setup::run(&dirs).await,
            Command::Styles => {
                for p in enfait_ai::styles::PRESETS {
                    tracing::info!("{:<8} {} : {}", p.slug, p.name, p.summary);
                }
                Ok(())
            }
            Command::Make(args) => make::run(&dirs, *args).await,
            Command::Eval(args) => eval::run(&dirs, *args).await,
            Command::Media(args) => media::run(&dirs, args).await,
            Command::Cut(args) => cut::run(&dirs, args).await,
            Command::Export(args) => export::run(args).await,
            Command::Search(args) => assets::search_cmd(args).await,
            Command::Cutout(args) => assets::cutout_cmd(&dirs, args).await,
            Command::Mcp(args) => mcp::run(args).await,
        }
    }
    .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
