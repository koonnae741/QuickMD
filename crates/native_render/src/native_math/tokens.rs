//! Deterministic LaTeX-math lexer built on `logos`.

use logos::{Lexer, Logos};
use std::ops::Range;

#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
#[logos(skip r"([ \t\r\n\f]+|%[^\n]*)")]
pub enum Token<'a> {
    /// `\name` — control word (stored without the backslash).
    #[regex(r"\\[a-zA-Z]+", |lex| &lex.slice()[1..])]
    Command(&'a str),

    /// `\x` — control symbol such as `\{`, `\,`, `\\`, `\|`.
    #[regex(r"\\[^a-zA-Z]", |lex| lex.slice()[1..].chars().next())]
    Escaped(char),

    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("^")]
    Caret,
    #[token("_")]
    Underscore,
    #[token("&")]
    Ampersand,
    #[token("'")]
    Prime,
    #[token("~")]
    Tilde,

    #[regex(r"[0-9]", first_char)]
    Digit(char),

    #[regex(r"[a-zA-Z]", first_char)]
    Letter(char),

    /// Operators, relations, punctuation and fence characters.
    #[regex(r"[+\-*/=<>!?,;:.()\[\]|]", first_char)]
    Symbol(char),

    /// Any non-ASCII scalar value (Greek, math symbols, complex scripts).
    #[regex(r"[^\x00-\x7F]", first_char)]
    Unicode(char),
}

fn first_char<'s>(lex: &mut Lexer<'s, Token<'s>>) -> Option<char> {
    lex.slice().chars().next()
}

/// A token together with its byte span. `token == None` marks invalid input,
/// which is only reported if the parser actually reaches it (raw `\text{}`
/// groups may legally contain such characters).
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<'a> {
    pub token: Option<Token<'a>>,
    pub span: Range<usize>,
}

pub fn tokenize(source: &str) -> Vec<Spanned<'_>> {
    let mut lexer = Token::lexer(source);
    let mut out = Vec::with_capacity(source.len() / 2 + 1);
    while let Some(result) = lexer.next() {
        out.push(Spanned { token: result.ok(), span: lexer.span() });
    }
    out
}
