use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use tracing::info;

pub(crate) fn build_prompt(diff: &str, branch: Option<&str>, lang: ResponseLang) -> String {
    info!(%lang, ?branch, "Building prompt");

    let branch_str = branch
        .map(|b| format!("- Current Branch: {} (Use this branch name to infer the task context, work category, or ticket details)\n", b))
        .unwrap_or_default();

    format!(
        r#"
You are an expert developer.

Write a concise Git commit message based on the diff below.

{0}
- Language: {1}
{2}
Diff:
{3}
"#,
        commit_guidelines(),
        lang,
        branch_str,
        diff
    )
}

pub(crate) fn build_summary_prompt(diff_chunk: &str, lang: ResponseLang) -> String {
    format!(
        r#"
You are an expert developer.
Analyze the following git diff chunk and write a concise, bullet-pointed summary of the changes.
Focus on WHAT changed and WHY.

- Language: {0}

Diff Chunk:
{1}

Summary (Output only the bullet points in the requested language):
"#,
        lang, diff_chunk
    )
}

pub(crate) fn build_commit_from_summaries_prompt(
    summaries: &str,
    branch: Option<&str>,
    lang: ResponseLang,
) -> String {
    let branch_str = branch
        .map(|b| format!("- Current Branch: {} (Use this branch name to infer the task context, work category, or ticket details)\n", b))
        .unwrap_or_default();

    format!(
        r#"
You are an expert developer.

Write a concise Git commit message based on the summarized changes below.

{0}
- Language: {1}
{2}
Summarized Changes:
{3}
"#,
        commit_guidelines(),
        lang,
        branch_str,
        summaries
    )
}

fn commit_guidelines() -> &'static str {
    r#"
Follow these rules strictly:

[Format]
- Use Conventional Commits format:
  <type>: <short summary>
- Types: feat, fix, refactor, docs, test, chore

[Structure]
- Title must be <= 50 characters
- Optionally include a body separated by a blank line
- Body lines should be <= 72 characters

[Style]
- Use imperative mood (e.g., "Add", "Fix", not "Added")
- Be concise but descriptive

[Content]
- Clearly explain what the change does
- Avoid vague messages

[Focus]
- Explain WHY the change was made when relevant
- Refer to the provided Current Branch name to help determine the commit type (e.g., feat, fix, chore) and context.

[Output Rules]
- Output ONLY the commit message
"#
}

#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ResponseLang {
    Ko,
    En,
}

impl Display for ResponseLang {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_possible_value().unwrap().get_name())
    }
}
