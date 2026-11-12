//! Recursive TeX math box layout (The TeXbook, Appendix G).
//!
//! Coordinates: x grows right, y grows **down**, origin = left end of baseline.

use std::sync::Arc;

use kurbo::{Affine, BezPath, Point, Rect, Shape, Vec2};

use super::ast::{
    AccentKind, AtomClass, Delimiter, FontVariant, LimitsMode, MathList, MathNode, MathStyle,
    MatrixKind,
};
use super::font::{MathConstants, MathFontContext, ShapedText};

// ───────────────────────── primitives & boxes ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathPaint {
    Fill,
    Stroke { width: f64 },
}

#[derive(Debug, Clone)]
pub enum Primitive {
    /// Shaped glyph run; `origin` is the run's baseline start.
    Glyphs { origin: Point, run: Arc<ShapedText> },
    /// Solid rectangle (fraction bars, overlines, radical bars).
    Rule(Rect),
    /// Vector outline (stretchy delimiters, radical signs).
    Path { path: BezPath, paint: PathPaint },
}

impl Primitive {
    fn translate(&mut self, offset: Vec2) {
        match self {
            Self::Glyphs { origin, .. } => *origin += offset,
            Self::Rule(rect) => *rect = *rect + offset,
            Self::Path { path, .. } => path.apply_affine(Affine::translate(offset)),
        }
    }
}

/// TeX box: width, height above baseline, depth below baseline.
#[derive(Debug, Clone, Default)]
pub struct MathBox {
    pub width: f64,
    pub height: f64,
    pub depth: f64,
    pub items: Vec<Primitive>,
}

impl MathBox {
    pub fn kern(width: f64) -> Self {
        Self { width, ..Self::default() }
    }

    pub fn from_run(run: Arc<ShapedText>) -> Self {
        Self {
            width: run.advance,
            height: run.height,
            depth: run.depth,
            items: vec![Primitive::Glyphs { origin: Point::ORIGIN, run }],
        }
    }

    pub fn total_height(&self) -> f64 {
        self.height + self.depth
    }

    /// Moves `other`'s primitives into `self` with its origin at (dx, dy).
    /// Does not touch `self`'s metrics.
    pub fn place(&mut self, mut other: MathBox, dx: f64, dy: f64) {
        let offset = Vec2::new(dx, dy);
        if offset != Vec2::ZERO {
            for item in &mut other.items {
                item.translate(offset);
            }
        }
        self.items.append(&mut other.items);
    }
}

/// Horizontal list packer (TeX `\hbox` without glue stretching).
struct HBuilder {
    out: MathBox,
    x: f64,
}

impl HBuilder {
    fn new() -> Self {
        Self { out: MathBox::default(), x: 0.0 }
    }

    fn push(&mut self, b: MathBox, shift_down: f64) {
        self.out.height = self.out.height.max(b.height - shift_down);
        self.out.depth = self.out.depth.max(b.depth + shift_down);
        let width = b.width;
        self.out.place(b, self.x, shift_down);
        self.x += width;
    }

    fn kern(&mut self, width: f64) {
        self.x += width;
    }

    fn finish(mut self) -> MathBox {
        self.out.width = self.x;
        self.out
    }
}

// ───────────────────────── styles & spacing ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyleState {
    pub style: MathStyle,
    pub cramped: bool,
}

impl StyleState {
    pub const fn new(style: MathStyle) -> Self {
        Self { style, cramped: false }
    }
    /// `\displaystyle` etc. always yield an uncramped style.
    const fn with_style(self, style: MathStyle) -> Self {
        Self { style, cramped: false }
    }
    const fn sup(self) -> Self {
        Self { style: self.style.script(), cramped: self.cramped }
    }
    const fn sub(self) -> Self {
        Self { style: self.style.script(), cramped: true }
    }
    const fn num(self) -> Self {
        Self { style: self.style.fraction(), cramped: self.cramped }
    }
    const fn denom(self) -> Self {
        Self { style: self.style.fraction(), cramped: true }
    }
    const fn cramp(self) -> Self {
        Self { style: self.style, cramped: true }
    }
}

/// TeXbook p.170. 1=thin, 2=medium, 3=thick; negative = only in D/T styles.
const SPACING: [[i8; 8]; 8] = [
    // Ord Op  Bin Rel Open Close Punct Inner
    [0, 1, -2, -3, 0, 0, 0, -1],     // Ord
    [1, 1, 0, -3, 0, 0, 0, -1],      // Op
    [-2, -2, 0, 0, -2, 0, 0, -2],    // Bin
    [-3, -3, 0, 0, -3, 0, 0, -3],    // Rel
    [0, 0, 0, 0, 0, 0, 0, 0],        // Open
    [0, 1, -2, -3, 0, 0, 0, -1],     // Close
    [-1, -1, 0, -1, -1, -1, -1, -1], // Punct
    [-1, 1, -2, -3, -1, 0, -1, -1],  // Inner
];

fn spacing_mu(left: AtomClass, right: AtomClass, style: MathStyle) -> f64 {
    let code = SPACING[left as usize][right as usize];
    let level = if code < 0 {
        if style.is_script() { 0 } else { -code }
    } else {
        code
    };
    match level {
        1 => 3.0,
        2 => 4.0,
        3 => 5.0,
        _ => 0.0,
    }
}

