use std::fmt;
use std::ops::Range;

/// Errors produced while lexing, parsing, or rasterizing a formula.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MathError {
    /// A character sequence that is not valid TeX math input.
    Lex { span: Range<usize>, fragment: String },
    /// Structurally invalid input (unbalanced groups, unknown commands, ...).
    Parse { span: Range<usize>, message: String },
    /// Rasterization failure (invalid scale, oversized surface, ...).
    Render { message: String },
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lex { span, fragment } => {
                write!(f, "invalid input {fragment:?} at {}..{}", span.start, span.end)
            }
            Self::Parse { span, message } => {
                write!(f, "parse error at {}..{}: {message}", span.start, span.end)
            }
            Self::Render { message } => write!(f, "render error: {message}"),
        }
    }
}

impl std::error::Error for MathError {}
