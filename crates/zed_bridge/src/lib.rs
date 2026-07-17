//! Opens files (optionally at a line) in Zed via the `zed` CLI.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locates the `zed` CLI once. `None` means fall back to `open -a Zed`.
pub fn zed_cli() -> Option<PathBuf> {
    let candidates = ["zed"];
    for cli in candidates {
        if let Ok(out) = Command::new("which").arg(cli).output() {
            if out.status.success() {
                let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(PathBuf::from(path));
                }
            }
        }
    }
    let fallback = PathBuf::from("/usr/local/bin/zed");
    fallback.exists().then_some(fallback)
}

/// Opens `path` in Zed, at `line` when the CLI is available (the `open -a`
/// fallback cannot target a line). Spawn-and-forget.
pub fn open_in_zed(path: &Path, line: Option<u32>) -> Result<()> {
    if let Some(cli) = zed_cli() {
        let target = match line {
            Some(line) => format!("{}:{}", path.display(), line),
            None => path.display().to_string(),
        };
        Command::new(cli)
            .arg(target)
            .spawn()
            .context("failed to launch zed CLI")?;
    } else {
        Command::new("open")
            .args(["-a", "Zed"])
            .arg(path)
            .spawn()
            .context("failed to open Zed via LaunchServices")?;
    }
    Ok(())
}
