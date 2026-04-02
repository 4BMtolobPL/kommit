use std::process::Command;
use tracing::info;

pub(crate) fn get_diff(staged: bool) -> anyhow::Result<String> {
    info!("Getting diff");
    // TODO: diff 너무 길명 truncate 필요
    // TODO: binary 파일 제외

    let output = if staged {
        Command::new("git").args(["diff", "--cached"]).output()?
    } else {
        Command::new("git").args(["diff"]).output()?
    };

    Ok(String::from_utf8(output.stdout)?)
}
