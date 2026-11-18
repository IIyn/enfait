//! Executables: lookup in `PATH` and spawning without a console window.

use std::path::PathBuf;

/// Windows: `CREATE_NO_WINDOW`, so that ffmpeg & co. do not flash a
/// console when started from the graphical app.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Looks for an executable in `PATH`. On Windows the extensions of
/// `PATHEXT` are tried too (`claude` finds `claude.cmd`).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions = extensions();
    std::env::split_paths(&path).find_map(|dir| {
        extensions
            .iter()
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(windows)]
fn extensions() -> Vec<String> {
    let mut list = vec![String::new()];
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
    list.extend(
        pathext
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_ascii_lowercase),
    );
    list
}

#[cfg(not(windows))]
fn extensions() -> Vec<String> {
    vec![String::new()]
}

/// `std::process::Command` that opens no console window on Windows.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// `tokio::process::Command` that opens no console window on Windows.
pub fn tokio_command(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nothing_for_an_unknown_program() {
        assert!(which("enfait-surely-not-a-real-program").is_none());
    }
}
