//! Token definitions and lexer for Mermaid diagrams built on `logos`.

use logos::{Lexer, Logos};
use std::ops::Range;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"([ \t\r\f]+|%%[^\n]*)")]
pub enum Token<'a> {
    #[token("\n")]
    #[token(";")]
    Separator,

    #[token("graph", ignore(ascii_case))]
    #[token("flowchart", ignore(ascii_case))]
    Graph,

    #[token("subgraph", ignore(ascii_case))]
    Subgraph,

    #[token("end", ignore(ascii_case))]
    End,

    #[token("TB", ignore(ascii_case))]
    #[token("TD", ignore(ascii_case))]
    DirTB,

    #[token("BT", ignore(ascii_case))]
    DirBT,

    #[token("LR", ignore(ascii_case))]
    DirLR,

    #[token("RL", ignore(ascii_case))]
    DirRL,

    #[token("&")]
    Ampersand,

    #[token("|")]
    Pipe,

    // Edges (ordered longer matches first)
    #[token("-->")]
    ArrowSolid,
    #[token("---")]
    LineSolid,
    #[token("-.->")]
    ArrowDotted,
    #[token("-.-")]
    LineDotted,
    #[token("==>")]
    ArrowThick,
    #[token("===")]
    LineThick,

    #[token("--")]
    EdgeTextOpenSolid,
    #[token("-.")]
    EdgeTextOpenDotted,
    #[token("==")]
    EdgeTextOpenThick,

    // Compound shape delimiters (longer multi-char tokens first)
    #[token("([")]
    OpenStadium,
    #[token("])")]
    CloseStadium,

    #[token("[[")]
    OpenSubroutine,
    #[token("]]")]
    CloseSubroutine,

    #[token("[(")]
    OpenCylinder,
    #[token(")]")]
    CloseCylinder,

    #[token("((")]
    OpenCircle,
    #[token("))")]
    CloseCircle,

    #[token("{{")]
    OpenHexagon,
    #[token("}}")]
    CloseHexagon,

    // Single-char shape delimiters
    #[token("[")]
    OpenBracket,
    #[token("]")]
    CloseBracket,

    #[token("(")]
    OpenParen,
    #[token(")")]
    CloseParen,

    #[token("{")]
    OpenBrace,
    #[token("}")]
    CloseBrace,

    #[regex(r#""([^"\\]|\\.)*""#, strip_quotes)]
    QuotedString(&'a str),

    #[regex(r"[a-zA-Z0-9_\u0080-\uFFFF][a-zA-Z0-9_\-\.\u0080-\uFFFF]*")]
    Ident(&'a str),
}

fn strip_quotes<'s>(lex: &mut Lexer<'s, Token<'s>>) -> Option<&'s str> {
    let s = lex.slice();
    if s.len() >= 2 {
        Some(&s[1..s.len() - 1])
    } else {
        Some(s)
    }
}

pub type Spanned<'a> = (Token<'a>, Range<usize>);

/// Tokenizes a Mermaid diagram source string into spanned tokens.
pub fn tokenize(source: &str) -> Vec<Spanned<'_>> {
    let mut lexer = Token::lexer(source);
    let mut tokens = Vec::new();
    while let Some(res) = lexer.next() {
        let span = lexer.span();
        if let Ok(token) = res {
            tokens.push((token, span));
        }
    }
    tokens
}
