//! Math symbol definitions, command tables, and character conversions.

use super::ast::{AccentKind, AtomClass, Delimiter, FontVariant, LimitsMode, MathNode};

/// Commands mapping to symbol glyphs, their TeX atom class, and default font variant.
pub fn symbol_command(name: &str) -> Option<(&'static str, AtomClass, FontVariant)> {
    let res = match name {
        // Greek lowercase
        "alpha" => ("\u{03B1}", AtomClass::Ord, FontVariant::Italic),
        "beta" => ("\u{03B2}", AtomClass::Ord, FontVariant::Italic),
        "gamma" => ("\u{03B3}", AtomClass::Ord, FontVariant::Italic),
        "delta" => ("\u{03B4}", AtomClass::Ord, FontVariant::Italic),
        "epsilon" => ("\u{03F5}", AtomClass::Ord, FontVariant::Italic),
        "varepsilon" => ("\u{03B5}", AtomClass::Ord, FontVariant::Italic),
        "zeta" => ("\u{03B6}", AtomClass::Ord, FontVariant::Italic),
        "eta" => ("\u{03B7}", AtomClass::Ord, FontVariant::Italic),
        "theta" => ("\u{03B8}", AtomClass::Ord, FontVariant::Italic),
        "vartheta" => ("\u{03D1}", AtomClass::Ord, FontVariant::Italic),
        "iota" => ("\u{03B9}", AtomClass::Ord, FontVariant::Italic),
        "kappa" => ("\u{03BA}", AtomClass::Ord, FontVariant::Italic),
        "lambda" => ("\u{03BB}", AtomClass::Ord, FontVariant::Italic),
        "mu" => ("\u{03BC}", AtomClass::Ord, FontVariant::Italic),
        "nu" => ("\u{03BD}", AtomClass::Ord, FontVariant::Italic),
        "xi" => ("\u{03BE}", AtomClass::Ord, FontVariant::Italic),
        "pi" => ("\u{03C0}", AtomClass::Ord, FontVariant::Italic),
        "varpi" => ("\u{03D6}", AtomClass::Ord, FontVariant::Italic),
        "rho" => ("\u{03C1}", AtomClass::Ord, FontVariant::Italic),
        "varrho" => ("\u{03F1}", AtomClass::Ord, FontVariant::Italic),
        "sigma" => ("\u{03C3}", AtomClass::Ord, FontVariant::Italic),
        "varsigma" => ("\u{03C2}", AtomClass::Ord, FontVariant::Italic),
        "tau" => ("\u{03C4}", AtomClass::Ord, FontVariant::Italic),
        "upsilon" => ("\u{03C5}", AtomClass::Ord, FontVariant::Italic),
        "phi" => ("\u{03D5}", AtomClass::Ord, FontVariant::Italic),
        "varphi" => ("\u{03C6}", AtomClass::Ord, FontVariant::Italic),
        "chi" => ("\u{03C7}", AtomClass::Ord, FontVariant::Italic),
        "psi" => ("\u{03C8}", AtomClass::Ord, FontVariant::Italic),
        "omega" => ("\u{03C9}", AtomClass::Ord, FontVariant::Italic),

        // Greek uppercase (TeX renders upright roman by default)
        "Gamma" => ("\u{0393}", AtomClass::Ord, FontVariant::Roman),
        "Delta" => ("\u{0394}", AtomClass::Ord, FontVariant::Roman),
        "Theta" => ("\u{0398}", AtomClass::Ord, FontVariant::Roman),
        "Lambda" => ("\u{039B}", AtomClass::Ord, FontVariant::Roman),
        "Xi" => ("\u{039E}", AtomClass::Ord, FontVariant::Roman),
        "Pi" => ("\u{03A0}", AtomClass::Ord, FontVariant::Roman),
        "Sigma" => ("\u{03A3}", AtomClass::Ord, FontVariant::Roman),
        "Upsilon" => ("\u{03A5}", AtomClass::Ord, FontVariant::Roman),
        "Phi" => ("\u{03A6}", AtomClass::Ord, FontVariant::Roman),
        "Psi" => ("\u{03A8}", AtomClass::Ord, FontVariant::Roman),
        "Omega" => ("\u{03A9}", AtomClass::Ord, FontVariant::Roman),

        // Binary operators
        "pm" => ("\u{00B1}", AtomClass::Bin, FontVariant::Roman),
        "mp" => ("\u{2213}", AtomClass::Bin, FontVariant::Roman),
        "times" => ("\u{00D7}", AtomClass::Bin, FontVariant::Roman),
        "div" => ("\u{00F7}", AtomClass::Bin, FontVariant::Roman),
        "cdot" => ("\u{22C5}", AtomClass::Bin, FontVariant::Roman),
        "ast" | "star" => ("\u{2217}", AtomClass::Bin, FontVariant::Roman),
        "circ" => ("\u{2218}", AtomClass::Bin, FontVariant::Roman),
        "bullet" => ("\u{2219}", AtomClass::Bin, FontVariant::Roman),
        "cap" => ("\u{2229}", AtomClass::Bin, FontVariant::Roman),
        "cup" => ("\u{222A}", AtomClass::Bin, FontVariant::Roman),
        "vee" | "lor" => ("\u{2228}", AtomClass::Bin, FontVariant::Roman),
        "wedge" | "land" => ("\u{2227}", AtomClass::Bin, FontVariant::Roman),
        "oplus" => ("\u{2295}", AtomClass::Bin, FontVariant::Roman),
        "ominus" => ("\u{2296}", AtomClass::Bin, FontVariant::Roman),
        "otimes" => ("\u{2297}", AtomClass::Bin, FontVariant::Roman),
        "oslash" => ("\u{2298}", AtomClass::Bin, FontVariant::Roman),
        "odot" => ("\u{2299}", AtomClass::Bin, FontVariant::Roman),
        "setminus" => ("\u{2216}", AtomClass::Bin, FontVariant::Roman),
        "wr" => ("\u{2240}", AtomClass::Bin, FontVariant::Roman),

        // Relations
        "le" | "leq" => ("\u{2264}", AtomClass::Rel, FontVariant::Roman),
        "ge" | "geq" => ("\u{2265}", AtomClass::Rel, FontVariant::Roman),
        "ne" | "neq" => ("\u{2260}", AtomClass::Rel, FontVariant::Roman),
        "ll" => ("\u{226A}", AtomClass::Rel, FontVariant::Roman),
        "gg" => ("\u{226B}", AtomClass::Rel, FontVariant::Roman),
        "equiv" => ("\u{2261}", AtomClass::Rel, FontVariant::Roman),
        "sim" => ("\u{223C}", AtomClass::Rel, FontVariant::Roman),
        "simeq" => ("\u{2243}", AtomClass::Rel, FontVariant::Roman),
        "approx" => ("\u{2248}", AtomClass::Rel, FontVariant::Roman),
        "cong" => ("\u{2245}", AtomClass::Rel, FontVariant::Roman),
        "perp" => ("\u{22A5}", AtomClass::Rel, FontVariant::Roman),
        "mid" => ("\u{2223}", AtomClass::Rel, FontVariant::Roman),
        "parallel" => ("\u{2225}", AtomClass::Rel, FontVariant::Roman),
        "in" => ("\u{2208}", AtomClass::Rel, FontVariant::Roman),
        "notin" => ("\u{2209}", AtomClass::Rel, FontVariant::Roman),
        "ni" | "owns" => ("\u{220B}", AtomClass::Rel, FontVariant::Roman),
        "subset" => ("\u{2282}", AtomClass::Rel, FontVariant::Roman),
        "supset" => ("\u{2283}", AtomClass::Rel, FontVariant::Roman),
        "subseteq" => ("\u{2286}", AtomClass::Rel, FontVariant::Roman),
        "supseteq" => ("\u{2287}", AtomClass::Rel, FontVariant::Roman),
        "propto" => ("\u{221D}", AtomClass::Rel, FontVariant::Roman),

        // Arrows
        "leftarrow" | "gets" => ("\u{2190}", AtomClass::Rel, FontVariant::Roman),
        "rightarrow" | "to" => ("\u{2192}", AtomClass::Rel, FontVariant::Roman),
        "leftrightarrow" => ("\u{2194}", AtomClass::Rel, FontVariant::Roman),
        "Leftarrow" => ("\u{21D0}", AtomClass::Rel, FontVariant::Roman),
        "Rightarrow" => ("\u{21D2}", AtomClass::Rel, FontVariant::Roman),
        "Leftrightarrow" | "iff" => ("\u{21D4}", AtomClass::Rel, FontVariant::Roman),
        "mapsto" => ("\u{21A6}", AtomClass::Rel, FontVariant::Roman),
        "uparrow" => ("\u{2191}", AtomClass::Rel, FontVariant::Roman),
        "downarrow" => ("\u{2193}", AtomClass::Rel, FontVariant::Roman),

        // Ordinary symbols
        "infty" => ("\u{221E}", AtomClass::Ord, FontVariant::Roman),
        "partial" => ("\u{2202}", AtomClass::Ord, FontVariant::Italic),
        "nabla" => ("\u{2207}", AtomClass::Ord, FontVariant::Roman),
        "forall" => ("\u{2200}", AtomClass::Ord, FontVariant::Roman),
        "exists" => ("\u{2203}", AtomClass::Ord, FontVariant::Roman),
        "nexists" => ("\u{2204}", AtomClass::Ord, FontVariant::Roman),
        "emptyset" | "varnothing" => ("\u{2205}", AtomClass::Ord, FontVariant::Roman),
        "neg" | "lnot" => ("\u{00AC}", AtomClass::Ord, FontVariant::Roman),
        "hbar" => ("\u{210F}", AtomClass::Ord, FontVariant::Italic),
        "ell" => ("\u{2113}", AtomClass::Ord, FontVariant::Italic),
        "aleph" => ("\u{2135}", AtomClass::Ord, FontVariant::Roman),
        "prime" => ("\u{2032}", AtomClass::Ord, FontVariant::Roman),
        "cdots" => ("\u{22EF}", AtomClass::Inner, FontVariant::Roman),
        "ldots" | "dots" => ("\u{2026}", AtomClass::Inner, FontVariant::Roman),
        "vdots" => ("\u{22EE}", AtomClass::Inner, FontVariant::Roman),
        "ddots" => ("\u{22F1}", AtomClass::Inner, FontVariant::Roman),

        _ => return None,
    };
    Some(res)
}

