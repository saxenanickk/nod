//! Minimal tree-sitter syntax highlighting for individual diff lines.
//!
//! UI-free: returns semantic [`HighlightSpan`]s (byte ranges + a coarse kind)
//! that the caller maps to colors. Lines are highlighted in isolation, which
//! is imperfect for constructs that span lines (block comments, multi-line
//! strings) but robust and fast for a diff viewer.

use std::sync::OnceLock;
use tree_sitter::Language;
use tree_sitter_highlight::{HighlightConfiguration, Highlighter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Json,
}

/// A coarse highlight category. Kept small and editor-agnostic; the UI maps
/// each to a theme color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    Keyword,
    Function,
    Type,
    String,
    Number,
    Comment,
    Constant,
    Operator,
    Punctuation,
    Property,
    Variable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub kind: Highlight,
}

/// Picks a language from a file path's extension.
pub fn lang_for_path(path: &str) -> Option<Lang> {
    let ext = path.rsplit('.').next()?;
    Some(match ext {
        "rs" => Lang::Rust,
        "ts" | "mts" | "cts" => Lang::TypeScript,
        "tsx" => Lang::Tsx,
        "js" | "jsx" | "mjs" | "cjs" => Lang::JavaScript,
        "py" | "pyi" => Lang::Python,
        "go" => Lang::Go,
        "json" => Lang::Json,
        _ => return None,
    })
}

/// The capture names we recognize, in priority order. A capture like
/// `function.method` matches the `function` prefix.
const RECOGNIZED: &[(&str, Highlight)] = &[
    ("keyword", Highlight::Keyword),
    ("function", Highlight::Function),
    ("method", Highlight::Function),
    ("type", Highlight::Type),
    ("constructor", Highlight::Type),
    ("string", Highlight::String),
    ("number", Highlight::Number),
    ("comment", Highlight::Comment),
    ("constant", Highlight::Constant),
    ("operator", Highlight::Operator),
    ("punctuation", Highlight::Punctuation),
    ("property", Highlight::Property),
    ("variable", Highlight::Variable),
];

fn highlight_names() -> Vec<String> {
    RECOGNIZED.iter().map(|(name, _)| name.to_string()).collect()
}

fn config_for(lang: Lang) -> Option<&'static HighlightConfiguration> {
    macro_rules! lazy_config {
        ($cell:ident, $language:expr, $highlights:expr, $name:expr) => {{
            static $cell: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
            $cell
                .get_or_init(|| {
                    let language: Language = $language;
                    let mut cfg =
                        HighlightConfiguration::new(language, $name, $highlights, "", "").ok()?;
                    cfg.configure(&highlight_names());
                    Some(cfg)
                })
                .as_ref()
        }};
    }

    match lang {
        Lang::Rust => lazy_config!(
            RUST,
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            "rust"
        ),
        Lang::TypeScript => lazy_config!(
            TS,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            "typescript"
        ),
        Lang::Tsx => lazy_config!(
            TSX,
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
            "tsx"
        ),
        Lang::JavaScript => lazy_config!(
            JS,
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            "javascript"
        ),
        Lang::Python => lazy_config!(
            PY,
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "python"
        ),
        Lang::Go => lazy_config!(
            GO,
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY,
            "go"
        ),
        Lang::Json => lazy_config!(
            JSON,
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
            "json"
        ),
    }
}

/// Highlights a single line. Returns non-overlapping spans in source order;
/// gaps between spans are unstyled. Empty on any parse failure.
pub fn highlight_line(lang: Lang, line: &str) -> Vec<HighlightSpan> {
    if line.trim().is_empty() {
        return Vec::new();
    }
    let Some(config) = config_for(lang) else {
        return Vec::new();
    };
    let mut highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(config, line.as_bytes(), None, |_| None) else {
        return Vec::new();
    };

    let mut spans = Vec::new();
    let mut stack: Vec<Highlight> = Vec::new();
    for event in events.flatten() {
        use tree_sitter_highlight::HighlightEvent as E;
        match event {
            E::HighlightStart(h) => {
                stack.push(RECOGNIZED[h.0].1);
            }
            E::HighlightEnd => {
                stack.pop();
            }
            E::Source { start, end } => {
                if let Some(&kind) = stack.last() {
                    if end > start {
                        spans.push(HighlightSpan { start, end, kind });
                    }
                }
            }
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_detection() {
        assert_eq!(lang_for_path("src/a.rs"), Some(Lang::Rust));
        assert_eq!(lang_for_path("app/x.tsx"), Some(Lang::Tsx));
        assert_eq!(lang_for_path("a.unknownext"), None);
    }

    #[test]
    fn rust_line_highlights_keyword_and_string() {
        let spans = highlight_line(Lang::Rust, r#"let name = "hello";"#);
        assert!(!spans.is_empty(), "expected some highlights");
        // `let` is a keyword.
        assert!(
            spans.iter().any(|s| s.kind == Highlight::Keyword),
            "expected a keyword span: {spans:?}"
        );
        // The string literal is captured.
        assert!(
            spans.iter().any(|s| s.kind == Highlight::String),
            "expected a string span: {spans:?}"
        );
        // Spans are within bounds and ordered.
        for s in &spans {
            assert!(s.end <= r#"let name = "hello";"#.len());
            assert!(s.start < s.end);
        }
    }

    #[test]
    fn blank_line_is_empty() {
        assert!(highlight_line(Lang::Rust, "   ").is_empty());
    }

    #[test]
    fn typescript_and_python_parse() {
        assert!(!highlight_line(Lang::TypeScript, "const x: number = 1;").is_empty());
        assert!(!highlight_line(Lang::Python, "def f(): return 1").is_empty());
    }
}
