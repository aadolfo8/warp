//! Which CLI agents can be started from a terminal on this machine.

use std::ffi::OsStr;

use enum_iterator::all;

use crate::terminal::CLIAgent;

/// An agent found on `PATH`, with the command that starts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstalledAgent {
    pub agent: CLIAgent,
    pub command: &'static str,
}

/// The agents whose command is on `PATH`, in [`CLIAgent`] order. Touches the file system, so call
/// it off the main thread.
pub fn installed_agents() -> Vec<InstalledAgent> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let extensions = executable_extensions();
    all::<CLIAgent>()
        .filter(|agent| !matches!(agent, CLIAgent::WarpTui | CLIAgent::Unknown))
        .filter_map(|agent| {
            let command = agent
                .command_prefixes()
                .iter()
                .find(|command| is_on_path(command, &path, &extensions))?;
            Some(InstalledAgent { agent, command })
        })
        .collect()
}

/// Suffixes a command can have on disk: the `PATHEXT` extensions on Windows, none elsewhere.
fn executable_extensions() -> Vec<String> {
    if !cfg!(windows) {
        return vec![String::new()];
    }
    std::env::var("PATHEXT")
        .ok()
        .map(|pathext| {
            pathext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(str::to_lowercase)
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| [".com", ".exe", ".bat", ".cmd"].map(String::from).to_vec())
}

/// Whether a file named `command` plus one of `extensions` is in a folder of `path`.
pub(crate) fn is_on_path(command: &str, path: &OsStr, extensions: &[String]) -> bool {
    std::env::split_paths(path).any(|dir| {
        extensions
            .iter()
            .any(|ext| dir.join(format!("{command}{ext}")).is_file())
    })
}

#[cfg(test)]
#[path = "installed_tests.rs"]
mod tests;