/// Standard mathematical function names and whether they take limits in display style.
pub fn function_name(name: &str) -> Option<(&'static str, LimitsMode)> {
    let (display, limits) = match name {
        "lim" => ("lim", LimitsMode::Auto),
        "limsup" => ("limsup", LimitsMode::Auto),
        "liminf" => ("liminf", LimitsMode::Auto),
        "max" => ("max", LimitsMode::Auto),
        "min" => ("min", LimitsMode::Auto),
        "sup" => ("sup", LimitsMode::Auto),
        "inf" => ("inf", LimitsMode::Auto),
        "gcd" => ("gcd", LimitsMode::Auto),
        "det" => ("det", LimitsMode::Auto),
        "Pr" => ("Pr", LimitsMode::Auto),
        "sin" => ("sin", LimitsMode::NoLimits),
        "cos" => ("cos", LimitsMode::NoLimits),
        "tan" => ("tan", LimitsMode::NoLimits),
        "cot" => ("cot", LimitsMode::NoLimits),
        "sec" => ("sec", LimitsMode::NoLimits),
        "csc" => ("csc", LimitsMode::NoLimits),
        "arcsin" => ("arcsin", LimitsMode::NoLimits),
        "arccos" => ("arccos", LimitsMode::NoLimits),
        "arctan" => ("arctan", LimitsMode::NoLimits),
        "sinh" => ("sinh", LimitsMode::NoLimits),
        "cosh" => ("cosh", LimitsMode::NoLimits),
        "tanh" => ("tanh", LimitsMode::NoLimits),
        "coth" => ("coth", LimitsMode::NoLimits),
        "exp" => ("exp", LimitsMode::NoLimits),
        "log" => ("log", LimitsMode::NoLimits),
        "ln" => ("ln", LimitsMode::NoLimits),
        "lg" => ("lg", LimitsMode::NoLimits),
        "deg" => ("deg", LimitsMode::NoLimits),
        "dim" => ("dim", LimitsMode::NoLimits),
        "hom" => ("hom", LimitsMode::NoLimits),
        "ker" => ("ker", LimitsMode::NoLimits),
        "arg" => ("arg", LimitsMode::NoLimits),
        _ => return None,
    };
    Some((display, limits))
}

