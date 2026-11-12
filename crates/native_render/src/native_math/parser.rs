//! Recursive-descent parser: token stream → `MathList`.

use std::ops::Range;

use super::ast::{
    AtomClass, Delimiter, FontVariant, LimitsMode, MathList, MathNode, MathStyle, MatrixKind,
};
use super::error::MathError;
use super::symbols;
use super::tokens::{tokenize, Spanned, Token};

/// Bounds recursion depth (parser *and* layout) against adversarial input.
const MAX_NESTING: usize = 192;

#[derive(Debug, Clone, Copy, Default)]
struct Stops {
    brace: bool,
    bracket: bool,
    right: bool,
    env: bool,
}

pub fn parse(source: &str) -> Result<MathList, MathError> {
    let mut parser = Parser { source, tokens: tokenize(source), pos: 0, depth: 0 };
    let list = parser.parse_list(Stops::default())?;
    match parser.peek()? {
        None => Ok(list),
        Some(_) => Err(parser.error_here("unexpected trailing input")),
    }
}

fn style_switch(name: &str) -> Option<MathStyle> {
    match name {
        "displaystyle" => Some(MathStyle::Display),
        "textstyle" => Some(MathStyle::Text),
        "scriptstyle" => Some(MathStyle::Script),
        "scriptscriptstyle" => Some(MathStyle::ScriptScript),
        _ => None,
    }
}

fn prime_symbol() -> MathNode {
    MathNode::Symbol { text: "\u{2032}".to_string(), class: AtomClass::Ord, variant: FontVariant::Roman }
}

fn roman(text: &str, class: AtomClass) -> MathNode {
    MathNode::Symbol { text: text.to_string(), class, variant: FontVariant::Roman }
}

/// Converts raw `\text{...}` source into display text.
fn unescape_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    let mut last_space = false;
    while let Some(c) = chars.next() {
        let produced = match c {
            '\\' => match chars.next() {
                Some(n) if "{}%&$#_".contains(n) => n,
                Some(' ') | None => ' ',
                Some(n) => {
                    out.push('\\');
                    n
                }
            },
            '~' => '\u{00A0}',
            c if c.is_whitespace() => ' ',
            c => c,
        };
        if produced == ' ' {
            if last_space {
                continue;
            }
            last_space = true;
        } else {
            last_space = false;
        }
        out.push(produced);
    }
    out
}

