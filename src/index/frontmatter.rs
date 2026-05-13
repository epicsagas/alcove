// ---------------------------------------------------------------------------
// Frontmatter parsing — skip draft / deprecated documents from indexing
// ---------------------------------------------------------------------------

/// Flags extracted from a document's YAML front matter.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FrontmatterFlags {
    /// `true` when the document should be excluded from the search index.
    pub(crate) should_skip: bool,
}

/// Parse the YAML front matter of `content` and return indexing flags.
///
/// A valid front matter block **must** start on line 0 with exactly `---`
/// (no leading whitespace) and be closed by a subsequent `---` or `...` line.
/// If the opening delimiter is not on line 0, or the block is never closed,
/// the function returns the safe default (`should_skip = false`).
///
/// Recognised skip conditions (case-insensitive value matching):
/// * `draft: true | yes | 1 | TRUE | YES`
/// * `status: deprecated | DEPRECATED`
///
/// Any YAML parse ambiguity is treated as "no skip" (safe default).
pub(crate) fn parse_frontmatter_flags(content: &str) -> FrontmatterFlags {
    let mut lines = content.lines();

    // Line 0 must be exactly `---`
    match lines.next() {
        Some(first) if first.trim_end() == "---" => {}
        _ => return FrontmatterFlags::default(),
    }

    // Collect front-matter lines until the closing `---` or `...`
    let mut fm_lines: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            closed = true;
            break;
        }
        fm_lines.push(line);
    }

    if !closed {
        return FrontmatterFlags::default();
    }

    let mut should_skip = false;

    for line in &fm_lines {
        // Split on first `:` only — values may contain colons themselves.
        let Some(colon_pos) = line.find(':') else {
            continue;
        };
        let key = line[..colon_pos].trim().to_ascii_lowercase();
        let raw_value = line[colon_pos + 1..].trim();

        // Strip optional surrounding quotes (' or ")
        let value = raw_value
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_ascii_lowercase();

        match key.as_str() {
            "draft" => {
                if matches!(value.as_str(), "true" | "yes" | "1") {
                    should_skip = true;
                }
            }
            "status" => {
                if value == "deprecated" {
                    should_skip = true;
                }
            }
            _ => {}
        }
    }

    FrontmatterFlags { should_skip }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_true_should_skip() {
        let content = "---\ndraft: true\n---\n# My Doc\n\nBody text.";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn draft_yes_should_skip() {
        let content = "---\ndraft: yes\n---\n# My Doc";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn draft_1_should_skip() {
        let content = "---\ndraft: 1\n---\n# My Doc";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn draft_true_uppercase_should_skip() {
        let content = "---\ndraft: TRUE\n---\n# My Doc";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn draft_yes_uppercase_should_skip() {
        let content = "---\ndraft: YES\n---\n# My Doc";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn draft_false_should_not_skip() {
        let content = "---\ndraft: false\n---\n# My Doc\n\nBody text.";
        assert!(!parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn no_frontmatter_should_not_skip() {
        let content = "# My Doc\n\nBody text without front matter.";
        assert!(!parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn status_deprecated_should_skip() {
        let content = "---\nstatus: deprecated\n---\n# Old Doc";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn status_deprecated_uppercase_should_skip() {
        let content = "---\nstatus: DEPRECATED\n---\n# Old Doc";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn status_active_should_not_skip() {
        let content = "---\nstatus: active\n---\n# Active Doc";
        assert!(!parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn malformed_yaml_with_delimiter_should_not_skip() {
        // Invalid YAML but valid `---` delimiters — safe default
        let content = "---\n: broken: yaml: line\ndraft true\n---\n# Body";
        assert!(!parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn unclosed_frontmatter_should_not_skip() {
        let content = "---\ndraft: true\n# No closing delimiter";
        assert!(!parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn frontmatter_not_on_line_zero_should_not_skip() {
        // Leading blank line means `---` is NOT on line 0
        let content = "\n---\ndraft: true\n---\n# Body";
        assert!(!parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn code_block_dash_separator_inside_body_should_not_skip() {
        // The `---` inside the body (after the front matter closes) must not
        // trigger a second parse. This also tests that a valid front matter
        // without skip flags returns false.
        let content = "---\ntitle: My Doc\n---\n# Body\n\n---\ndraft: true\n---";
        assert!(!parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn dotdotdot_close_delimiter_works() {
        let content = "---\ndraft: true\n...\n# Body";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn quoted_value_should_skip() {
        let content = "---\ndraft: \"true\"\n---\n# Body";
        assert!(parse_frontmatter_flags(content).should_skip);
    }

    #[test]
    fn single_quoted_deprecated_should_skip() {
        let content = "---\nstatus: 'deprecated'\n---\n# Body";
        assert!(parse_frontmatter_flags(content).should_skip);
    }
}
