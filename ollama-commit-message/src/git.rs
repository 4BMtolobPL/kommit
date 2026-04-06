use std::process::Command;
use tracing::info;

pub(crate) fn get_diff(staged: bool) -> anyhow::Result<String> {
    info!(%staged, "Getting diff");
    // TODO: diff 너무 길명 truncate 필요
    // TODO: binary 파일 제외

    let output = if staged {
        Command::new("git").args(["diff", "--staged"]).output()?
    } else {
        Command::new("git").args(["diff"]).output()?
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git diff failed: {}", stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