/// Big operators (integrals, summations) with flag whether it is an integral.
pub fn big_operator(name: &str) -> Option<(&'static str, bool)> {
    let (sym, integral) = match name {
        "sum" => ("\u{2211}", false),
        "prod" => ("\u{220F}", false),
        "coprod" => ("\u{2210}", false),
        "bigcap" => ("\u{22C2}", false),
        "bigcup" => ("\u{22C3}", false),
        "bigvee" => ("\u{22C1}", false),
        "bigwedge" => ("\u{22C0}", false),
        "bigoplus" => ("\u{2A01}", false),
        "bigotimes" => ("\u{2A02}", false),
        "bigodot" => ("\u{2A00}", false),
        "biguplus" => ("\u{2A04}", false),
        "bigsqcup" => ("\u{2A06}", false),
        "int" => ("\u{222B}", true),
        "iint" => ("\u{222C}", true),
        "iiint" => ("\u{222D}", true),
        "oint" => ("\u{222E}", true),
        _ => return None,
    };
    Some((sym, integral))
}

/// Spacing commands in mu (1 em = 18 mu).
pub fn space_command(name: &str) -> Option<f64> {
    match name {
        "," | "thinspace" => Some(3.0),
        ":" | ">" | "medspace" => Some(4.0),
        ";" | "thickspace" => Some(5.0),
        "!" | "negthinspace" => Some(-3.0),
        "enspace" => Some(9.0),
        "quad" => Some(18.0),
        "qquad" => Some(36.0),
        _ => None,
    }
}