fn resolve_limits(mode: LimitsMode, style: MathStyle) -> bool {
    match mode {
        LimitsMode::Auto => style.is_display(),
        LimitsMode::Limits => true,
        LimitsMode::NoLimits => false,
    }
}

fn resolve_variant(context: Option<FontVariant>, natural: FontVariant) -> FontVariant {
    match (context, natural) {
        (None, v) => v,
        (
            Some(FontVariant::Bold | FontVariant::BoldItalic),
            FontVariant::Italic | FontVariant::BoldItalic,
        ) => FontVariant::BoldItalic,
        (Some(FontVariant::Bold | FontVariant::BoldItalic), _) => FontVariant::Bold,
        (Some(FontVariant::Roman), _) => FontVariant::Roman,
        (Some(FontVariant::Italic), v) => v,
    }
}

// ───────────────────────── atoms ─────────────────────────

#[derive(Clone, Copy)]
struct Ctx {
    st: StyleState,
    variant: Option<FontVariant>,
}

struct Atom {
    /// `None` for explicit kerns (they do not take part in inter-atom spacing).
    class: Option<AtomClass>,
    style: MathStyle,
    mbox: MathBox,
    italic: f64,
    is_char: bool,
    limits: bool,
}

impl Atom {
    fn new(class: AtomClass, style: MathStyle, mbox: MathBox) -> Self {
        Self { class: Some(class), style, mbox, italic: 0.0, is_char: false, limits: false }
    }
}

/// TeX rules 5 & 6: Bin atoms in non-binary positions become Ord.
fn reclassify_bins(atoms: &mut [Atom]) {
    let mut prev: Option<usize> = None;
    for i in 0..atoms.len() {
        let Some(class) = atoms[i].class else { continue };
        match class {
            AtomClass::Bin => {
                let demote = match prev.and_then(|p| atoms[p].class) {
                    None => true,
                    Some(c) => matches!(
                        c,
                        AtomClass::Bin | AtomClass::Op | AtomClass::Rel | AtomClass::Open | AtomClass::Punct
                    ),
                };
                if demote {
                    atoms[i].class = Some(AtomClass::Ord);
                }
            }
            AtomClass::Rel | AtomClass::Close | AtomClass::Punct => {
                if let Some(p) = prev {
                    if atoms[p].class == Some(AtomClass::Bin) {
                        atoms[p].class = Some(AtomClass::Ord);
                    }
                }
            }
            _ => {}
        }
        prev = Some(i);
    }
    if let Some(p) = prev {
        if atoms[p].class == Some(AtomClass::Bin) {
            atoms[p].class = Some(AtomClass::Ord);
        }
    }
}

// ───────────────────────── vector delimiter shapes ─────────────────────────

#[derive(Clone, Copy)]
struct Pen {
    em: f64,
    light: f64,
    heavy: f64,
    margin: f64,
}

fn fill(path: BezPath) -> Primitive {
    Primitive::Path { path, paint: PathPaint::Fill }
}

fn stroke(path: BezPath, width: f64) -> Primitive {
    Primitive::Path { path, paint: PathPaint::Stroke { width } }
}

fn rect_prim(x0: f64, y0: f64, x1: f64, y1: f64) -> Primitive {
    fill(Rect::new(x0, y0, x1, y1).to_path(0.1))
}

/// Crescent-shaped parenthesis: thick in the middle, tapered ends.
fn paren_shape(h: f64, pen: Pen) -> (f64, Vec<Primitive>) {
    let width = pen.em * (0.30 + 0.05 * h / pen.em).min(0.75);
    let xr = width - pen.margin;
    let xl = pen.margin;
    // Symmetric cubic: x(0.5) = 0.25·xr + 0.75·c  ⇒  solve for c.
    let c_out = (xl - 0.25 * xr) / 0.75;
    let c_in = (xl + pen.heavy - 0.25 * xr) / 0.75;
    let f = 0.17 * h;
    let cap = pen.light * 0.8;
    let mut p = BezPath::new();
    p.move_to((xr, 0.0));
    p.curve_to((c_out, f), (c_out, h - f), (xr, h));
    p.line_to((xr, h - cap));
    p.curve_to((c_in, h - f), (c_in, f), (xr, cap));
    p.close_path();
    (width, vec![fill(p)])
}

fn bracket_shape(h: f64, pen: Pen, top: bool, bottom: bool) -> (f64, Vec<Primitive>) {
    let width = pen.em * 0.40;
    let stem = pen.heavy * 0.85;
    let x0 = pen.margin + 0.04 * pen.em;
    let x1 = width - pen.margin;
    let mut items = vec![rect_prim(x0, 0.0, x0 + stem, h)];
    if top {
        items.push(rect_prim(x0, 0.0, x1, pen.light));
    }
    if bottom {
        items.push(rect_prim(x0, h - pen.light, x1, h));
    }
    (width, items)
}

