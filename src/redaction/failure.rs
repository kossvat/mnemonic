//! What a failed generation may leave behind: one of a few fixed codes.
//!
//! The text of a backend, parser or store error is fresh output like any
//! other: a model echoes what it was given, a parser quotes the line it
//! stopped at, a client names the address it called. None of it is
//! written to a retry record, a log line or a reply on a protected path.

/// Why a generation step failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The model backend did not answer.
    Backend,
    /// The answer is not the structure that was asked for.
    InvalidJson,
    /// The answer names something the redaction policy refuses.
    Rejected,
    /// The store did not take the result.
    Storage,
    /// The step itself stopped (a panic in a worker).
    Worker,
    /// Anything else, and whatever a caller passed that is no code.
    Other,
}

const ALL: [Failure; 6] = [
    Failure::Backend,
    Failure::InvalidJson,
    Failure::Rejected,
    Failure::Storage,
    Failure::Worker,
    Failure::Other,
];

impl Failure {
    pub const fn code(self) -> &'static str {
        match self {
            Failure::Backend => "BACKEND_FAILED",
            Failure::InvalidJson => "GENERATED_JSON_INVALID",
            Failure::Rejected => "STRUCTURAL_INPUT_REJECTED",
            Failure::Storage => "STORAGE_FAILED",
            Failure::Worker => "WORKER_FAILED",
            Failure::Other => "GENERATION_FAILED",
        }
    }

    /// The failure a text stands for: the one whose code it is, and
    /// [`Failure::Other`] for every other text. What a retry record
    /// stores goes through here, so that no caller can store a message.
    pub fn of(text: &str) -> Self {
        ALL.into_iter()
            .find(|failure| failure.code() == text)
            .unwrap_or(Failure::Other)
    }

    /// An error that says the code and nothing else: it has no source, so
    /// no way of printing it shows the error it stands for.
    pub fn error(self) -> anyhow::Error {
        anyhow::Error::new(self)
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for Failure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_generated_failure_codes_are_fixed_and_distinct() {
        let codes: std::collections::BTreeSet<&str> = ALL.iter().map(|f| f.code()).collect();
        assert!(codes.len() == ALL.len());
        for failure in ALL {
            assert!(Failure::of(failure.code()) == failure);
            assert!(super::super::is_clean(failure.code()));
        }
    }

    #[test]
    fn redaction_generated_failure_of_a_message_is_the_generic_code() {
        let secret: String = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
        for text in [
            format!("backend: connection refused for {secret}"),
            "BACKEND_FAILED: and more".into(),
            " BACKEND_FAILED".into(),
            "backend_failed".into(),
            String::new(),
        ] {
            assert!(Failure::of(&text) == Failure::Other);
        }
        let error = Failure::Backend.error();
        let said = format!("{error} {error:#} {error:?}");
        assert!(said.contains("BACKEND_FAILED") && error.source().is_none());
    }
}
