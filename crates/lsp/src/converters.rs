// ============================================================================
// NOTICE: Full documentation, design decisions, and fix history for this file
// live in docs/ubel_stratum_lsp.md, section "converters.rs"
// ============================================================================
// crates/lsp/src/converters.rs
//! Compiler diagnostics to LSP diagnostics.
//!
//! Compiler spans carry byte offsets plus a 1-based line and column. LSP
//! positions are 0-based, and their column counts UTF-16 code units, so the
//! conversion walks the source text instead of trusting the compiler's
//! column on lines that contain non-ASCII characters.

use tower_lsp::lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location,
    NumberOrString, Position, Range, Url,
};
use ubel_stratum::error_management::Diagnostic;
use ubel_stratum::lexer::Span;

/// LSP position for a byte offset in `source`. An offset past the end is
/// clamped to the end; one inside a multi-byte character is moved back to
/// that character's start.
pub fn position_at(source: &str, offset: usize) -> Position {
    let mut offset = offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &source[..offset];
    let line = before.bytes().filter(|&b| b == b'\n').count() as u32;
    let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = source[line_start..offset].encode_utf16().count() as u32;
    Position::new(line, character)
}

/// LSP range for a compiler span. Falls back to the span's own line and
/// column, one character wide, when its offsets do not fit the text (spans
/// produced while parsing a string-interpolation hole can be relative to
/// the hole, not the file).
pub fn range_for(span: &Span, source: &str) -> Range {
    if span.start <= span.end && span.end <= source.len() {
        return Range::new(position_at(source, span.start), position_at(source, span.end));
    }
    let line = span.line.saturating_sub(1) as u32;
    let col  = span.column.saturating_sub(1) as u32;
    Range::new(Position::new(line, col), Position::new(line, col + 1))
}

/// Convert one compiler diagnostic. A suggestion becomes a `help:` line
/// after the message, and secondary spans become related information.
pub fn to_diagnostic(d: &Diagnostic, source: &str, uri: &Url) -> LspDiagnostic {
    let mut message = d.message.clone();
    if let Some(s) = &d.suggestion {
        message.push_str("\nhelp: ");
        message.push_str(s);
    }

    let related: Vec<DiagnosticRelatedInformation> = d.secondary.iter()
        .map(|(span, label)| DiagnosticRelatedInformation {
            location: Location::new(uri.clone(), range_for(span, source)),
            message:  label.clone(),
        })
        .collect();

    LspDiagnostic {
        range:               range_for(&d.primary_span, source),
        severity:            Some(DiagnosticSeverity::ERROR),
        code:                Some(NumberOrString::String(d.code.to_string())),
        source:              Some("ubel".to_string()),
        message,
        related_information: if related.is_empty() { None } else { Some(related) },
        ..Default::default()
    }
}

/// Convert every diagnostic for one document.
pub fn to_diagnostics(diags: &[Diagnostic], source: &str, uri: &Url) -> Vec<LspDiagnostic> {
    diags.iter().map(|d| to_diagnostic(d, source, uri)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri() -> Url {
        Url::parse("file:///t.ubl").unwrap()
    }

    #[test]
    fn positions_are_zero_based_across_lines() {
        let src = "ab\ncde\nf";
        assert_eq!(position_at(src, 0), Position::new(0, 0));
        assert_eq!(position_at(src, 2), Position::new(0, 2));
        assert_eq!(position_at(src, 3), Position::new(1, 0));
        assert_eq!(position_at(src, 5), Position::new(1, 2));
        assert_eq!(position_at(src, 7), Position::new(2, 0));
    }

    #[test]
    fn columns_count_utf16_units_not_bytes() {
        // "é" is 2 bytes and 1 UTF-16 unit; "😀" is 4 bytes and 2 units.
        let src = "é😀x";
        assert_eq!(position_at(src, 2), Position::new(0, 1));
        assert_eq!(position_at(src, 6), Position::new(0, 3));
        assert_eq!(position_at(src, 7), Position::new(0, 4));
    }

    #[test]
    fn an_offset_inside_a_character_moves_back_to_its_start() {
        assert_eq!(position_at("é", 1), Position::new(0, 0));
    }

    #[test]
    fn an_offset_past_the_end_is_clamped() {
        assert_eq!(position_at("ab", 99), Position::new(0, 2));
    }

    #[test]
    fn a_span_that_does_not_fit_falls_back_to_line_and_column() {
        let span = Span { start: 500, end: 510, line: 3, column: 4 };
        let r = range_for(&span, "short");
        assert_eq!(r.start, Position::new(2, 3));
        assert_eq!(r.end, Position::new(2, 4));
    }

    #[test]
    fn a_real_diagnostic_converts_with_code_and_source() {
        let src = "fn main() void { let x: int = \"a\" }\n";
        let diags = ubel_stratum_rd::check_source(src).diagnostics;
        let lsp = to_diagnostics(&diags, src, &uri());
        assert_eq!(lsp.len(), 1);
        assert_eq!(lsp[0].code, Some(NumberOrString::String("TYPE-101".into())));
        assert_eq!(lsp[0].source.as_deref(), Some("ubel"));
        assert_eq!(lsp[0].range.start.line, 0);
        assert_eq!(lsp[0].severity, Some(DiagnosticSeverity::ERROR));
    }
}