fn vert_shape(h: f64, pen: Pen, count: usize) -> (f64, Vec<Primitive>) {
    let stem = pen.light * 1.2;
    let gap = 0.16 * pen.em;
    let ink = stem * count as f64 + gap * count.saturating_sub(1) as f64;
    let width = 2.0 * pen.margin + 0.12 * pen.em + ink;
    let mut x = (width - ink) / 2.0;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(rect_prim(x, 0.0, x + stem, h));
        x += stem + gap;
    }
    (width, items)
}

fn angle_shape(h: f64, pen: Pen) -> (f64, Vec<Primitive>) {
    let width = pen.em * (0.36 + 0.04 * h / pen.em).min(0.8);
    let sw = pen.light * 1.1;
    let mut p = BezPath::new();
    p.move_to((width - pen.margin, sw / 2.0));
    p.line_to((pen.margin + sw / 2.0, h / 2.0));
    p.line_to((width - pen.margin, h - sw / 2.0));
    (width, vec![stroke(p, sw)])
}

fn brace_shape(h: f64, pen: Pen) -> (f64, Vec<Primitive>) {
    let width = pen.em * 0.55;
    let sw = pen.light * 1.15;
    let xl = pen.margin + sw / 2.0;
    let xr = width - pen.margin - sw / 2.0;
    let xm = width / 2.0;
    let (y0, y1, ym) = (sw / 2.0, h - sw / 2.0, h / 2.0);
    let r = (xr - xm).max(0.0).min((y1 - y0) * 0.12);
    let mut p = BezPath::new();
    p.move_to((xr, y0));
    p.quad_to((xm, y0), (xm, y0 + r));
    p.line_to((xm, ym - r));
    p.quad_to((xm, ym), (xl, ym));
    p.quad_to((xm, ym), (xm, ym + r));
    p.line_to((xm, y1 - r));
    p.quad_to((xm, y1), (xr, y1));
    (width, vec![stroke(p, sw)])
}

/// Builds a delimiter of total height `h`; local coords: top at y = 0.
fn delimiter_shape(delimiter: Delimiter, h: f64, pen: Pen) -> (f64, Vec<Primitive>) {
    let (width, mut items) = match delimiter {
        Delimiter::Null => (0.0, Vec::new()),
        Delimiter::LeftParen | Delimiter::RightParen => paren_shape(h, pen),
        Delimiter::LeftBracket | Delimiter::RightBracket => bracket_shape(h, pen, true, true),
        Delimiter::LeftFloor | Delimiter::RightFloor => bracket_shape(h, pen, false, true),
        Delimiter::LeftCeil | Delimiter::RightCeil => bracket_shape(h, pen, true, false),
        Delimiter::LeftBrace | Delimiter::RightBrace => brace_shape(h, pen),
        Delimiter::Vert => vert_shape(h, pen, 1),
        Delimiter::DoubleVert => vert_shape(h, pen, 2),
        Delimiter::LeftAngle | Delimiter::RightAngle => angle_shape(h, pen),
    };
    if delimiter.is_right_facing() {
        let mirror = Affine::new([-1.0, 0.0, 0.0, 1.0, width, 0.0]);
        for item in &mut items {
            if let Primitive::Path { path, .. } = item {
                path.apply_affine(mirror);
            }
        }
    }
    (width, items)
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

fn column_alignment(kind: MatrixKind, column: usize) -> Align {
    match kind {
        MatrixKind::Cases => Align::Left,
        MatrixKind::Aligned => {
            if column % 2 == 0 { Align::Right } else { Align::Left }
        }
        _ => Align::Center,
    }
}

// ───────────────────────── engine ─────────────────────────

pub struct LayoutEngine<'f> {
    fonts: &'f mut MathFontContext,
    base_size: f64,
    constants: MathConstants,
}

impl<'f> LayoutEngine<'f> {
    /// `base_size` = em size of Display/Text style, in logical px.
    pub fn new(fonts: &'f mut MathFontContext, base_size: f64) -> Self {
        let constants = fonts.constants();
        let base_size = if base_size.is_finite() && base_size > 0.0 { base_size } else { 16.0 };
        Self { fonts, base_size, constants }
    }

    pub fn layout(&mut self, list: &[MathNode], style: MathStyle) -> MathBox {
        self.layout_list(list, Ctx { st: StyleState::new(style), variant: None }, false)
    }

    fn em(&self, style: MathStyle) -> f64 {
        self.base_size * style.size_factor()
    }

    fn pen(&self, em: f64) -> Pen {
        let theta = self.constants.rule_thickness * em;
        Pen {
            em,
            light: theta.max(0.045 * em),
            heavy: (2.0 * theta).max(0.085 * em),
            margin: 0.07 * em,
        }
    }

    fn list(&mut self, list: &[MathNode], ctx: Ctx) -> MathBox {
        self.layout_list(list, ctx, false)
    }

    fn collect_atoms(&mut self, list: &[MathNode], ctx: Ctx, out: &mut Vec<Atom>) {
        for node in list {
            match node {
                MathNode::StyleChange { style, body } => {
                    let inner = Ctx { st: ctx.st.with_style(*style), ..ctx };
                    self.collect_atoms(body, inner, out);
                }
                _ => out.push(self.layout_atom(node, ctx)),
            }
        }
    }

