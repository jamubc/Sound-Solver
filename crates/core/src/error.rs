/// Errors returned by the solver core.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The project or a call argument is outside the model's validity range.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// The numerical solution became non-physical (negative density or energy) or failed to
    /// converge. The solver never returns a clamped or patched state in place of a solution.
    #[error("solver failure: {0}")]
    Solver(String),
}

impl Error {
    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }

    pub fn solver(msg: impl Into<String>) -> Self {
        Self::Solver(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
