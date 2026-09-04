//! Private, on-device writing assistance for the prompt composer.

use harper_core::{
    Dialect, Document,
    linting::{LintGroup, LintKind, Linter, Suggestion},
    spell::FstDictionary,
};
use std::cell::RefCell;

const MAX_ISSUES: usize = 16;

thread_local! {
    static LINTER: RefCell<LintGroup> = RefCell::new(LintGroup::new_curated(
        FstDictionary::curated(),
        Dialect::American,
    ));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WritingIssue {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: LintKind,
    pub(crate) message: String,
    pub(crate) suggestions: Vec<WritingSuggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WritingSuggestion {
    Replace(String),
    InsertAfter(String),
    Remove,
}

impl WritingSuggestion {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Replace(value) => format!("Replace with “{value}”"),
            Self::InsertAfter(value) => format!("Insert “{value}”"),
            Self::Remove => "Remove".into(),
        }
    }
}

impl From<&Suggestion> for WritingSuggestion {
    fn from(suggestion: &Suggestion) -> Self {
        match suggestion {
            Suggestion::ReplaceWith(chars) => Self::Replace(chars.iter().collect()),
            Suggestion::InsertAfter(chars) => Self::InsertAfter(chars.iter().collect()),
            Suggestion::Remove => Self::Remove,
        }
    }
}

/// Lint Markdown-flavoured chat input. Harper masks fenced and inline code,
/// while this final filter avoids presenting identifiers and paths as prose.
pub(crate) fn check(source: &str) -> Vec<WritingIssue> {
    if source.trim().is_empty() {
        return Vec::new();
    }
    let chars = source.chars().collect::<Vec<_>>();
    let document = Document::new_markdown_default_curated(source);
    let mut lints = LINTER.with(|linter| linter.borrow_mut().lint(&document));
    lints.sort_by_key(|lint| (lint.span.start, lint.priority));

    let mut issues = Vec::new();
    let mut last_end = 0;
    for lint in lints {
        if lint.span.start < last_end || lint.span.end > chars.len() {
            continue;
        }
        let offending = chars[lint.span.start..lint.span.end]
            .iter()
            .collect::<String>();
        if looks_technical(&offending) {
            continue;
        }
        last_end = lint.span.end;
        issues.push(WritingIssue {
            start: lint.span.start,
            end: lint.span.end,
            kind: lint.lint_kind,
            message: lint.message,
            suggestions: lint.suggestions.iter().take(3).map(Into::into).collect(),
        });
        if issues.len() == MAX_ISSUES {
            break;
        }
    }
    issues
}

fn looks_technical(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() <= 1 {
        return true;
    }
    trimmed.contains(['/', '\\', '_', '@', ':'])
        || trimmed.chars().any(|character| character.is_ascii_digit())
        || (trimmed.chars().any(char::is_uppercase)
            && trimmed.chars().skip(1).any(char::is_uppercase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_spelling_and_grammar_with_suggestions() {
        let issues = check("I could of done that. Fix the sentnce.");
        assert!(issues.iter().any(|issue| issue.kind == LintKind::Spelling));
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == LintKind::WordChoice)
        );
        assert!(issues.iter().any(|issue| !issue.suggestions.is_empty()));
    }

    #[test]
    fn ignores_code_and_common_identifier_shapes() {
        let source = "Run `cargo chek` for foo_bar at /tmp/vkore2. Fix the sentnce.";
        let chars = source.chars().collect::<Vec<_>>();
        let offending = check(source)
            .into_iter()
            .map(|issue| chars[issue.start..issue.end].iter().collect::<String>())
            .collect::<Vec<_>>();
        assert!(offending.iter().any(|text| text == "sentnce"));
        assert!(offending.iter().all(|text| !text.contains("chek")));
    }
}