    /// Lays out a math list: atoms → Bin reclassification → inter-atom glue → hpack.
    fn layout_list(&mut self, list: &[MathNode], ctx: Ctx, leading_ord: bool) -> MathBox {
        let mut atoms = Vec::with_capacity(list.len() + 1);
        if leading_ord {
            atoms.push(Atom::new(AtomClass::Ord, ctx.st.style, MathBox::default()));
        }
        self.collect_atoms(list, ctx, &mut atoms);
        reclassify_bins(&mut atoms);

        let mut hb = HBuilder::new();
        let mut prev: Option<AtomClass> = None;
        for atom in atoms {
            if let Some(class) = atom.class {
                if let Some(p) = prev {
                    let mu = spacing_mu(p, class, atom.style);
                    if mu != 0.0 {
                        hb.kern(mu * self.em(atom.style) / 18.0);
                    }
                }
                prev = Some(class);
            }
            hb.push(atom.mbox, 0.0);
        }
        hb.finish()
    }

    fn glyph_atom(
        &mut self,
        text: &str,
        variant: FontVariant,
        size: f64,
        class: AtomClass,
        style: MathStyle,
        is_char: bool,
    ) -> Atom {
        let run = self.fonts.shape(text, variant, size);
        let italic = run.italic;
        Atom { class: Some(class), style, mbox: MathBox::from_run(run), italic, is_char, limits: false }
    }

    fn layout_atom(&mut self, node: &MathNode, ctx: Ctx) -> Atom {
        let style = ctx.st.style;
        let em = self.em(style);
        match node {
            MathNode::Number(text) => {
                let variant = match ctx.variant {
                    Some(FontVariant::Bold | FontVariant::BoldItalic) => FontVariant::Bold,
                    _ => FontVariant::Roman,
                };
                let single = text.chars().count() == 1;
                self.glyph_atom(text, variant, em, AtomClass::Ord, style, single)
            }
            MathNode::Identifier(c) => {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                let variant = ctx.variant.unwrap_or(FontVariant::Italic);
                self.glyph_atom(s, variant, em, AtomClass::Ord, style, true)
            }
            MathNode::Symbol { text, class, variant } => {
                let v = resolve_variant(ctx.variant, *variant);
                self.glyph_atom(text, v, em, *class, style, true)
            }
            MathNode::Function { name, limits } => {
                let v = resolve_variant(ctx.variant, FontVariant::Roman);
                let mut atom = self.glyph_atom(name, v, em, AtomClass::Op, style, false);
                atom.limits = resolve_limits(*limits, style);
                atom
            }
            MathNode::BigOperator { symbol, limits, integral } => {
                self.layout_big_op(symbol, *limits, *integral, ctx)
            }
            MathNode::Text { content, variant } => {
                self.glyph_atom(content, *variant, em, AtomClass::Ord, style, false)
            }
            MathNode::Group(list) => Atom::new(AtomClass::Ord, style, self.list(list, ctx)),
            MathNode::Fraction { numerator, denominator, has_bar, delimiters, style: forced } => {
                self.layout_fraction(numerator, denominator, *has_bar, *delimiters, *forced, ctx)
            }
            MathNode::Radical { index, radicand } => {
                self.layout_radical(index.as_deref(), radicand, ctx)
            }
            MathNode::Scripts { base, sub, sup } => {
                self.layout_scripts(base, sub.as_deref(), sup.as_deref(), ctx)
            }
            MathNode::Delimited { left, body, right } => {
                let inner = self.list(body, ctx);
                let wrapped = self.wrap_delimiters(inner, *left, *right, ctx.st);
                Atom::new(AtomClass::Inner, style, wrapped)
            }
            MathNode::SizedDelimiter { delimiter, size, class } => {
                const SIZES: [f64; 4] = [1.2, 1.8, 2.4, 3.0];
                let index = usize::from((*size).clamp(1, 4)) - 1;
                let b = self.make_delimiter(*delimiter, SIZES[index] * em, ctx.st);
                Atom::new(*class, style, b)
            }
            MathNode::Matrix { kind, rows } => self.layout_matrix(*kind, rows, ctx),
            MathNode::Overline(body) => self.layout_overline(body, ctx),
            MathNode::Underline(body) => self.layout_underline(body, ctx),
            MathNode::Accent { kind, body } => self.layout_accent(*kind, body, ctx),
            MathNode::Space(mu) => Atom {
                class: None,
                style,
                mbox: MathBox::kern(mu * em / 18.0),
                italic: 0.0,
                is_char: false,
                limits: false,
            },
            MathNode::StyleChange { style: s, body } => {
                let inner = Ctx { st: ctx.st.with_style(*s), ..ctx };
                Atom::new(AtomClass::Ord, *s, self.list(body, inner))
            }
            MathNode::Styled { variant, body } => {
                let inner = Ctx { variant: Some(*variant), ..ctx };
                Atom::new(AtomClass::Ord, style, self.list(body, inner))
            }
        }
    }

