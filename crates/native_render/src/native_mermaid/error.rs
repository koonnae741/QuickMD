//! Error types for the native Mermaid diagram engine.

use std::fmt;
use std::ops::Range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MermaidError {
    Lexer {
        message: String,
        span: Range<usize>,
    },
    Parse {
        message: String,
        span: Range<usize>,
    },
    Layout {
        message: String,
    },
    Render {
        message: String,
    },
}

impl fmt::Display for MermaidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lexer { message, span } => write!(f, "lexer error at {:?}: {}", span, message),
            Self::Parse { message, span } => write!(f, "parse error at {:?}: {}", span, message),
            Self::Layout { message } => write!(f, "layout error: {}", message),
            Self::Render { message } => write!(f, "render error: {}", message),
        }
    }
}

impl std::error::Error for MermaidError {}
