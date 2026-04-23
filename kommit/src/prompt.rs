use clap::ValueEnum;
use std::fmt::{Display, Formatter};
use tracing::info;

pub(crate) fn build_prompt(diff: &str, lang: ResponseLang) -> String {
    info!(%lang, "Building prompt");
    // TODO: diff 요약 먼저 시키기 (2-step)
    // TODO: 파일별 그룹
    // TODO: 프롬프트 버전 관리

    format!(
        r#"
You are an expert developer.

Write a concise Git commit message based on the diff below.

{0}
- Language: {1}

Diff:
{2}
"#,
        commit_guidelines(),
        lang,
        diff
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

[Output Rules]
- Output ONLY the commit message
"#
}

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum ResponseLang {
    Ko,
    En,
}

impl Display for ResponseLang {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_possible_value().unwrap().get_name())
    }
}