    // ── Rule 13: large operators (vertically centred on the math axis) ──
    fn layout_big_op(&mut self, symbol: &str, limits: LimitsMode, integral: bool, ctx: Ctx) -> Atom {
        let style = ctx.st.style;
        let em = self.em(style);
        let factor = match (style.is_display(), integral) {
            (true, true) => 2.0,
            (true, false) => 1.45,
            (false, true) => 1.3,
            (false, false) => 1.05,
        };
        let run = self.fonts.shape(symbol, FontVariant::Roman, em * factor);
        let axis = self.constants.axis_height * em;
        let shift = (run.height - run.depth) / 2.0 - axis;
        let italic = run.italic;
        let glyph = MathBox::from_run(run);
        let mut b = MathBox {
            width: glyph.width,
            height: glyph.height - shift,
            depth: glyph.depth + shift,
            items: Vec::new(),
        };
        b.place(glyph, 0.0, shift);
        Atom {
            class: Some(AtomClass::Op),
            style,
            mbox: b,
            italic,
            is_char: false,
            limits: resolve_limits(limits, style),
        }
    }

    // ── Rule 18: superscripts & subscripts ──
    fn layout_scripts(
        &mut self,
        base: &MathNode,
        sub: Option<&[MathNode]>,
        sup: Option<&[MathNode]>,
        ctx: Ctx,
    ) -> Atom {
        let nucleus = self.layout_atom(base, ctx);
        if nucleus.limits {
            return self.layout_limits(nucleus, sub, sup, ctx);
        }
        let st = ctx.st;
        let c = self.constants;
        let em = self.em(st.style);
        let script_em = self.em(st.style.script());
        let x_height = c.x_height * em;
        let theta = c.rule_thickness * em;

        let sup_box = sup.map(|l| self.list(l, Ctx { st: st.sup(), ..ctx }));
        let sub_box = sub.map(|l| self.list(l, Ctx { st: st.sub(), ..ctx }));

        let class = nucleus.class.unwrap_or(AtomClass::Ord);
        let italic = nucleus.italic;
        let is_char = nucleus.is_char;
        let nb = nucleus.mbox;

        // 18a: initial shifts for non-character nuclei.
        let (mut u, mut v) = if is_char {
            (0.0, 0.0)
        } else {
            (nb.height - c.sup_drop * script_em, nb.depth + c.sub_drop * script_em)
        };
        let w = nb.width;
        let mut out = MathBox { width: w, height: nb.height, depth: nb.depth, items: Vec::new() };
        out.place(nb, 0.0, 0.0);
        let mut script_width: f64 = 0.0;

        match (sup_box, sub_box) {
            (None, None) => {}
            // 18b: subscript only.
            (None, Some(sb)) => {
                v = v.max(c.sub1 * em).max(sb.height - 0.8 * x_height);
                script_width = sb.width;
                out.height = out.height.max(sb.height - v);
                out.depth = out.depth.max(sb.depth + v);
                out.place(sb, w, v);
            }
            // 18c–18f: superscript, optionally with subscript.
            (Some(sp), sub_box) => {
                let p = if st.cramped {
                    c.sup3
                } else if st.style.is_display() {
                    c.sup1
                } else {
                    c.sup2
                };
                u = u.max(p * em).max(sp.depth + 0.25 * x_height);
                if let Some(sb) = sub_box {
                    v = v.max(c.sub2 * em);
                    let gap = (u - sp.depth) - (sb.height - v);
                    if gap < 4.0 * theta {
                        v = 4.0 * theta - (u - sp.depth) + sb.height;
                        let psi = 0.8 * x_height - (u - sp.depth);
                        if psi > 0.0 {
                            u += psi;
                            v -= psi;
                        }
                    }
                    script_width = sb.width;
                    out.height = out.height.max(sb.height - v);
                    out.depth = out.depth.max(sb.depth + v);
                    out.place(sb, w, v);
                }
                script_width = script_width.max(sp.width + italic);
                out.height = out.height.max(sp.height + u);
                out.depth = out.depth.max(sp.depth - u);
                out.place(sp, w + italic, -u);
            }
        }
        out.width = w + script_width + c.script_space * em;
        Atom::new(class, st.style, out)
    }

    // ── Rule 13a: limits above/below operators ──
    fn layout_limits(
        &mut self,
        nucleus: Atom,
        sub: Option<&[MathNode]>,
        sup: Option<&[MathNode]>,
        ctx: Ctx,
    ) -> Atom {
        let st = ctx.st;
        let c = self.constants;
        let em = self.em(st.style);
        let sup_box = sup.map(|l| self.list(l, Ctx { st: st.sup(), ..ctx }));
        let sub_box = sub.map(|l| self.list(l, Ctx { st: st.sub(), ..ctx }));

        let delta = nucleus.italic;
        let class = nucleus.class.unwrap_or(AtomClass::Op);
        let nb = nucleus.mbox;
        let width = nb
            .width
            .max(sup_box.as_ref().map_or(0.0, |b| b.width))
            .max(sub_box.as_ref().map_or(0.0, |b| b.width));

        let (mut height, mut depth) = (nb.height, nb.depth);
        let mut placements: Vec<(MathBox, f64, f64)> = Vec::with_capacity(3);
        if let Some(sp) = sup_box {
            let kern = (c.big_op_spacing1 * em).max(c.big_op_spacing3 * em - sp.depth);
            let baseline = -(nb.height + kern + sp.depth);
            height = nb.height + kern + sp.depth + sp.height + c.big_op_spacing5 * em;
            let x = (width - sp.width) / 2.0 + delta / 2.0;
            placements.push((sp, x, baseline));
        }
        if let Some(sb) = sub_box {
            let kern = (c.big_op_spacing2 * em).max(c.big_op_spacing4 * em - sb.height);
            let baseline = nb.depth + kern + sb.height;
            depth = baseline + sb.depth + c.big_op_spacing5 * em;
            let x = (width - sb.width) / 2.0 - delta / 2.0;
            placements.push((sb, x, baseline));
        }
        let nx = (width - nb.width) / 2.0;
        placements.push((nb, nx, 0.0));

        let min_x = placements.iter().map(|p| p.1).fold(0.0, f64::min);
        let max_x = placements.iter().map(|p| p.1 + p.0.width).fold(width, f64::max);
        let mut out = MathBox { width: max_x - min_x, height, depth, items: Vec::new() };
        for (b, x, y) in placements {
            out.place(b, x - min_x, y);
        }
        Atom::new(class, st.style, out)
    }