/// Mathematical accents.
pub fn accent_command(name: &str) -> Option<AccentKind> {
    match name {
        "hat" => Some(AccentKind::Hat),
        "check" => Some(AccentKind::Check),
        "tilde" => Some(AccentKind::Tilde),
        "acute" => Some(AccentKind::Acute),
        "grave" => Some(AccentKind::Grave),
        "dot" => Some(AccentKind::Dot),
        "ddot" => Some(AccentKind::Ddot),
        "breve" => Some(AccentKind::Breve),
        "bar" => Some(AccentKind::Bar),
        "vec" => Some(AccentKind::Arrow),
        "mathring" => Some(AccentKind::Mathring),
        _ => None,
    }
}

/// Delimiter sizing prefixes: \big, \Big, \bigg, \Bigg.
pub fn sized_delimiter_spec(name: &str) -> Option<(u8, AtomClass)> {
    match name {
        "big" => Some((1, AtomClass::Ord)),
        "bigl" => Some((1, AtomClass::Open)),
        "bigr" => Some((1, AtomClass::Close)),
        "bigm" => Some((1, AtomClass::Rel)),
        "Big" => Some((2, AtomClass::Ord)),
        "Bigl" => Some((2, AtomClass::Open)),
        "Bigr" => Some((2, AtomClass::Close)),
        "Bigm" => Some((2, AtomClass::Rel)),
        "bigg" => Some((3, AtomClass::Ord)),
        "biggl" => Some((3, AtomClass::Open)),
        "biggr" => Some((3, AtomClass::Close)),
        "biggm" => Some((3, AtomClass::Rel)),
        "Bigg" => Some((4, AtomClass::Ord)),
        "Biggl" => Some((4, AtomClass::Open)),
        "Biggr" => Some((4, AtomClass::Close)),
        "Biggm" => Some((4, AtomClass::Rel)),
        _ => None,
    }
}

