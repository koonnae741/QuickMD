//! Automatic table layout (CSS `table-layout: auto` style).
//!
//! 1. Measure each column's min-content (longest unbreakable run) and
//!    max-content (unwrapped) width, padding included.
//! 2. Distribute the available width:
//!    * Σmax ≤ available → every column gets its max-content width.
//!    * Σmin ≥ available → every column gets its min-content width (overflow,
//!      the renderer scrolls horizontally).
//!    * otherwise `wᵢ = minᵢ + (maxᵢ − minᵢ) · (avail − Σmin) / (Σmax − Σmin)`.
//! 3. Re-shape every cell at its final width to get row heights.

use cosmic_text::{FontSystem, Wrap};

use crate::ast::TableData;
use crate::shaper::{TextShaper, TextStyle};

/// Slack added to measured widths so float rounding never triggers a wrap.
const WIDTH_EPSILON: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableStyle {
    pub cell_padding_x: f32,
    pub cell_padding_y: f32,
    pub border_width: f32,
    /// Minimum outer column width (padding included).
    pub min_column_width: f32,
}

impl Default for TableStyle {
    fn default() -> Self {
        Self { cell_padding_x: 12.0, cell_padding_y: 7.0, border_width: 1.0, min_column_width: 36.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ColumnMetrics {
    pub min_content: f32,
    pub max_content: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableLayout {
    /// Outer column widths (padding included, borders excluded).
    pub column_widths: Vec<f32>,
    /// Left edge of each column, relative to the table origin.
    pub column_x: Vec<f32>,
    /// Row heights; index 0 is the header row.
    pub row_heights: Vec<f32>,
    /// Top edge of each row, relative to the table origin.
    pub row_y: Vec<f32>,
    pub total_width: f32,
    pub total_height: f32,
    /// True when even min-content widths exceed the available width.
    pub overflows: bool,
}

impl TableLayout {
    pub fn header_height(&self) -> f32 {
        self.row_heights.first().copied().unwrap_or(0.0)
    }

    /// Cell box (padding included) relative to the table origin.
    pub fn cell_rect(&self, row: usize, col: usize) -> Option<tiny_skia::Rect> {
        let x = *self.column_x.get(col)?;
        let y = *self.row_y.get(row)?;
        let w = *self.column_widths.get(col)?;
        let h = *self.row_heights.get(row)?;
        tiny_skia::Rect::from_xywh(x, y, w, h)
    }
}

/// Pure width solver (no fonts needed). Returns `(widths, overflows)`.
pub fn solve_column_widths(columns: &[ColumnMetrics], available: f32) -> (Vec<f32>, bool) {
    if columns.is_empty() {
        return (Vec::new(), false);
    }
    let available = if available.is_finite() { available.max(0.0) } else { f32::MAX };
    let sanitize = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };

    let mins: Vec<f32> = columns.iter().map(|c| sanitize(c.min_content)).collect();
    let maxs: Vec<f32> = columns
        .iter()
        .zip(&mins)
        .map(|(c, &min)| sanitize(c.max_content).max(min))
        .collect();
    let sum_min: f32 = mins.iter().sum();
    let sum_max: f32 = maxs.iter().sum();

    if sum_max <= available {
        return (maxs, false);
    }
    if sum_min >= available {
        return (mins, sum_min > available);
    }
    let extra = available - sum_min;
    let flex = sum_max - sum_min; // > 0 here
    let widths = mins
        .iter()
        .zip(&maxs)
        .map(|(&min, &max)| min + (max - min) * (extra / flex))
        .collect();
    (widths, false)
}

/// Measures min/max-content per column (outer widths, padding included).
pub fn measure_columns(
    table: &TableData,
    font_size: f32,
    style: &TableStyle,
    shaper: &mut TextShaper,
    font_system: &mut FontSystem,
) -> Vec<ColumnMetrics> {
    let cols = table.column_count();
    let pad = 2.0 * style.cell_padding_x.max(0.0);
    let floor = style.min_column_width.max(pad);
    let mut out = vec![ColumnMetrics { min_content: floor, max_content: floor }; cols];

    for (r, row) in table.all_rows().enumerate() {
        let text_style = if r == 0 { TextStyle::bold(font_size) } else { TextStyle::body(font_size) };
        for (c, cell) in row.iter().enumerate().take(cols) {
            if cell.spans.is_empty() {
                continue;
            }
            let min = shaper.min_content_width(font_system, &cell.spans, text_style) + pad + WIDTH_EPSILON;
            let max = shaper.max_content_width(font_system, &cell.spans, text_style) + pad + WIDTH_EPSILON;
            let m = &mut out[c];
            m.min_content = m.min_content.max(min);
            m.max_content = m.max_content.max(max.max(min));
        }
    }
    out
}

/// Full table layout: column widths, row heights and cell origins.
pub fn layout_table(
    table: &TableData,
    available_width: f32,
    font_size: f32,
    style: &TableStyle,
    shaper: &mut TextShaper,
    font_system: &mut FontSystem,
) -> TableLayout {
    let cols = table.column_count();
    if cols == 0 {
        return TableLayout::default();
    }
    let border = style.border_width.max(0.0);
    let pad_x = style.cell_padding_x.max(0.0);
    let pad_y = style.cell_padding_y.max(0.0);

    let metrics = measure_columns(table, font_size, style, shaper, font_system);
    let avail_cols = (available_width - border * (cols as f32 + 1.0)).max(0.0);
    let (column_widths, overflows) = solve_column_widths(&metrics, avail_cols);

    let min_row = shaper.line_height(font_size) + 2.0 * pad_y;
    let mut row_heights = Vec::with_capacity(table.rows.len() + 1);
    for (r, row) in table.all_rows().enumerate() {
        let text_style = if r == 0 { TextStyle::bold(font_size) } else { TextStyle::body(font_size) };
        let mut h = min_row;
        for (c, cell) in row.iter().enumerate().take(cols) {
            if cell.spans.is_empty() {
                continue;
            }
            let inner = (column_widths[c] - 2.0 * pad_x).max(1.0) + WIDTH_EPSILON;
            let m = shaper.measure_spans(font_system, &cell.spans, text_style, Some(inner), Wrap::WordOrGlyph);
            h = h.max(m.height + 2.0 * pad_y);
        }
        row_heights.push(h.ceil());
    }

    let mut column_x = Vec::with_capacity(cols);
    let mut x = border;
    for w in &column_widths {
        column_x.push(x);
        x += w + border;
    }
    let mut row_y = Vec::with_capacity(row_heights.len());
    let mut y = border;
    for h in &row_heights {
        row_y.push(y);
        y += h + border;
    }

    TableLayout {
        column_widths,
        column_x,
        row_heights,
        row_y,
        total_width: x,
        total_height: y,
        overflows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Alignment, InlineSpan, TableCell};
    use crate::shaper::TypographyConfig;
    use crate::test_support::font_system;

    fn cm(min: f32, max: f32) -> ColumnMetrics {
        ColumnMetrics { min_content: min, max_content: max }
    }

    #[test]
    fn natural_widths_when_everything_fits() {
        let (w, overflow) = solve_column_widths(&[cm(50.0, 100.0), cm(50.0, 100.0)], 300.0);
        assert_eq!(w, vec![100.0, 100.0]);
        assert!(!overflow);
    }

    #[test]
    fn proportional_distribution_between_min_and_max() {
        let (w, overflow) = solve_column_widths(&[cm(50.0, 150.0), cm(50.0, 250.0)], 200.0);
        assert!(!overflow);
        assert!((w.iter().sum::<f32>() - 200.0).abs() < 1e-3);
        assert!((w[0] - (50.0 + 100.0 / 3.0)).abs() < 1e-3);
        assert!((w[1] - (50.0 + 200.0 / 3.0)).abs() < 1e-3);
    }

    #[test]
    fn overflow_falls_back_to_min_content() {
        let (w, overflow) = solve_column_widths(&[cm(200.0, 400.0), cm(200.0, 300.0)], 300.0);
        assert_eq!(w, vec![200.0, 200.0]);
        assert!(overflow);
    }

    #[test]
    fn degenerate_inputs_are_sanitised() {
        assert_eq!(solve_column_widths(&[], 100.0), (vec![], false));
        let (w, _) = solve_column_widths(&[cm(f32::NAN, 10.0), cm(30.0, 5.0)], f32::INFINITY);
        assert_eq!(w, vec![10.0, 30.0]);
    }

    #[test]
    fn full_table_layout_is_consistent() {
        let mut fs = font_system();
        let mut shaper = TextShaper::new(TypographyConfig::default());
        let cell = |s: &str| TableCell { spans: vec![InlineSpan::Text(s.into())], alignment: Alignment::Left };
        let table = TableData {
            headers: vec![cell("Name"), cell("คำอธิบาย")],
            rows: vec![
                vec![cell("alpha"), cell("ข้อความภาษาไทยยาวๆ ที่ต้องตัดบรรทัดอย่างปลอดภัย ".repeat(3).as_str())],
                vec![cell("beta"), cell("short")],
            ],
        };
        let style = TableStyle::default();
        let tl = layout_table(&table, 420.0, 16.0, &style, &mut shaper, &mut fs);

        assert_eq!(tl.column_widths.len(), 2);
        assert_eq!(tl.row_heights.len(), 3);
        let min_row = shaper.line_height(16.0) + 2.0 * style.cell_padding_y;
        assert!(tl.row_heights.iter().all(|&h| h >= min_row));
        let expected_h: f32 = tl.row_heights.iter().sum::<f32>() + style.border_width * 4.0;
        assert!((tl.total_height - expected_h).abs() < 1e-3);
        if !tl.overflows {
            assert!(tl.total_width <= 420.0 + 0.01);
        }
        assert!(tl.cell_rect(2, 1).is_some());
        assert!(tl.cell_rect(3, 0).is_none());
    }
}