    // ── Rule 15: generalized fractions ──
    fn layout_fraction(
        &mut self,
        numerator: &[MathNode],
        denominator: &[MathNode],
        has_bar: bool,
        delimiters: Option<(Delimiter, Delimiter)>,
        forced: Option<MathStyle>,
        ctx: Ctx,
    ) -> Atom {
        let outer_style = ctx.st.style;
        let st = match forced {
            Some(s) => ctx.st.with_style(s),
            None => ctx.st,
        };
        let c = self.constants;
        let em = self.em(st.style);
        let display = st.style.is_display();

        let nb = self.list(numerator, Ctx { st: st.num(), ..ctx });
        let db = self.list(denominator, Ctx { st: st.denom(), ..ctx });

        let default_rule = c.rule_thickness * em;
        let theta = if has_bar { default_rule } else { 0.0 };
        let axis = c.axis_height * em;

        // 15b
        let (mut u, mut v) = if display {
            (c.num1 * em, c.denom1 * em)
        } else if has_bar {
            (c.num2 * em, c.denom2 * em)
        } else {
            (c.num3 * em, c.denom2 * em)
        };

        if has_bar {
            // 15d: clearance between each part and the bar.
            let phi = if display { 3.0 * theta } else { theta };
            let above = (u - nb.depth) - (axis + theta / 2.0);
            if above < phi {
                u += phi - above;
            }
            let below = (axis - theta / 2.0) - (db.height - v);
            if below < phi {
                v += phi - below;
            }
        } else {
            // 15c: clearance between numerator and denominator.
            let phi = if display { 7.0 * default_rule } else { 3.0 * default_rule };
            let psi = (u - nb.depth) - (db.height - v);
            if psi < phi {
                u += (phi - psi) / 2.0;
                v += (phi - psi) / 2.0;
            }
        }

        // 15a/15e: horizontal centring with null-delimiter clearance.
        let pad = if delimiters.is_some() { 0.0 } else { c.null_delimiter_space * em };
        let inner_w = nb.width.max(db.width);
        let mut core = MathBox {
            width: inner_w + 2.0 * pad,
            height: (u + nb.height).max(if has_bar { axis + theta / 2.0 } else { 0.0 }),
            depth: (v + db.depth).max(0.0),
            items: Vec::new(),
        };
        let (nw, dw) = (nb.width, db.width);
        core.place(nb, pad + (inner_w - nw) / 2.0, -u);
        core.place(db, pad + (inner_w - dw) / 2.0, v);
        if has_bar {
            core.items.push(Primitive::Rule(Rect::new(
                pad,
                -(axis + theta / 2.0),
                pad + inner_w,
                -(axis - theta / 2.0),
            )));
        }

        let result = match delimiters {
            None => core,
            Some((left, right)) => {
                let size = if display { c.delim1 } else { c.delim2 } * em;
                let lb = self.make_delimiter(left, size, st);
                let rb = self.make_delimiter(right, size, st);
                let mut hb = HBuilder::new();
                hb.push(lb, 0.0);
                hb.push(core, 0.0);
                hb.push(rb, 0.0);
                hb.finish()
            }
        };
        Atom::new(AtomClass::Inner, outer_style, result)
    }

