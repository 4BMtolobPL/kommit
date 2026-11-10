/// How many bytes of diff or summaries a single LLM request may carry.
///
/// Decided once per run, after the model is known, so that it can later follow the
/// model's context length instead of fixed values.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InputBudget {
    /// A whole diff, or the joined summaries, sent in one prompt.
    pub prompt_bytes: usize,
    /// One diff chunk sent for summarizing.
    pub chunk_bytes: usize,
}

impl Default for InputBudget {
    fn default() -> Self {
        Self {
            prompt_bytes: 15000,
            chunk_bytes: 12000,
        }
    }
}