/// Negation of symbols for `\not`.
pub fn negate(text: &str) -> String {
    match text {
        "=" => "\u{2260}".to_string(),
        "<" => "\u{226E}".to_string(),
        ">" => "\u{226F}".to_string(),
        "\u{2264}" => "\u{2270}".to_string(),
        "\u{2265}" => "\u{2271}".to_string(),
        "\u{2208}" => "\u{2209}".to_string(),
        "\u{2282}" => "\u{2284}".to_string(),
        "\u{2283}" => "\u{2285}".to_string(),
        "\u{2286}" => "\u{2288}".to_string(),
        "\u{2287}" => "\u{2289}".to_string(),
        "\u{2261}" => "\u{2262}".to_string(),
        "\u{2203}" => "\u{2204}".to_string(),
        _ => {
            let mut s = text.to_string();
            s.push('\u{0338}'); // Combining Long Solidus Overlay
            s
        }
    }
}

/// Delimiter from single char.
pub fn delimiter_from_char(c: char) -> Option<Delimiter> {
    match c {
        '(' => Some(Delimiter::LeftParen),
        ')' => Some(Delimiter::RightParen),
        '[' => Some(Delimiter::LeftBracket),
        ']' => Some(Delimiter::RightBracket),
        '{' => Some(Delimiter::LeftBrace),
        '}' => Some(Delimiter::RightBrace),
        '|' => Some(Delimiter::Vert),
        '/' => Some(Delimiter::Null), // Treat slash as extensible or null
        '.' => Some(Delimiter::Null),
        '<' | '\u{27E8}' => Some(Delimiter::LeftAngle),
        '>' | '\u{27E9}' => Some(Delimiter::RightAngle),
        '\u{2308}' => Some(Delimiter::LeftCeil),
        '\u{2309}' => Some(Delimiter::RightCeil),
        '\u{230A}' => Some(Delimiter::LeftFloor),
        '\u{230B}' => Some(Delimiter::RightFloor),
        '\u{2016}' => Some(Delimiter::DoubleVert),
        _ => None,
    }
}

/// Delimiter from command name.
pub fn delimiter_from_command(name: &str) -> Option<Delimiter> {
    match name {
        "(" => Some(Delimiter::LeftParen),
        ")" => Some(Delimiter::RightParen),
        "[" => Some(Delimiter::LeftBracket),
        "]" => Some(Delimiter::RightBracket),
        "{" | "lbrace" => Some(Delimiter::LeftBrace),
        "}" | "rbrace" => Some(Delimiter::RightBrace),
        "|" | "vert" => Some(Delimiter::Vert),
        "\\|" | "Vert" => Some(Delimiter::DoubleVert),
        "langle" => Some(Delimiter::LeftAngle),
        "rangle" => Some(Delimiter::RightAngle),
        "lceil" => Some(Delimiter::LeftCeil),
        "rceil" => Some(Delimiter::RightCeil),
        "lfloor" => Some(Delimiter::LeftFloor),
        "rfloor" => Some(Delimiter::RightFloor),
        "." => Some(Delimiter::Null),
        _ => None,
    }
}