    // ── Rule 11: radicals ──
    fn layout_radical(&mut self, index: Option<&[MathNode]>, radicand: &[MathNode], ctx: Ctx) -> Atom {
        let st = ctx.st;
        let style = st.style;
        let em = self.em(style);
        let c = self.constants;
        let theta = c.rule_thickness * em;
        let x_height = c.x_height * em;
        let pen = self.pen(em);

        let body = self.list(radicand, Ctx { st: st.cramp(), ..ctx });
        let phi = if style.is_display() { x_height } else { theta };
        let mut psi = theta + phi / 4.0;
        let needed = body.height + body.depth + psi + theta;
        let total = needed.max(em);
        if total > needed {
            // Distribute the surplus clearance evenly (TeX rule 11).
            psi += (total - needed) / 2.0;
        }
        let bar_top = body.height + psi + theta; // above baseline
        let sign_bottom = bar_top - total; // above baseline (negative ⇒ below)

        let growth = (total / em - 1.0).max(0.0);
        let sign_w = em * (0.56 + 0.08 * growth).min(1.0);

        // Optional index (LaTeX \root: kern 5mu, raise 60%, kern −10mu).
        let mu = em / 18.0;
        let mut sign_x = 0.0;
        let mut index_box: Option<(MathBox, f64, f64)> = None;
        if let Some(idx) = index {
            let ib = self.list(idx, Ctx { st: StyleState::new(MathStyle::ScriptScript), ..ctx });
            let before = 5.0 * mu;
            let after = -10.0 * mu;
            let raise = sign_bottom + 0.6 * total + ib.depth;
            let span = before + ib.width + after;
            sign_x = span.max(0.0);
            index_box = Some((ib, before, raise));
        }

        let tick_h = (0.5 * em).min(total * 0.55);
        let p0 = Point::new(sign_x + 0.06 * em, -(sign_bottom + tick_h * 0.82));
        let p1 = Point::new(sign_x + 0.06 * em + sign_w * 0.16, -(sign_bottom + tick_h));
        let p2 = Point::new(sign_x + sign_w * 0.46, -(sign_bottom + pen.heavy * 0.3));
        let p3 = Point::new(sign_x + sign_w, -(bar_top - theta / 2.0));

        let segment = |a: Point, b: Point, width: f64| {
            let mut p = BezPath::new();
            p.move_to(a);
            p.line_to(b);
            stroke(p, width)
        };

        let right_pad = 0.04 * em;
        let body_x = sign_x + sign_w;
        let mut out = MathBox {
            width: body_x + body.width + right_pad,
            height: bar_top + theta,
            depth: body.depth.max(-sign_bottom),
            items: Vec::new(),
        };
        out.items.push(segment(p0, p1, pen.light));
        out.items.push(segment(p1, p2, pen.heavy));
        out.items.push(segment(p2, p3, pen.light));
        out.items.push(Primitive::Rule(Rect::new(
            body_x - pen.light * 0.5,
            -bar_top,
            body_x + body.width + right_pad,
            -(bar_top - theta),
        )));
        out.place(body, body_x, 0.0);

        if let Some((ib, x, raise)) = index_box {
            out.height = out.height.max(raise + ib.height);
            out.place(ib, x, -raise);
        }
        Atom::new(AtomClass::Ord, style, out)
    }

    // ── Rule 19 / \left…\right ──
    fn wrap_delimiters(&mut self, body: MathBox, left: Delimiter, right: Delimiter, st: StyleState) -> MathBox {
        let em = self.em(st.style);
        let c = self.constants;
        let axis = c.axis_height * em;
        let delta = (body.height - axis).max(body.depth + axis);
        let total = (2.0 * delta * c.delimiter_factor).max(2.0 * delta - c.delimiter_shortfall * em);
        let lb = self.make_delimiter(left, total, st);
        let rb = self.make_delimiter(right, total, st);
        let mut hb = HBuilder::new();
        hb.push(lb, 0.0);
        hb.push(body, 0.0);
        hb.push(rb, 0.0);
        hb.finish()
    }

    /// Natural glyph if it is tall enough, otherwise a vector delimiter
    /// of exactly `total` height, centred on the math axis.
    fn make_delimiter(&mut self, delimiter: Delimiter, total: f64, st: StyleState) -> MathBox {
        let em = self.em(st.style);
        let Some(glyph) = delimiter.glyph() else {
            return MathBox::kern(self.constants.null_delimiter_space * em);
        };
        let run = self.fonts.shape(glyph, FontVariant::Roman, em);
        let natural = run.height + run.depth;
        if !total.is_finite() || (natural > 0.0 && total <= natural * 1.01) {
            return MathBox::from_run(run);
        }
        let axis = self.constants.axis_height * em;
        let (width, items) = delimiter_shape(delimiter, total, self.pen(em));
        let top = axis + total / 2.0;
        let mut out = MathBox { width, height: top, depth: total - top, items: Vec::new() };
        out.place(MathBox { items, ..MathBox::default() }, 0.0, -top);
        out
    }

