# enfait

enfait is a desktop video editor that turns a voice-over or a filmed take
into a finished video with motion design. You write or import the voice,
enfait cleans up the takes, splits the speech into blocks (scenes), and AI
animators build each block as an HTML/CSS/SVG animation that is rendered
frame by frame and edited on top of your voice. Everything runs on your
machine; the models are called through the Anthropic API, your Claude Code
account, or a local OpenAI-compatible model.

Written in Rust with [gpui](https://www.gpui.rs/). Linux, Windows and macOS.

## Contents

- [How it works](#how-it-works)
- [Install](#install)
- [Build from source](#build-from-source)
- [Running the app](#running-the-app)
- [Command-line tool](#command-line-tool)
- [MCP server](#mcp-server)
- [Repository layout](#repository-layout)
- [Configuration and data](#configuration-and-data)
- [Development](#development)

## How it works

A project goes through these steps (only the ones that apply to how the
project starts are shown):

1. **Script**: write the text; a teleprompter (F5) reads it to you.
2. **Voice**: record the voice (microphone, or camera + microphone), or
   import a voice-over or a filmed take.
3. **Cut**: the take is transcribed locally with whisper; false starts,
   stutters and retakes are found and the last complete take of each
   sentence is kept ("last take" rule). Every choice can be changed by hand.
4. **Storyboard**: the edit is split into blocks of 2 to 10 seconds. For
   each block you choose a type and describe what is seen:
   - *Animation*: full-screen motion design;
   - *Mixed*: the filmed person with an animation beside or on top
     (optionally cut out, so the animation passes behind them);
   - *Rush*: the take as is;
   - *Effect*: an effect applied to the take itself (reframing, split
     screen, captions…);
   - *B-roll*: a real shot from your library or from free stock banks.

   Blocks can be split at a word or merged, and precise moments can be
   anchored on a word. A global style (rules, presets, post-FX, fonts and
   files of the library) applies to every block; it can be saved and
   applied to other projects.
5. **Montage**: a director pass sets the art direction, then animators
   create each block in parallel, check it, fix it and (depending on the
   care level) critique it on rendered frames.
6. **Retouch**: leave notes at the playhead, get a new version, or restore
   an earlier one (restoring re-renders without calling a model).
7. **Finish**: music, sound effects, ducking, then export: MP4 (x264,
   loudness normalised to −14 LUFS) or an FCP7 XML edit for Premiere Pro
   and DaVinci Resolve.

*Magic mode* runs the whole chain from a single file: cleanup, written
storyboard, stock shots and sounds, animations, transitions and export.

Formats: landscape 16:9, vertical 9:16 (Shorts, Reels, TikTok) and square.
Care levels: Light (Haiku), Fast (Sonnet), Polished and Ultra (Opus, with
more critique rounds).

## Install

Download the package for your system from the
[latest release](https://github.com/IIyn/enfait/releases/latest):

| System | File |
|---|---|
| Windows | `enfait-<version>-x86_64-windows-setup.exe` (or `.msi`, `.zip`) |
| macOS (Apple silicon) | `enfait-<version>-arm64-macos.dmg` |
| Debian, Ubuntu | `enfait_<version>_amd64.deb` |
| Fedora, openSUSE | `enfait-<version>-1.x86_64.rpm` |
| Arch Linux | `enfait-bin-<version>-1-x86_64.pkg.tar.zst` |
| Any Linux | `enfait-<version>-x86_64.flatpak` or `enfait-<version>-x86_64-linux.tar.gz` |

`SHA256SUMS` lists the checksums. Settings → General → Updates checks for a
newer release, shows its release notes and downloads the right file for
your installation.

### Requirements

- **ffmpeg** and **ffprobe** in the `PATH` (bundled in the Flatpak).
- **Chromium headless shell** and the **whisper model**: downloaded from
  Settings → Tools, or with `enfait-cli setup`.
- **A model provider**, one of:
  - an Anthropic API key (Settings → Models, stored in the system keychain,
    or `ANTHROPIC_API_KEY`);
  - [Claude Code](https://claude.com/claude-code) installed and signed in
    (`claude` in the `PATH`), which uses your Claude account;
  - a local model behind an OpenAI-compatible API (Settings → Models, or
    `ENFAIT_LOCAL_URL`, `ENFAIT_LOCAL_MODEL`, `ENFAIT_LOCAL_KEY`).
- Optional keys for stock banks: Pexels, Pixabay, Freesound (Settings →
  Asset banks). Wikimedia Commons and Openverse need no key.
- Optional NVIDIA GPU: the renderer uses it through Vulkan when
  `nvidia-smi` is present.

## Build from source

The toolchain is pinned in `rust-toolchain.toml` (Rust 1.98); `rustup`
installs it on the first build.

Linux build dependencies (Debian/Ubuntu names):

```sh
sudo apt install build-essential pkg-config cmake clang libclang-dev \
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
  libx11-dev libx11-xcb-dev libxcb1-dev \
  libfontconfig1-dev libfreetype-dev libvulkan-dev \
  libasound2-dev libdbus-1-dev
```

Then:

```sh
git clone https://github.com/IIyn/enfait.git
cd enfait
cargo build --release -p enfait-app -p enfait-cli
```

This produces `target/release/enfait` (the app) and
`target/release/enfait-cli` (the command-line tool). Whisper on the GPU is
behind the `vulkan` feature of `enfait-speech` (needs `vulkan-headers`).

## Running the app

```sh
target/release/enfait                 # home screen: recent projects, new project
target/release/enfait take.mp4        # opens a file in a new project form
```

The first launch walks through the settings (language, theme, models,
tools). Closing the window keeps enfait running in the system tray while
jobs and exports finish (Settings → General to quit instead).

Useful shortcuts:

| Keys | Action |
|---|---|
| Space, J / K / L | play/pause, shuttle backward / pause / forward (×1 to ×8) |
| ← → / Shift+← → | one frame / one second |
| Home / End | start / end |
| Alt+1 … Alt+7 | go to a step |
| Ctrl+Shift+H | home screen |
| Ctrl+Z / Ctrl+Shift+Z, Ctrl+Y | undo / redo |
| Ctrl+B | style panel |
| Ctrl+O, Ctrl+I | open a file, import a take |
| F5 | teleprompter |

On macOS, Cmd replaces Ctrl.

## Command-line tool

`enfait-cli` runs the same engine without the interface.

| Command | What it does |
|---|---|
| `setup` | Downloads Chromium headless shell and the whisper model. |
| `styles` | Lists the style presets. |
| `make <voice>` | Makes a whole video from a voice-over: transcription, blocks, direction, animation, render, encoding. Options: `--script`, `--style`, `--preset`, `--look`, `--quality light\|fast\|polished\|ultra`, `--aspect 16:9\|9:16\|1:1`, `--fps`, `--provider auto\|api\|claude-code\|local`, `--jobs`, `--redo 2,5`, `--limit`, `--transitions`, `--gpu`, `-o`. |
| `cut <take>` | Cuts a take with the "last take" rule, with or without `--script`; `--export edit.mp4` writes the result. |
| `media <file>` | Prepares a media for playback (analysis, proxy, sound, waveform) in the cache. |
| `export` | Exports the current project of the app: `-o video.mp4`, `--premiere edit.xml`, `--no-sfx`, `--lufs -14`, `--no-normalize`, `--session <folder>`. |
| `search <query>` | Searches royalty-free images (`--video`, `--audio`), `--download <dir>` saves them with their credits. |
| `cutout <in> <out>` | Removes the background of an image (transparent PNG); `--fast` for the lighter model. |
| `eval <dir>` | Runs the evaluation set (`eval/cases.json`) and writes a rating sheet. |
| `mcp` | Starts the MCP server (see below). |

Example:

```sh
enfait-cli make voice.wav --script script.md --aspect 9:16 --quality fast
```

`make` and `eval` call models and use credits; the other commands run
locally.

## MCP server

`enfait-cli mcp` is an MCP server (stdio) that lets Claude Code or any MCP
client read and edit the project open in the app. Register it in Claude
Code once:

```sh
enfait-cli mcp --install
# same as: claude mcp add --scope user enfait -- /path/to/enfait-cli mcp
```

Then `/mcp` in Claude Code lists it. Without `ENFAIT_SESSION`, the server
follows whichever project is open in the app; edits made through MCP show
up in the app and can be undone there. Tools marked *app* hand the work to
the running app, which must be open.

| Tool | Purpose |
|---|---|
| `storyboard` | Format and blocks: number, times, type, spoken text, description, moments, transition, render state. |
| `script` | The script written by the user. |
| `edit_block` | Changes a block: type, description, transition (`cut`, `auto` or a description), b-roll file and start, cutout. |
| `add_moment` / `remove_moment` | Anchors (or removes) a precise moment on a word of a block. |
| `split_block` | Splits a block before a word (or at a time of the video). |
| `merge_blocks` | Merges a block with the next one. |
| `format` | 16:9, 9:16 or square; framing `crop` (with `pan`) or `bars`. |
| `library` | Files of the project library (images, SVG, videos, fonts, sounds). |
| `search_media` | Searches royalty-free images, videos or sounds. |
| `add_media` | Downloads a search result into the library, with its credits. |
| `import_file` | Copies a file of the computer into the library. |
| `describe_file` | Says what a library file is for (read by the animators). |
| `style` / `set_style` | Reads or changes the style: preset, rules, post-FX, rules remembered from retouches. |
| `saved_styles` / `save_style` | Lists the styles saved for every project, or saves the current one (with the library files its rules name). |
| `apply_saved_style` / `delete_saved_style` | Applies a saved style to the project (its files are copied into the library), or deletes it. |
| `finish` | The sound finish: music, sound effects, ducking. |
| `add_music` / `add_sfx` / `remove_sound` | Places or removes music and sound effects (an effect can be anchored on a word). |
| `edit_sound` | Changes a placed sound: volume, fades, start, blocks covered, block or word of an effect. |
| `set_ducking` | How much the music dips under the voice, and the voice volume. |
| `credits` | Credits to publish with the video. |
| `generate` | *app*: generates the animations of some blocks, or everything left, with transitions; optional care level (`light`, `fast`, `polished`, `ultra`) and provider. |
| `retouch` | *app*: new version of a block's animation from remarks anchored on a time or a word. |
| `restore_version` | *app*: back to a previous version of a block (no model call). |
| `write_storyboard` | *app*: has the editor write the storyboard (types, descriptions, moments, shots, sounds). |
| `magic_mode` | *app*: runs magic mode on the take. Long and costly in models. |
| `export` | Exports the MP4 and/or the Premiere XML (absolute paths). |

## Repository layout

```
crates/
  enfait-core     time in flicks, project, blocks, moments, look, history (undo/redo by commands)
  enfait-config   settings, API keys (keychain), paths, projects list, executable lookup
  enfait-speech   ffmpeg decoding, whisper, blocks, script alignment, takes, cleanup without script
  enfait-media    probe, proxies, audio, waveform, edit decision list, playback clock, audio output, post-FX, recording, camera
  enfait-motion   composition contract, library (fonts, anime.js), Chromium frame renderer, checks
  enfait-assets   royalty-free search (Commons, Openverse, Pexels, Pixabay, Freesound), downloads, background removal (ONNX via tract)
  enfait-ai       providers (Anthropic API, Claude Code, local), prompts, editor, director, animators, style presets
  enfait-studio   shared workshop for the CLI and the app: storyboard writing, animation, retouches, render, preview, mix, export, Premiere XML
  enfait-mcp      MCP server and file bridge with the app
  enfait-cli      command-line tool
  enfait-app      gpui desktop app (screens, player, jobs, tray, notifications, updates)
assets/           icons, fonts and motion libraries bundled with the app
packaging/        Flatpak manifest, Linux desktop file, macOS bundle, Windows installers
eval/             evaluation cases and their generator
scripts/ci.sh     the checks run by CI
```

An animation is a self-contained HTML page with anime.js that exposes
`window.enfait = { duration, seek(t) }`. The renderer drives
`chrome-headless-shell` frame by frame (`beginFrame`), one Chromium process
per parallel job. Scenes that show the take receive its frames in
`img[data-enfait-scene="from"]`.

## Configuration and data

On Linux (other systems use their usual equivalents):

| Path | Content |
|---|---|
| `~/.config/enfait/settings.json` | settings |
| `~/.local/share/enfait/projects/` | one folder per project: `project.json`, `look.json`, `montage.json`, `finish.json`, `library/`, `studio/` (compositions, renders, logs) |
| `~/.cache/enfait/` | media cache (proxies, sound, waveform, transcripts), Chromium, models |

Environment variables:

| Variable | Effect |
|---|---|
| `ANTHROPIC_API_KEY` | API key, takes precedence over the saved one |
| `ENFAIT_LOCAL_URL`, `ENFAIT_LOCAL_MODEL`, `ENFAIT_LOCAL_KEY` | local model |
| `ENFAIT_SESSION` | project folder used by the CLI and the MCP server instead of the app's current project |
| `ENFAIT_CHROME_DIR`, `ENFAIT_CHROME_ARGS` | another Chromium, extra Chromium options |
| `ENFAIT_OPEN` | opens a page directly: `settings/<section>`, `onboarding/<n>`, `new/custom`, `new/magic`, `step/<step>` (screenshots) |
| `ENFAIT_FULLSCREEN=1` | fullscreen window |
| `RUST_LOG` | log filter (`info` by default, e.g. `RUST_LOG=debug`) |

## Development

```sh
scripts/ci.sh            # what CI runs
cargo clippy --workspace --all-targets
cargo test --workspace   # tests never call a model
cargo deny check
```

Rules of the codebase: no `unsafe`, no `unwrap`/`expect`/`panic`/direct
indexing outside tests (workspace lints); widely used crates only;
dependencies and assets must be redistributable (MIT, Apache, OFL…), which
is why animations use anime.js. Code, comments, prompts, CLI and MCP are in
English; the interface is translated (English and French) with `tr!` and
`crates/enfait-app/locales/*.yml`.

Releases: pushing a `v*` tag builds every package and attaches them to the
GitHub release (`.github/workflows/release.yml`).

## License

enfait is free software, released under the
[GNU Affero General Public License v3.0 or later](LICENSE)
(`AGPL-3.0-or-later`). You may use, modify and redistribute it, but any
modified version you distribute, or let people use over a network (for example
as a hosted service), must be released under the same license with its full
source code.