/// Single ASCII symbol token to MathNode.
pub fn char_symbol(c: char) -> MathNode {
    let (text, class) = match c {
        '+' => ("+", AtomClass::Bin),
        '-' => ("\u{2212}", AtomClass::Bin), // Unicode minus
        '*' => ("*", AtomClass::Bin),
        '=' => ("=", AtomClass::Rel),
        '<' => ("<", AtomClass::Rel),
        '>' => (">", AtomClass::Rel),
        ':' => (":", AtomClass::Rel),
        ',' => (",", AtomClass::Punct),
        ';' => (";", AtomClass::Punct),
        '.' => (".", AtomClass::Ord),
        '!' => ("!", AtomClass::Close),
        '?' => ("?", AtomClass::Close),
        '/' => ("/", AtomClass::Ord),
        '(' => ("(", AtomClass::Open),
        ')' => (")", AtomClass::Close),
        '[' => ("[", AtomClass::Open),
        ']' => ("]", AtomClass::Close),
        '|' => ("|", AtomClass::Ord),
        _ => {
            let mut s = String::with_capacity(4);
            s.push(c);
            return MathNode::Symbol {
                text: s,
                class: AtomClass::Ord,
                variant: FontVariant::Roman,
            };
        }
    };
    MathNode::Symbol {
        text: text.to_string(),
        class,
        variant: FontVariant::Roman,
    }
}

/// Single Unicode char token to MathNode.
pub fn unicode_symbol(c: char) -> MathNode {
    let mut s = String::with_capacity(4);
    s.push(c);
    let class = match c {
        '\u{2264}' | '\u{2265}' | '\u{2260}' | '\u{2192}' | '\u{2190}' | '\u{21D2}' | '\u{21D4}' => {
            AtomClass::Rel
        }
        '\u{00B1}' | '\u{00D7}' | '\u{00F7}' | '\u{22C5}' | '\u{2229}' | '\u{222A}' => {
            AtomClass::Bin
        }
        '\u{2211}' | '\u{220F}' | '\u{222B}' => AtomClass::Op,
        _ => AtomClass::Ord,
    };
    let variant = if ('\u{03B1}'..='\u{03C9}').contains(&c) || ('\u{03D1}'..='\u{03F5}').contains(&c) {
        FontVariant::Italic
    } else {
        FontVariant::Roman
    };
    MathNode::Symbol {
        text: s,
        class,
        variant,
    }
}

/// Alphabet remapping for \mathbb, \mathcal, \mathfrak, \mathscr.
pub fn map_alphabet(alphabet: &str, c: char) -> char {
    match alphabet {
        "mathbb" => match c {
            'C' => '\u{2102}',
            'H' => '\u{210D}',
            'N' => '\u{2115}',
            'P' => '\u{2119}',
            'Q' => '\u{211A}',
            'R' => '\u{211D}',
            'Z' => '\u{2124}',
            'A'..='Z' => {
                let offset = (c as u32) - ('A' as u32);
                char::from_u32(0x1D538 + offset).unwrap_or(c)
            }
            'a'..='z' => {
                let offset = (c as u32) - ('a' as u32);
                char::from_u32(0x1D552 + offset).unwrap_or(c)
            }
            '0'..='9' => {
                let offset = (c as u32) - ('0' as u32);
                char::from_u32(0x1D7D8 + offset).unwrap_or(c)
            }
            _ => c,
        },
        "mathcal" | "mathscr" => match c {
            'B' => '\u{212C}',
            'E' => '\u{2130}',
            'F' => '\u{2131}',
            'H' => '\u{210B}',
            'I' => '\u{2110}',
            'L' => '\u{2112}',
            'M' => '\u{2133}',
            'R' => '\u{211B}',
            'A'..='Z' => {
                let offset = (c as u32) - ('A' as u32);
                char::from_u32(0x1D49C + offset).unwrap_or(c)
            }
            'a'..='z' => {
                let offset = (c as u32) - ('a' as u32);
                char::from_u32(0x1D4B6 + offset).unwrap_or(c)
            }
            _ => c,
        },
        "mathfrak" => match c {
            'C' => '\u{212D}',
            'H' => '\u{210C}',
            'I' => '\u{2111}',
            'R' => '\u{211C}',
            'Z' => '\u{2128}',
            'A'..='Z' => {
                let offset = (c as u32) - ('A' as u32);
                char::from_u32(0x1D504 + offset).unwrap_or(c)
            }
            'a'..='z' => {
                let offset = (c as u32) - ('a' as u32);
                char::from_u32(0x1D51E + offset).unwrap_or(c)
            }
            _ => c,
        },
        _ => c,
    }
}
