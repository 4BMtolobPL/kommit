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

/// Asks for a summary of `diff_chunk` of about `max_chars` characters at most, so that
/// the summaries of every chunk fit in the prompt that writes the commit message.
pub(crate) fn build_summary_prompt(
    diff_chunk: &str,
    lang: ResponseLang,
    max_chars: usize,
) -> String {
    format!(
        r#"
You are an expert developer.
Analyze the following git diff chunk and write a concise, bullet-pointed summary of the changes.
Focus on WHAT changed and WHY.

Rules:
- Name the changed file in the summary
- Write each bullet point on a single line
- Do not restate the code: no function bodies, expressions, or lists of values
- No headings, bold text, or nested bullet points such as "Why:"
- Do not repeat yourself
- Keep the whole summary under about {2} characters

- Language: {0}

Diff Chunk:
{1}

Summary (Output only the bullet points in the requested language):
"#,
        lang, diff_chunk, max_chars
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

/// Conventional Commits types shared by the prompt and the validator.
pub(crate) const COMMIT_TYPES: &[&str] = &[
    "feat", "fix", "refactor", "docs", "test", "chore", "style", "ci", "perf", "build",
];

fn commit_guidelines() -> String {
    format!(
        r#"
Follow these rules strictly:

[Format]
- Use Conventional Commits format:
  <type>[(<scope>)][!]: <short summary>
- Types: {0}
- Scope is optional; use `!` only for breaking changes

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
"#,
        COMMIT_TYPES.join(", ")
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_prompt_states_the_length_allowance() {
        let prompt = build_summary_prompt("diff --git a/x b/x", ResponseLang::En, 150);
        assert!(prompt.contains("under about 150 characters"), "{prompt}");
        assert!(prompt.contains("git diff chunk"), "{prompt}");
    }

    #[test]
    fn test_commit_guidelines_lists_all_commit_types() {
        let guidelines = commit_guidelines();
        let types_line = guidelines
            .lines()
            .find(|l| l.starts_with("- Types:"))
            .expect("guidelines should contain a Types line");
        for ty in COMMIT_TYPES {
            assert!(
                types_line.split([' ', ',']).any(|t| t == *ty),
                "type `{ty}` missing from prompt: {types_line}"
            );
        }
    }
}
