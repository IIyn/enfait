//! `enfait-cli`: command-line version of the whole pipeline.

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
    about = "Voice-over → motion design video"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Downloads Chromium and the whisper model.
    Setup,
    /// Lists the style presets.
    Styles,
    /// Makes the video from a voice-over.
    Make(Box<make::MakeArgs>),
    /// Cuts a take following the script ("last take" rule).
    Cut(cut::CutArgs),
    /// Prepares a media for playback (analysis, proxy, sound, waveform).
    Media(media::MediaArgs),
    /// Generates the evaluation set and a rating sheet.
    Eval(Box<eval::EvalArgs>),
    /// Exports the final video or the Premiere edit (XML) of the app's
    /// session.
    Export(export::ExportArgs),
    /// Searches royalty-free images or videos.
    Search(assets::SearchArgs),
    /// Cuts out an image (transparent PNG).
    Cutout(assets::CutoutArgs),
    /// MCP server on the app's session (for Claude Code and other agents).
    Mcp(mcp::McpArgs),
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        // Standard output is reserved for the results (and the MCP protocol).
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