    // ── Matrices, cases, aligned ──
    fn layout_matrix(&mut self, kind: MatrixKind, rows: &[Vec<MathList>], ctx: Ctx) -> Atom {
        let outer = ctx.st.style;
        let aligned = kind == MatrixKind::Aligned;
        let cell_style = if outer.is_script() {
            outer
        } else if aligned {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let cell_ctx = Ctx { st: StyleState::new(cell_style), variant: ctx.variant };
        let em = self.em(cell_style);
        let ncols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if rows.is_empty() || ncols == 0 {
            return Atom::new(AtomClass::Ord, outer, MathBox::default());
        }

        let mut grid: Vec<Vec<MathBox>> = Vec::with_capacity(rows.len());
        for row in rows {
            let cells: Vec<MathBox> = row
                .iter()
                .enumerate()
                .map(|(j, cell)| self.layout_list(cell, cell_ctx, aligned && j % 2 == 1))
                .collect();
            grid.push(cells);
        }

        let strut_height = 0.84 * em;
        let strut_depth = 0.36 * em;
        let row_gap = match kind {
            MatrixKind::Aligned => 0.3 * em,
            MatrixKind::Cases => 0.15 * em,
            _ => 0.0,
        };

        let mut col_widths = vec![0.0f64; ncols];
        let mut row_heights = Vec::with_capacity(grid.len());
        let mut row_depths = Vec::with_capacity(grid.len());
        for row in &grid {
            let mut h = strut_height;
            let mut d = strut_depth;
            for (j, cell) in row.iter().enumerate() {
                col_widths[j] = col_widths[j].max(cell.width);
                h = h.max(cell.height);
                d = d.max(cell.depth);
            }
            row_heights.push(h);
            row_depths.push(d);
        }

        let col_gap = |j: usize| -> f64 {
            match kind {
                MatrixKind::Aligned => {
                    if j % 2 == 1 { 2.0 * em } else { 0.0 }
                }
                _ => em,
            }
        };
        let mut col_x = Vec::with_capacity(ncols);
        let mut x = 0.0;
        for (j, width) in col_widths.iter().enumerate() {
            col_x.push(x);
            x += width;
            if j + 1 < ncols {
                x += col_gap(j);
            }
        }
        let total_width = x;
        let total_height: f64 = row_heights.iter().zip(&row_depths).map(|(h, d)| h + d).sum::<f64>()
            + row_gap * (grid.len() - 1) as f64;

        let axis = self.constants.axis_height * self.em(outer);
        let top = axis + total_height / 2.0;
        let mut body = MathBox {
            width: total_width,
            height: top,
            depth: total_height - top,
            items: Vec::new(),
        };
        let mut y = -top;
        for (i, row) in grid.into_iter().enumerate() {
            let baseline = y + row_heights[i];
            for (j, cell) in row.into_iter().enumerate() {
                let slack = col_widths[j] - cell.width;
                let dx = match column_alignment(kind, j) {
                    Align::Left => 0.0,
                    Align::Center => slack / 2.0,
                    Align::Right => slack,
                };
                body.place(cell, col_x[j] + dx, baseline);
            }
            y = baseline + row_depths[i] + row_gap;
        }

        let (left, right) = kind.delimiters();
        if left == Delimiter::Null && right == Delimiter::Null {
            return Atom::new(AtomClass::Ord, outer, body);
        }
        let wrapped = self.wrap_delimiters(body, left, right, ctx.st);
        Atom::new(AtomClass::Inner, outer, wrapped)
    }

    // ── Rule 9: \overline ──
    fn layout_overline(&mut self, body: &[MathNode], ctx: Ctx) -> Atom {
        let em = self.em(ctx.st.style);
        let theta = self.constants.rule_thickness * em;
        let b = self.list(body, Ctx { st: ctx.st.cramp(), ..ctx });
        let (w, h, d) = (b.width, b.height, b.depth);
        let mut out = MathBox { width: w, height: h + 5.0 * theta, depth: d, items: Vec::new() };
        out.place(b, 0.0, 0.0);
        out.items.push(Primitive::Rule(Rect::new(0.0, -(h + 4.0 * theta), w, -(h + 3.0 * theta))));
        Atom::new(AtomClass::Ord, ctx.st.style, out)
    }

    // ── Rule 10: \underline ──
    fn layout_underline(&mut self, body: &[MathNode], ctx: Ctx) -> Atom {
        let em = self.em(ctx.st.style);
        let theta = self.constants.rule_thickness * em;
        let b = self.list(body, ctx);
        let (w, h, d) = (b.width, b.height, b.depth);
        let mut out = MathBox { width: w, height: h, depth: d + 5.0 * theta, items: Vec::new() };
        out.place(b, 0.0, 0.0);
        out.items.push(Primitive::Rule(Rect::new(0.0, d + 3.0 * theta, w, d + 4.0 * theta)));
        Atom::new(AtomClass::Ord, ctx.st.style, out)
    }

    // ── Rule 12: accents ──
    fn layout_accent(&mut self, kind: AccentKind, body: &[MathNode], ctx: Ctx) -> Atom {
        let em = self.em(ctx.st.style);
        let c = self.constants;
        let inner = Ctx { st: ctx.st.cramp(), ..ctx };
        let (base, skew) = if body.len() == 1 {
            let atom = self.layout_atom(&body[0], inner);
            let skew = if atom.is_char { atom.italic * 0.5 } else { 0.0 };
            (atom.mbox, skew)
        } else {
            (self.list(body, inner), 0.0)
        };
        let accent_size = if kind == AccentKind::Arrow { em * 0.8 } else { em };
        let accent = self.fonts.shape(kind.glyph(), FontVariant::Roman, accent_size);
        let gap = 0.06 * em;
        let base_top = base.height.max(c.x_height * em);
        let raise = base_top + gap + accent.depth;
        let ink_w = accent.ink_right - accent.ink_left;
        let accent_x = (base.width - ink_w) / 2.0 - accent.ink_left + skew;
        let mut out = MathBox {
            width: base.width,
            height: base.height.max(raise + accent.height),
            depth: base.depth,
            items: Vec::new(),
        };
        out.place(base, 0.0, 0.0);
        out.place(MathBox::from_run(accent), accent_x, -raise);
        Atom::new(AtomClass::Ord, ctx.st.style, out)
    }
}