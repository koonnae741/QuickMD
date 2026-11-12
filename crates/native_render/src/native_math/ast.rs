//! Abstract syntax tree for TeX math lists.

/// TeX atom classes (TeXbook ch. 17). Discriminants index the spacing table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtomClass {
    Ord = 0,
    Op = 1,
    Bin = 2,
    Rel = 3,
    Open = 4,
    Close = 5,
    Punct = 6,
    Inner = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontVariant {
    Roman,
    Italic,
    Bold,
    BoldItalic,
}

/// The four TeX math styles. Crampedness is tracked in `layout::StyleState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MathStyle {
    Display,
    Text,
    Script,
    ScriptScript,
}

impl MathStyle {
    /// Font scaling factor relative to the base size.
    pub const fn size_factor(self) -> f64 {
        match self {
            Self::Display | Self::Text => 1.0,
            Self::Script => 0.7,
            Self::ScriptScript => 0.5,
        }
    }

    pub const fn is_display(self) -> bool {
        matches!(self, Self::Display)
    }

    pub const fn is_script(self) -> bool {
        matches!(self, Self::Script | Self::ScriptScript)
    }

    pub const fn script(self) -> Self {
        match self {
            Self::Display | Self::Text => Self::Script,
            Self::Script | Self::ScriptScript => Self::ScriptScript,
        }
    }

    pub const fn fraction(self) -> Self {
        match self {
            Self::Display => Self::Text,
            Self::Text => Self::Script,
            Self::Script | Self::ScriptScript => Self::ScriptScript,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LimitsMode {
    Auto,
    Limits,
    NoLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Delimiter {
    Null,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
    LeftCeil,
    RightCeil,
    LeftFloor,
    RightFloor,
    Vert,
    DoubleVert,
    LeftAngle,
    RightAngle,
}

impl Delimiter {
    pub const fn glyph(self) -> Option<&'static str> {
        match self {
            Self::Null => None,
            Self::LeftParen => Some("("),
            Self::RightParen => Some(")"),
            Self::LeftBracket => Some("["),
            Self::RightBracket => Some("]"),
            Self::LeftBrace => Some("{"),
            Self::RightBrace => Some("}"),
            Self::LeftCeil => Some("\u{2308}"),
            Self::RightCeil => Some("\u{2309}"),
            Self::LeftFloor => Some("\u{230A}"),
            Self::RightFloor => Some("\u{230B}"),
            Self::Vert => Some("|"),
            Self::DoubleVert => Some("\u{2016}"),
            Self::LeftAngle => Some("\u{27E8}"),
            Self::RightAngle => Some("\u{27E9}"),
        }
    }

    pub const fn is_right_facing(self) -> bool {
        matches!(
            self,
            Self::RightParen
                | Self::RightBracket
                | Self::RightBrace
                | Self::RightCeil
                | Self::RightFloor
                | Self::RightAngle
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatrixKind {
    Plain,
    Paren,
    Bracket,
    Brace,
    Vert,
    DoubleVert,
    Cases,
    Aligned,
}

impl MatrixKind {
    pub const fn delimiters(self) -> (Delimiter, Delimiter) {
        match self {
            Self::Plain | Self::Aligned => (Delimiter::Null, Delimiter::Null),
            Self::Paren => (Delimiter::LeftParen, Delimiter::RightParen),
            Self::Bracket => (Delimiter::LeftBracket, Delimiter::RightBracket),
            Self::Brace => (Delimiter::LeftBrace, Delimiter::RightBrace),
            Self::Vert => (Delimiter::Vert, Delimiter::Vert),
            Self::DoubleVert => (Delimiter::DoubleVert, Delimiter::DoubleVert),
            Self::Cases => (Delimiter::LeftBrace, Delimiter::Null),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccentKind {
    Hat,
    Check,
    Tilde,
    Acute,
    Grave,
    Dot,
    Ddot,
    Breve,
    Bar,
    Arrow,
    Mathring,
}

impl AccentKind {
    pub const fn glyph(self) -> &'static str {
        match self {
            Self::Hat => "\u{02C6}",
            Self::Check => "\u{02C7}",
            Self::Tilde => "\u{02DC}",
            Self::Acute => "\u{02CA}",
            Self::Grave => "\u{02CB}",
            Self::Dot => "\u{02D9}",
            Self::Ddot => "\u{00A8}",
            Self::Breve => "\u{02D8}",
            Self::Bar => "\u{02C9}",
            Self::Arrow => "\u{2192}",
            Self::Mathring => "\u{02DA}",
        }
    }
}

pub type MathList = Vec<MathNode>;

#[derive(Debug, Clone, PartialEq)]
pub enum MathNode {
    Number(String),
    Identifier(char),
    Symbol {
        text: String,
        class: AtomClass,
        variant: FontVariant,
    },
    Function {
        name: String,
        limits: LimitsMode,
    },
    BigOperator {
        symbol: String,
        limits: LimitsMode,
        integral: bool,
    },
    Text {
        content: String,
        variant: FontVariant,
    },
    Group(MathList),
    Fraction {
        numerator: MathList,
        denominator: MathList,
        has_bar: bool,
        delimiters: Option<(Delimiter, Delimiter)>,
        style: Option<MathStyle>,
    },
    Radical {
        index: Option<MathList>,
        radicand: MathList,
    },
    Scripts {
        base: Box<MathNode>,
        sub: Option<MathList>,
        sup: Option<MathList>,
    },
    Delimited {
        left: Delimiter,
        body: MathList,
        right: Delimiter,
    },
    SizedDelimiter {
        delimiter: Delimiter,
        size: u8,
        class: AtomClass,
    },
    Matrix {
        kind: MatrixKind,
        rows: Vec<Vec<MathList>>,
    },
    Overline(MathList),
    Underline(MathList),
    Accent {
        kind: AccentKind,
        body: MathList,
    },
    Space(f64),
    StyleChange {
        style: MathStyle,
        body: MathList,
    },
    Styled {
        variant: FontVariant,
        body: MathList,
    },
}
