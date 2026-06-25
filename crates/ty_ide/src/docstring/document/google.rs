use indexmap::IndexMap;
use ruff_python_stdlib::identifiers::is_identifier;
use ruff_text_size::TextRange;

use super::SectionKind;
use super::preformatted::PreformattedBlockScanner;
use super::syntax::{
    ParsedLine, indentation, parse_parenthesized_type, parsed_lines, split_once_unbracketed_colon,
};

/// Returns parameter documentation from recognized Google-style parameter sections.
pub(super) fn parameter_documentation(raw: &str) -> IndexMap<String, String> {
    let mut parameters = IndexMap::new();

    visit_sections(raw, |kind, _, body| {
        if matches!(
            kind,
            SectionKind::Parameters | SectionKind::KeywordArguments
        ) {
            extend_parameter_documentation(&mut parameters, body);
        }
    });

    parameters
}

/// Visits recognized Google-style sections in source order.
pub(in crate::docstring) fn visit_sections<'a>(
    raw: &'a str,
    mut visit: impl FnMut(SectionKind, TextRange, &[ParsedLine<'a>]),
) {
    let lines = parsed_lines(raw);
    let mut preformatted_blocks = PreformattedBlockScanner::default();
    let mut index = 0;

    while index < lines.len() {
        if preformatted_blocks.consume_preformatted_line(lines[index].text) {
            index += 1;
            continue;
        }

        let Some(header) = parse_google_section_like_header(&lines, index) else {
            preformatted_blocks.observe_line_outside_preformatted_block(lines[index].text);
            index += 1;
            continue;
        };
        let (body_end, range) = google_section_body_end(&lines, header);
        if let GoogleSectionHeaderKind::Supported(kind) = header.kind {
            visit(kind, range, &lines[header.body_start..body_end]);
        }
        index = body_end;
    }
}

/// Returns whether `name` is a valid Python parameter name, including variadic prefixes.
pub(in crate::docstring) fn is_parameter_name(name: &str) -> bool {
    let identifier = name
        .strip_prefix("**")
        .or_else(|| name.strip_prefix('*'))
        .unwrap_or(name);

    is_identifier(identifier)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GoogleSectionHeaderKind {
    Supported(SectionKind),
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GoogleSectionHeader {
    kind: GoogleSectionHeaderKind,
    indent: usize,
    body_start: usize,
    range: TextRange,
    underlined: bool,
}

fn google_section_body_end(
    lines: &[ParsedLine<'_>],
    header: GoogleSectionHeader,
) -> (usize, TextRange) {
    let mut body_end = header.body_start;
    let mut range = header.range;
    let mut body_preformatted_blocks = PreformattedBlockScanner::default();
    let mut parameter_item_indent = None;
    let mut aligned_unsupported_body = false;

    while let Some(line) = lines.get(body_end) {
        if body_preformatted_blocks.is_active()
            && body_preformatted_blocks.consume_preformatted_line(line.text)
        {
            range = TextRange::new(range.start(), line.range.end());
            body_end += 1;
            continue;
        }

        if line.text.trim().is_empty() {
            if !google_blank_line_continues_section(
                &lines[body_end..],
                header,
                parameter_item_indent,
                aligned_unsupported_body,
            ) {
                break;
            }

            while let Some(blank_line) = lines.get(body_end)
                && blank_line.text.trim().is_empty()
            {
                range = TextRange::new(range.start(), blank_line.range.end());
                body_end += 1;
            }
            continue;
        }

        let can_start_aligned_unsupported_body = body_end == header.body_start;
        if google_section_header_ends_body(
            lines,
            body_end,
            header,
            parameter_item_indent,
            aligned_unsupported_body || can_start_aligned_unsupported_body,
        ) {
            break;
        }

        if !google_line_belongs_to_body(
            header,
            line.text,
            parameter_item_indent,
            aligned_unsupported_body || can_start_aligned_unsupported_body,
        ) {
            break;
        }

        aligned_unsupported_body |= is_aligned_unsupported_section_body(header, line.text, true);

        parameter_item_indent =
            parameter_item_indent.or_else(|| google_parameter_item_indent(header, line.text));

        if !body_preformatted_blocks.consume_preformatted_line(line.text) {
            body_preformatted_blocks.observe_line_outside_preformatted_block(line.text);
        }
        range = TextRange::new(range.start(), line.range.end());
        body_end += 1;
    }

    (body_end, range)
}

fn google_blank_line_continues_section(
    lines: &[ParsedLine<'_>],
    header: GoogleSectionHeader,
    parameter_item_indent: Option<usize>,
    aligned_unsupported_body: bool,
) -> bool {
    let Some((offset, non_blank_line)) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| !line.text.trim().is_empty())
    else {
        return false;
    };

    let next_indent = indentation(non_blank_line.text);
    if next_indent <= header.indent
        && (parse_google_section_like_header(lines, offset).is_some()
            || is_inline_google_section_header(non_blank_line.text))
    {
        return false;
    }

    // A blank line separates prose aligned with the parameter items from the section body.
    if parameter_item_indent == Some(next_indent)
        && google_parameter_item_indent(header, non_blank_line.text).is_none()
    {
        return false;
    }

    google_line_belongs_to_body(
        header,
        non_blank_line.text,
        parameter_item_indent,
        aligned_unsupported_body,
    )
}

fn google_section_header_ends_body(
    lines: &[ParsedLine<'_>],
    index: usize,
    header: GoogleSectionHeader,
    parameter_item_indent: Option<usize>,
    aligned_unsupported_body: bool,
) -> bool {
    let Some(line) = lines.get(index) else {
        return false;
    };
    if is_aligned_unsupported_section_body(header, line.text, aligned_unsupported_body) {
        return false;
    }
    if indentation(line.text) <= header.indent && is_inline_google_section_header(line.text) {
        return true;
    }

    let Some(next) = parse_google_section_like_header(lines, index) else {
        return false;
    };

    next.indent <= header.indent
        && (next.underlined
            || !lowercase_same_indent_parameter_takes_precedence(
                header,
                line.text,
                parameter_item_indent,
            ))
}

fn google_line_belongs_to_body(
    header: GoogleSectionHeader,
    line: &str,
    parameter_item_indent: Option<usize>,
    aligned_unsupported_body: bool,
) -> bool {
    let line_indent = indentation(line);
    // PEP 257 can align items and their continuations with a first-line section heading.
    is_aligned_unsupported_section_body(header, line, aligned_unsupported_body)
        || line_indent > header.indent
        || (line_indent == header.indent
            && parameter_item_indent.is_none_or(|indent| indent == line_indent)
            && (parameter_item_indent.is_some()
                || google_parameter_item_indent(header, line).is_some()))
}

fn is_aligned_unsupported_section_body(
    header: GoogleSectionHeader,
    line: &str,
    aligned_unsupported_body: bool,
) -> bool {
    aligned_unsupported_body
        && header.kind == GoogleSectionHeaderKind::Unsupported
        && indentation(line) == header.indent
}

fn lowercase_same_indent_parameter_takes_precedence(
    header: GoogleSectionHeader,
    line: &str,
    parameter_item_indent: Option<usize>,
) -> bool {
    let line_indent = indentation(line);
    line_indent == header.indent
        && parameter_item_indent.is_none_or(|indent| indent == line_indent)
        && line.trim().chars().next().is_some_and(char::is_lowercase)
        && google_parameter_item_indent(header, line).is_some()
}

fn google_parameter_item_indent(header: GoogleSectionHeader, line: &str) -> Option<usize> {
    if matches!(
        header.kind,
        GoogleSectionHeaderKind::Supported(SectionKind::Parameters | SectionKind::KeywordArguments)
    ) && parse_google_parameter(line.trim()).is_some()
    {
        Some(indentation(line))
    } else {
        None
    }
}

fn is_google_section_underline(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.chars().all(|character| matches!(character, '-' | '='))
}

fn parse_google_section_like_header(
    lines: &[ParsedLine<'_>],
    index: usize,
) -> Option<GoogleSectionHeader> {
    let line = lines.get(index)?;
    let kind = google_section_kind(line.text)?;
    let underline = lines
        .get(index + 1)
        .filter(|line| is_google_section_underline(line.text));

    Some(GoogleSectionHeader {
        kind,
        indent: indentation(line.text),
        body_start: index + 1 + usize::from(underline.is_some()),
        range: underline.map_or(line.range, |underline| {
            TextRange::new(line.range.start(), underline.range.end())
        }),
        underlined: underline.is_some(),
    })
}

fn google_section_kind(line: &str) -> Option<GoogleSectionHeaderKind> {
    let name = line.trim().strip_suffix(':')?.trim();
    google_section_kind_from_name(name)
}

fn google_section_kind_from_name(name: &str) -> Option<GoogleSectionHeaderKind> {
    let normalized = name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let kind = match normalized.as_str() {
        "args" | "arguments" | "parameters" => {
            GoogleSectionHeaderKind::Supported(SectionKind::Parameters)
        }
        "keyword args" | "keyword arguments" => {
            GoogleSectionHeaderKind::Supported(SectionKind::KeywordArguments)
        }
        "attributes" => GoogleSectionHeaderKind::Supported(SectionKind::Attributes),
        "return" | "returns" => GoogleSectionHeaderKind::Supported(SectionKind::Returns),
        "yield" | "yields" => GoogleSectionHeaderKind::Supported(SectionKind::Yields),
        "raise" | "raises" => GoogleSectionHeaderKind::Supported(SectionKind::Raises),
        "attention" | "caution" | "danger" | "error" | "example" | "examples" | "hint"
        | "important" | "methods" | "note" | "notes" | "other args" | "other arguments"
        | "other parameters" | "references" | "see also" | "tip" | "todo" | "todos" | "warning"
        | "warnings" | "warns" => GoogleSectionHeaderKind::Unsupported,
        _ => return None,
    };
    Some(kind)
}

fn is_inline_google_section_header(line: &str) -> bool {
    let Some((name, description)) = split_once_unbracketed_colon(line.trim()) else {
        return false;
    };
    let name = name.trim();

    !description.trim().is_empty()
        && name.chars().next().is_some_and(char::is_uppercase)
        && google_section_kind_from_name(name).is_some()
}

fn parse_google_parameter(line: &str) -> Option<(&str, &str)> {
    let (name, description) = split_once_unbracketed_colon(line)?;
    let name = name.trim();
    let (display_name, _) = parse_parenthesized_type(name);
    google_parameter_names(display_name).next()?;

    Some((display_name, description.trim()))
}

fn extend_parameter_documentation(parameters: &mut IndexMap<String, String>, lines: &[ParsedLine]) {
    let mut current: Option<(String, String)> = None;
    let mut item_indent = None;

    for line in lines {
        let line = line.text;
        let trimmed = line.trim();
        let line_indent = indentation(line);

        if trimmed.is_empty() {
            if let Some(current) = &mut current {
                if !current.1.is_empty() && !current.1.ends_with('\n') {
                    current.1.push('\n');
                }
                current.1.push('\n');
            }
            continue;
        }

        if item_indent.is_none_or(|indent| line_indent == indent)
            && let Some((names, description)) = parse_google_parameter(trimmed)
        {
            insert_parameter_documentation(
                parameters,
                current.replace((names.to_string(), description.to_string())),
            );
            item_indent.get_or_insert(line_indent);
            continue;
        }

        if let Some(current) = &mut current {
            if !current.1.is_empty() && !current.1.ends_with('\n') {
                current.1.push('\n');
            }
            current.1.push_str(trimmed);
        }
    }

    insert_parameter_documentation(parameters, current);
}

fn insert_parameter_documentation(
    parameters: &mut IndexMap<String, String>,
    parameter: Option<(String, String)>,
) {
    let Some((names, description)) = parameter else {
        return;
    };
    let description = description.trim();
    if description.is_empty() {
        return;
    }
    for name in google_parameter_names(&names) {
        parameters.insert(name.to_string(), description.to_string());
    }
}

fn google_parameter_names(display_name: &str) -> impl Iterator<Item = &str> {
    display_name
        .split(',')
        .map(str::trim)
        .filter(|name| is_parameter_name(name))
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::{SectionKind, parameter_documentation, visit_sections};

    #[test]
    fn parameter_documentation_preserves_consecutive_blank_lines() {
        let parameters = parameter_documentation(
            "\
Args:
    value: First paragraph.


        Second paragraph.",
        );

        assert_eq!(
            parameters["value"],
            "First paragraph.\n\n\nSecond paragraph."
        );
    }

    #[test]
    fn parameter_documentation_accepts_same_indent_items() {
        let parameters = parameter_documentation(
            "\
Arguments:
first: First parameter.
Aligned continuation.
second: Second parameter.
Returns:
bool: Result.",
        );

        assert_eq!(parameters.len(), 2);
        assert_eq!(
            parameters["first"],
            "First parameter.\nAligned continuation."
        );
        assert_eq!(parameters["second"], "Second parameter.");
    }

    #[test]
    fn parameter_documentation_accepts_grouped_items() {
        let parameters = parameter_documentation(
            "\
Args:
    x, y: Coordinates.",
        );

        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters["x"], "Coordinates.");
        assert_eq!(parameters["y"], "Coordinates.");
    }

    #[test]
    fn parameter_documentation_prefers_last_duplicate() {
        let parameters = parameter_documentation(
            "\
Args:
    value: First documentation.
    value: Replacement documentation.",
        );

        assert_eq!(parameters["value"], "Replacement documentation.");
    }

    #[test]
    fn parameter_documentation_accepts_parentheses_in_quoted_types() {
        let parameters = parameter_documentation(
            r#"Args:
    value (Literal["("]): Parameter with a quoted parenthesis."#,
        );

        assert_eq!(parameters["value"], "Parameter with a quoted parenthesis.");
    }

    #[test]
    fn parameter_documentation_consumes_recognized_section_bodies() {
        let parameters = parameter_documentation(
            "\
Args:
    value: Parameter documentation.
Methods:
    helper: Method documentation.",
        );

        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters["value"], "Parameter documentation.");

        let parameters = parameter_documentation(
            "\
Example:
    Args:
        nested: Not parameter documentation.
    Returns:
        str: Example result.
Args:
    value: Parameter documentation.",
        );

        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters["value"], "Parameter documentation.");

        let parameters = parameter_documentation(
            "\
Example:
Args:
    nested: Not parameter documentation.
Returns:
    str: Example result.",
        );

        assert!(parameters.is_empty());
    }

    #[test]
    fn parameter_documentation_stops_at_inline_section_summary() {
        let parameters = parameter_documentation(
            "\
Args:
    first: First parameter.
    last: Last parameter.

Returns: Result.",
        );

        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters["last"], "Last parameter.");
    }

    #[test]
    fn parameter_documentation_ignores_sections_in_preformatted_blocks() {
        for raw in [
            "\
Summary.

    ```text
    Args:
        nested: Not parameter documentation.
    ```

    Args:
        value: Parameter documentation.",
            "\
Summary.

    >>> example()
    Args:
        nested: Not parameter documentation.

    Args:
        value: Parameter documentation.",
            "\
Summary.

    Example::

        Args:
            nested: Not parameter documentation.

    Args:
        value: Parameter documentation.",
        ] {
            let parameters = super::super::parameter_documentation(raw, IndexMap::default());

            assert_eq!(parameters.len(), 1, "{raw}");
            assert_eq!(
                parameters.get("value").map(String::as_str),
                Some("Parameter documentation."),
                "{raw}"
            );
        }
    }

    #[test]
    fn parameter_documentation_stops_at_same_indent_inline_section_summary() {
        let parameters = parameter_documentation(
            "\
Args:
first: First parameter.
last: Last parameter.
Returns: Result.",
        );

        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters["last"], "Last parameter.");
    }

    #[test]
    fn parameter_documentation_accepts_underlined_section() {
        let parameters = parameter_documentation(
            "\
Summary.

Args:
----
    value: Parameter documentation.

Returns:
    bool: Result.",
        );

        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters["value"], "Parameter documentation.");
    }

    #[test]
    fn parameter_documentation_prefers_lowercase_same_indent_parameter() {
        let parameters = parameter_documentation(
            "\
Args:
error:
    Error documentation.
args: Args parameter documentation.
code: Code documentation.
returns: Return parameter documentation.
Returns:
bool: Result.",
        );

        assert_eq!(parameters.len(), 4);
        assert_eq!(parameters["error"], "Error documentation.");
        assert_eq!(parameters["args"], "Args parameter documentation.");
        assert_eq!(parameters["code"], "Code documentation.");
        assert_eq!(parameters["returns"], "Return parameter documentation.");
    }

    #[test]
    fn section_visiting_preserves_underlined_lowercase_header() {
        let mut returns_body = None;
        visit_sections(
            "\
Args:
value: Parameter documentation.
returns:
--------
    bool: Result.",
            |kind, _, body| {
                if kind == SectionKind::Returns {
                    returns_body = Some(
                        body.iter()
                            .map(|line| line.text.to_string())
                            .collect::<Vec<_>>(),
                    );
                }
            },
        );

        assert_eq!(returns_body, Some(vec!["    bool: Result.".to_string()]));
    }

    #[test]
    fn parameter_documentation_stops_at_blank_separated_content() {
        let parameters = parameter_documentation(
            "\
Args:
value: Parameter documentation.

returns:
    bool: Result.",
        );

        assert_eq!(parameters.len(), 1);
        assert_eq!(parameters["value"], "Parameter documentation.");

        let parameters = parameter_documentation(
            "\
Args:
value: Parameter documentation.

Additional details about the function.",
        );

        assert_eq!(parameters["value"], "Parameter documentation.");

        let parameters = parameter_documentation(
            "\
Args:
    value: Parameter documentation.

    Additional details about the function.",
        );

        assert_eq!(parameters["value"], "Parameter documentation.");
    }
}
