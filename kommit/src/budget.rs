/// How many bytes of diff or summaries a single LLM request may carry.
///
/// Decided once per run, after the model is known, from the model's context length
/// when it is known and from fixed values otherwise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct InputBudget {
    /// A whole diff, or the joined summaries, sent in one prompt.
    pub prompt_bytes: usize,
    /// One diff chunk sent for summarizing.
    pub chunk_bytes: usize,
}

/// The share of the context a diff or the summaries may take, as a fraction. The rest is
/// left for the instructions, the reply and any thinking, and because models attend to
/// a nearly full context less reliably.
const INPUT_SHARE: (usize, usize) = (1, 2);
/// Bytes per token, a little below the about 3.4 measured on code so that the estimate
/// errs on the side of a smaller prompt.
const BYTES_PER_TOKEN: usize = 3;
/// The least a prompt may carry, however small the context.
const MIN_PROMPT_BYTES: usize = 1024;

impl InputBudget {
    /// The budget for a model loaded with a context of `tokens` tokens.
    pub(crate) fn from_context_length(tokens: u32) -> Self {
        let input_tokens = tokens as usize * INPUT_SHARE.0 / INPUT_SHARE.1;
        Self::from_prompt_bytes((input_tokens * BYTES_PER_TOKEN).max(MIN_PROMPT_BYTES))
    }

    /// A chunk takes a fifth less than a prompt, leaving room for the summary it asks for.
    fn from_prompt_bytes(prompt_bytes: usize) -> Self {
        Self {
            prompt_bytes,
            chunk_bytes: prompt_bytes * 4 / 5,
        }
    }
}

impl Default for InputBudget {
    /// For a model whose context length is unknown: fits a context of about 8k tokens.
    fn default() -> Self {
        Self::from_prompt_bytes(15000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_follows_the_context_length() {
        assert_eq!(
            InputBudget::from_context_length(4096),
            InputBudget {
                prompt_bytes: 6144,
                chunk_bytes: 4915,
            }
        );
        assert_eq!(
            InputBudget::from_context_length(8192),
            InputBudget {
                prompt_bytes: 12288,
                chunk_bytes: 9830,
            }
        );
        assert_eq!(
            InputBudget::from_context_length(32768),
            InputBudget {
                prompt_bytes: 49152,
                chunk_bytes: 39321,
            }
        );
    }

    #[test]
    fn tiny_contexts_still_get_the_minimum() {
        for tokens in [0, 1, 512] {
            let budget = InputBudget::from_context_length(tokens);
            assert_eq!(budget.prompt_bytes, MIN_PROMPT_BYTES, "{tokens} tokens");
            assert!(budget.chunk_bytes > 0, "{tokens} tokens");
        }
    }

    #[test]
    fn default_keeps_the_fixed_limits() {
        assert_eq!(
            InputBudget::default(),
            InputBudget {
                prompt_bytes: 15000,
                chunk_bytes: 12000,
            }
        );
    }
}
