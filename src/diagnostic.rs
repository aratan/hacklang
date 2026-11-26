//! Shared, machine-readable diagnostics for compiler frontend and analysis phases.

use crate::lexer::SourceSpan;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPhase {
    Lexing,
    Parsing,
    TypeChecking,
}

impl DiagnosticPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexing => "lexing",
            Self::Parsing => "parsing",
            Self::TypeChecking => "type_check",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

impl DiagnosticSeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Stable machine-readable identifier.
    pub code: &'static str,
    pub phase: DiagnosticPhase,
    pub severity: DiagnosticSeverity,
    pub message: String,
    /// Character-based source range, absent when no source location is available.
    pub range: Option<SourceSpan>,
}

impl Diagnostic {
    pub fn new(
        code: &'static str,
        phase: DiagnosticPhase,
        severity: DiagnosticSeverity,
        message: impl Into<String>,
        range: Option<SourceSpan>,
    ) -> Self {
        Self { code, phase, severity, message: message.into(), range }
    }

    pub fn to_json(&self) -> String {
        let range = self.range.map_or_else(
            || "null".to_string(),
            |span| format!(
                "{{\\"start\\":{},\\"end\\":{},\\"line\\":{},\\"column\\":{},\\"endLine\\":{},\\"endColumn\\":{}}}",
                span.start, span.end, span.line, span.column, span.end_line, span.end_column
            ),
        );
        format!(
            "{{\\"code\\":\\"{}\\",\\"phase\\":\\"{}\\",\\"severity\\":\\"{}\\",\\"message\\":\\"{}\\",\\"range\\":{}}}",
            self.code,
            self.phase.as_str(),
            self.severity.as_str(),
            json_escape(&self.message),
            range
        )
    }
}

pub fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            ch if ch <= '\\u{1f}' => escaped.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => escaped.push(ch),
        }
    }
    escaped
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_json_includes_phase_severity_and_optional_range() {
        let diagnostic = Diagnostic::new(
            "HL-LEX-001",
            DiagnosticPhase::Lexing,
            DiagnosticSeverity::Error,
            "unexpected character",
            None,
        );
        let json = diagnostic.to_json();
        assert!(json.contains("\\"code\\":\\"HL-LEX-001\\""));
        assert!(json.contains("\\"phase\\":\\"lexing\\""));
        assert!(json.contains("\\"severity\\":\\"error\\""));
        assert!(json.contains("\\"range\\":null"));
    }

    #[test]
    fn diagnostic_json_escapes_control_characters() {
        assert_eq!(json_escape("quote: \\" slash: \\\\ newline:\n tab:\t"), r#"quote: \\" slash: \\\\ newline:\\n tab:\\t"#);
    }
}