fn remap_alphabet(alphabet: &str, node: MathNode) -> MathNode {
    match node {
        MathNode::Identifier(c) => MathNode::Symbol {
            text: symbols::map_alphabet(alphabet, c).to_string(),
            class: AtomClass::Ord,
            variant: FontVariant::Roman,
        },
        MathNode::Number(digits) => MathNode::Symbol {
            text: digits.chars().map(|c| symbols::map_alphabet(alphabet, c)).collect(),
            class: AtomClass::Ord,
            variant: FontVariant::Roman,
        },
        MathNode::Group(list) => {
            MathNode::Group(list.into_iter().map(|n| remap_alphabet(alphabet, n)).collect())
        }
        MathNode::Scripts { base, sub, sup } => MathNode::Scripts {
            base: Box::new(remap_alphabet(alphabet, *base)),
            sub,
            sup,
        },
        other => other,
    }
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Spanned<'a>>,
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    // ---------- token helpers ----------

    fn peek(&self) -> Result<Option<Token<'a>>, MathError> {
        match self.tokens.get(self.pos) {
            None => Ok(None),
            Some(spanned) => match spanned.token {
                Some(token) => Ok(Some(token)),
                None => Err(MathError::Lex {
                    span: spanned.span.clone(),
                    fragment: self.source[spanned.span.clone()].to_string(),
                }),
            },
        }
    }

    fn bump(&mut self) -> Result<Option<Token<'a>>, MathError> {
        let token = self.peek()?;
        if token.is_some() {
            self.pos += 1;
        }
        Ok(token)
    }

    fn raw_token(&self, index: usize) -> Option<Token<'a>> {
        self.tokens.get(index).and_then(|s| s.token)
    }

    fn contiguous(&self, index: usize) -> bool {
        index > 0
            && index < self.tokens.len()
            && self.tokens[index - 1].span.end == self.tokens[index].span.start
    }

    fn current_span(&self) -> Range<usize> {
        self.tokens
            .get(self.pos)
            .map(|t| t.span.clone())
            .unwrap_or(self.source.len()..self.source.len())
    }

    fn previous_span(&self) -> Range<usize> {
        self.pos
            .checked_sub(1)
            .and_then(|i| self.tokens.get(i))
            .map(|t| t.span.clone())
            .unwrap_or(0..0)
    }

    fn error_at(&self, span: Range<usize>, message: impl Into<String>) -> MathError {
        MathError::Parse { span, message: message.into() }
    }

    fn error_here(&self, message: impl Into<String>) -> MathError {
        self.error_at(self.current_span(), message)
    }

    fn enter(&mut self) -> Result<(), MathError> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            Err(self.error_here("formula nesting is too deep"))
        } else {
            Ok(())
        }
    }

    fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    fn expect(&mut self, expected: Token<'a>, what: &str) -> Result<(), MathError> {
        match self.bump()? {
            Some(token) if token == expected => Ok(()),
            _ => Err(self.error_at(self.previous_span(), format!("expected {what}"))),
        }
    }

    // ---------- lists & atoms ----------

    fn parse_list(&mut self, stops: Stops) -> Result<MathList, MathError> {
        self.enter()?;
        let mut list = Vec::new();
        while let Some(token) = self.peek()? {
            match token {
                Token::RBrace if stops.brace => break,
                Token::RBrace => return Err(self.error_here("unbalanced '}'")),
                Token::Symbol(']') if stops.bracket => break,
                Token::Ampersand | Token::Escaped('\\') if stops.env => break,
                Token::Ampersand => {
                    return Err(self.error_here("'&' is only valid inside a matrix environment"));
                }
                Token::Escaped('\\') => {
                    return Err(self.error_here("'\\\\' is only valid inside a matrix environment"));
                }
                Token::Command("right") if stops.right => break,
                Token::Command("right") => {
                    return Err(self.error_here("\\right without matching \\left"));
                }
                Token::Command("end") if stops.env => break,
                Token::Command("end") => {
                    return Err(self.error_here("\\end without matching \\begin"));
                }
                Token::Command(name) if style_switch(name).is_some() => {
                    self.pos += 1;
                    let style = style_switch(name).unwrap_or(MathStyle::Text);
                    let body = self.parse_list(stops)?;
                    list.push(MathNode::StyleChange { style, body });
                    break;
                }
                _ => list.push(self.parse_atom()?),
            }
        }
        self.leave();
        Ok(list)
    }

    fn parse_atom(&mut self) -> Result<MathNode, MathError> {
        let mut base = self.parse_primary()?;
        let mut sub: Option<MathList> = None;
        let mut sup: Option<MathList> = None;
        let mut primes = 0usize;

        loop {
            match self.peek()? {
                Some(Token::Caret) => {
                    self.pos += 1;
                    if sup.is_some() {
                        return Err(self.error_at(self.previous_span(), "double superscript"));
                    }
                    sup = Some(self.parse_arg()?);
                }
                Some(Token::Underscore) => {
                    self.pos += 1;
                    if sub.is_some() {
                        return Err(self.error_at(self.previous_span(), "double subscript"));
                    }
                    sub = Some(self.parse_arg()?);
                }
                Some(Token::Prime) => {
                    self.pos += 1;
                    if sup.is_some() {
                        return Err(self.error_at(self.previous_span(), "prime after superscript"));
                    }
                    primes += 1;
                }
                Some(Token::Command(c @ ("limits" | "nolimits" | "displaylimits"))) => {
                    self.pos += 1;
                    let mode = match c {
                        "limits" => LimitsMode::Limits,
                        "nolimits" => LimitsMode::NoLimits,
                        _ => LimitsMode::Auto,
                    };
                    match &mut base {
                        MathNode::BigOperator { limits, .. } | MathNode::Function { limits, .. } => {
                            *limits = mode;
                        }
                        _ => {
                            return Err(self.error_at(
                                self.previous_span(),
                                "\\limits is only allowed after an operator",
                            ));
                        }
                    }
                }
                _ => break,
            }
        }

        if primes > 0 {
            let mut list: MathList = (0..primes).map(|_| prime_symbol()).collect();
            if let Some(rest) = sup.take() {
                list.extend(rest);
            }
            sup = Some(list);
        }

        if sub.is_none() && sup.is_none() {
            Ok(base)
        } else {
            Ok(MathNode::Scripts { base: Box::new(base), sub, sup })
        }
    }

    /// TeX argument: a braced group, or exactly one token/command.
    fn parse_arg(&mut self) -> Result<MathList, MathError> {
        match self.peek()? {
            None => Err(self.error_here("missing argument")),
            Some(Token::LBrace) => self.parse_group_contents(),
            Some(Token::Digit(d)) => {
                self.pos += 1;
                Ok(vec![MathNode::Number(d.to_string())])
            }
            Some(Token::Caret | Token::Underscore | Token::RBrace | Token::Ampersand) => {
                Err(self.error_here("missing argument"))
            }
            Some(_) => Ok(vec![self.parse_primary()?]),
        }
    }

    fn parse_group_contents(&mut self) -> Result<MathList, MathError> {
        self.expect(Token::LBrace, "'{'")?;
        let list = self.parse_list(Stops { brace: true, ..Stops::default() })?;
        self.expect(Token::RBrace, "'}'")?;
        Ok(list)
    }

    /// Returns the verbatim source between a balanced `{ ... }` pair.
    fn raw_group(&mut self) -> Result<String, MathError> {
        let open_span = self.current_span();
        if !matches!(self.raw_token(self.pos), Some(Token::LBrace)) {
            return Err(self.error_here("expected '{'"));
        }
        let mut depth = 0usize;
        let mut index = self.pos;
        while index < self.tokens.len() {
            match self.tokens[index].token {
                Some(Token::LBrace) => depth += 1,
                Some(Token::RBrace) => {
                    depth -= 1;
                    if depth == 0 {
                        let start = open_span.end;
                        let end = self.tokens[index].span.start;
                        self.pos = index + 1;
                        return Ok(self.source[start..end].to_string());
                    }
                }
                _ => {}
            }
            index += 1;
        }
        Err(self.error_at(open_span, "unterminated group"))
    }

    fn parse_primary(&mut self) -> Result<MathNode, MathError> {
        self.enter()?;
        let node = self.parse_primary_inner();
        self.leave();
        node
    }

    fn parse_primary_inner(&mut self) -> Result<MathNode, MathError> {
        let Some(token) = self.peek()? else {
            return Err(self.error_here("unexpected end of input"));
        };
        match token {
            Token::Caret | Token::Underscore => Ok(MathNode::Group(Vec::new())),
            Token::Digit(_) => Ok(self.parse_number()),
            Token::Letter(c) => {
                self.pos += 1;
                Ok(MathNode::Identifier(c))
            }
            Token::Symbol(c) => {
                self.pos += 1;
                Ok(symbols::char_symbol(c))
            }
            Token::Unicode(c) => {
                self.pos += 1;
                Ok(symbols::unicode_symbol(c))
            }
            Token::Prime => {
                self.pos += 1;
                Ok(prime_symbol())
            }
            Token::Tilde => {
                self.pos += 1;
                Ok(MathNode::Space(6.0))
            }
            Token::LBrace => Ok(MathNode::Group(self.parse_group_contents()?)),
            Token::Escaped(c) => {
                let span = self.current_span();
                self.pos += 1;
                self.parse_escaped(c, span)
            }
            Token::Command(name) => {
                let span = self.current_span();
                self.pos += 1;
                self.parse_command(name, span)
            }
            Token::RBrace => Err(self.error_here("unexpected '}'")),
            Token::Ampersand => Err(self.error_here("unexpected '&'")),
        }
    }

    fn parse_number(&mut self) -> MathNode {
        let mut text = String::new();
        loop {
            match self.raw_token(self.pos) {
                Some(Token::Digit(d)) => {
                    text.push(d);
                    self.pos += 1;
                }
                Some(Token::Symbol('.'))
                    if !text.is_empty()
                        && matches!(self.raw_token(self.pos + 1), Some(Token::Digit(_)))
                        && self.contiguous(self.pos + 1) =>
                {
                    text.push('.');
                    self.pos += 1;
                }
                _ => break,
            }
            if !self.contiguous(self.pos) {
                break;
            }
        }
        MathNode::Number(text)
    }

    fn parse_escaped(&mut self, c: char, span: Range<usize>) -> Result<MathNode, MathError> {
        match c {
            '{' => Ok(roman("{", AtomClass::Open)),
            '}' => Ok(roman("}", AtomClass::Close)),
            '|' => Ok(roman("‖", AtomClass::Ord)),
            ',' => Ok(MathNode::Space(3.0)),
            ':' | '>' => Ok(MathNode::Space(4.0)),
            ';' => Ok(MathNode::Space(5.0)),
            '!' => Ok(MathNode::Space(-3.0)),
            ' ' | '\t' | '\n' | '\r' => Ok(MathNode::Space(6.0)),
            '%' | '#' | '&' | '_' => Ok(roman(&c.to_string(), AtomClass::Ord)),
            '\\' => Err(self.error_at(span, "line break is only valid inside an environment")),
            other => Err(self.error_at(span, format!("unknown control symbol \\{other}"))),
        }
    }

    fn parse_command(&mut self, name: &'a str, span: Range<usize>) -> Result<MathNode, MathError> {
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let numerator = self.parse_arg()?;
                let denominator = self.parse_arg()?;
                let style = match name {
                    "dfrac" | "cfrac" => Some(MathStyle::Display),
                    "tfrac" => Some(MathStyle::Text),
                    _ => None,
                };
                Ok(MathNode::Fraction { numerator, denominator, has_bar: true, delimiters: None, style })
            }
            "binom" | "dbinom" | "tbinom" => {
                let numerator = self.parse_arg()?;
                let denominator = self.parse_arg()?;
                let style = match name {
                    "dbinom" => Some(MathStyle::Display),
                    "tbinom" => Some(MathStyle::Text),
                    _ => None,
                };
                Ok(MathNode::Fraction {
                    numerator,
                    denominator,
                    has_bar: false,
                    delimiters: Some((Delimiter::LeftParen, Delimiter::RightParen)),
                    style,
                })
            }
            "sqrt" => {
                let index = if matches!(self.peek()?, Some(Token::Symbol('['))) {
                    self.pos += 1;
                    let idx = self.parse_list(Stops { bracket: true, ..Stops::default() })?;
                    self.expect(Token::Symbol(']'), "']'")?;
                    Some(idx)
                } else {
                    None
                };
                let radicand = self.parse_arg()?;
                Ok(MathNode::Radical { index, radicand })
            }
            "left" => {
                let left = self.parse_delimiter()?;
                let body = self.parse_list(Stops { right: true, ..Stops::default() })?;
                match self.bump()? {
                    Some(Token::Command("right")) => {}
                    _ => return Err(self.error_at(span, "\\left without matching \\right")),
                }
                let right = self.parse_delimiter()?;
                Ok(MathNode::Delimited { left, body, right })
            }
            "right" => Err(self.error_at(span, "\\right without matching \\left")),
            "begin" => self.parse_environment(span),
            "end" => Err(self.error_at(span, "\\end without matching \\begin")),
            "mathrm" | "mathup" | "mathsf" | "mathtt" => self.parse_styled(FontVariant::Roman),
            "mathit" | "mathnormal" => self.parse_styled(FontVariant::Italic),
            "mathbf" => self.parse_styled(FontVariant::Bold),
            "boldsymbol" | "bm" => self.parse_styled(FontVariant::BoldItalic),
            "mathbb" | "mathcal" | "mathscr" | "mathfrak" => self.parse_alphabet(name),
            "text" | "textrm" | "textnormal" | "mbox" | "textup" => self.parse_text(FontVariant::Roman),
            "textit" | "emph" => self.parse_text(FontVariant::Italic),
            "textbf" => self.parse_text(FontVariant::Bold),
            "operatorname" => {
                let limits = if matches!(self.peek()?, Some(Token::Symbol('*'))) {
                    self.pos += 1;
                    LimitsMode::Auto
                } else {
                    LimitsMode::NoLimits
                };
                let raw = self.raw_group()?;
                Ok(MathNode::Function { name: unescape_text(&raw).trim().to_string(), limits })
            }
            "overline" => Ok(MathNode::Overline(self.parse_arg()?)),
            "underline" => Ok(MathNode::Underline(self.parse_arg()?)),
            "not" => self.parse_negation(span),
            "bmod" => Ok(roman("mod", AtomClass::Bin)),
            "pmod" => {
                let arg = self.parse_arg()?;
                Ok(MathNode::Group(vec![
                    MathNode::Space(18.0),
                    roman("(", AtomClass::Open),
                    MathNode::Function { name: "mod".to_string(), limits: LimitsMode::NoLimits },
                    MathNode::Space(6.0),
                    MathNode::Group(arg),
                    roman(")", AtomClass::Close),
                ]))
            }
            "hline" | "notag" | "nonumber" => Ok(MathNode::Group(Vec::new())),
            _ => self.parse_table_command(name, span),
        }
    }

    fn parse_table_command(&mut self, name: &'a str, span: Range<usize>) -> Result<MathNode, MathError> {
        if let Some((text, class, variant)) = symbols::symbol_command(name) {
            return Ok(MathNode::Symbol { text: text.to_string(), class, variant });
        }
        if let Some((display, limits)) = symbols::function_name(name) {
            return Ok(MathNode::Function { name: display.to_string(), limits });
        }
        if let Some((symbol, integral)) = symbols::big_operator(name) {
            let limits = if integral { LimitsMode::NoLimits } else { LimitsMode::Auto };
            return Ok(MathNode::BigOperator { symbol: symbol.to_string(), limits, integral });
        }
        if let Some(mu) = symbols::space_command(name) {
            return Ok(MathNode::Space(mu));
        }
        if let Some(kind) = symbols::accent_command(name) {
            let body = self.parse_arg()?;
            return Ok(MathNode::Accent { kind, body });
        }
        if let Some((size, class)) = symbols::sized_delimiter_spec(name) {
            let delimiter = self.parse_delimiter()?;
            return Ok(MathNode::SizedDelimiter { delimiter, size, class });
        }
        if style_switch(name).is_some() {
            return Err(self.error_at(span, format!("\\{name} must appear inside a group")));
        }
        Err(self.error_at(span, format!("unknown command \\{name}")))
    }

    fn parse_negation(&mut self, span: Range<usize>) -> Result<MathNode, MathError> {
        match self.parse_primary()? {
            MathNode::Symbol { text, class, variant } => {
                Ok(MathNode::Symbol { text: symbols::negate(&text), class, variant })
            }
            MathNode::Identifier(c) => Ok(MathNode::Symbol {
                text: symbols::negate(&c.to_string()),
                class: AtomClass::Ord,
                variant: FontVariant::Italic,
            }),
            _ => Err(self.error_at(span, "\\not must be followed by a symbol")),
        }
    }

    fn parse_delimiter(&mut self) -> Result<Delimiter, MathError> {
        let span = self.current_span();
        let delimiter = match self.bump()? {
            Some(Token::Symbol(c) | Token::Unicode(c)) => symbols::delimiter_from_char(c),
            Some(Token::Escaped('{')) => Some(Delimiter::LeftBrace),
            Some(Token::Escaped('}')) => Some(Delimiter::RightBrace),
            Some(Token::Escaped('|')) => Some(Delimiter::DoubleVert),
            Some(Token::Command(name)) => symbols::delimiter_from_command(name),
            _ => None,
        };
        delimiter.ok_or_else(|| self.error_at(span, "expected a delimiter"))
    }

    fn parse_styled(&mut self, variant: FontVariant) -> Result<MathNode, MathError> {
        let body = self.parse_arg()?;
        Ok(MathNode::Styled { variant, body })
    }

    fn parse_alphabet(&mut self, alphabet: &str) -> Result<MathNode, MathError> {
        let body = self.parse_arg()?;
        Ok(MathNode::Group(body.into_iter().map(|n| remap_alphabet(alphabet, n)).collect()))
    }

    fn parse_text(&mut self, variant: FontVariant) -> Result<MathNode, MathError> {
        let raw = self.raw_group()?;
        Ok(MathNode::Text { content: unescape_text(&raw), variant })
    }

    fn parse_environment(&mut self, span: Range<usize>) -> Result<MathNode, MathError> {
        let name = self.raw_group()?.trim().to_string();
        let kind = match name.as_str() {
            "matrix" | "smallmatrix" | "gathered" | "array" | "subarray" => MatrixKind::Plain,
            "pmatrix" => MatrixKind::Paren,
            "bmatrix" => MatrixKind::Bracket,
            "Bmatrix" => MatrixKind::Brace,
            "vmatrix" => MatrixKind::Vert,
            "Vmatrix" => MatrixKind::DoubleVert,
            "cases" | "dcases" => MatrixKind::Cases,
            "aligned" | "align" | "align*" | "alignat" | "alignat*" | "split" | "eqnarray"
            | "eqnarray*" => MatrixKind::Aligned,
            other => return Err(self.error_at(span, format!("unknown environment '{other}'"))),
        };
        if matches!(name.as_str(), "array" | "subarray" | "alignat" | "alignat*") {
            // Column specification / column count: consumed, layout is automatic.
            self.raw_group()?;
        }

        let mut rows: Vec<Vec<MathList>> = Vec::new();
        let mut row: Vec<MathList> = Vec::new();
        loop {
            let cell = self.parse_list(Stops { env: true, ..Stops::default() })?;
            row.push(cell);
            match self.bump()? {
                Some(Token::Ampersand) => {}
                Some(Token::Escaped('\\')) => {
                    if matches!(self.raw_token(self.pos), Some(Token::Symbol('['))) {
                        self.pos += 1;
                        while let Some(token) = self.bump()? {
                            if token == Token::Symbol(']') {
                                break;
                            }
                        }
                    }
                    rows.push(std::mem::take(&mut row));
                }
                Some(Token::Command("end")) => {
                    let end_name = self.raw_group()?;
                    if end_name.trim() != name {
                        return Err(self.error_at(
                            self.previous_span(),
                            format!("environment '{name}' closed by '{}'", end_name.trim()),
                        ));
                    }
                    let trailing_empty = row.len() == 1 && row[0].is_empty();
                    if !trailing_empty || rows.is_empty() {
                        rows.push(row);
                    }
                    break;
                }
                _ => return Err(self.error_at(span, format!("unterminated environment '{name}'"))),
            }
        }
        Ok(MathNode::Matrix { kind, rows })
    }
}
