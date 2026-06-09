/// A prose line and the Markdown prefix that precedes it.
#[derive(Clone, Copy)]
pub(super) struct Line<'a> {
    /// Prefix to emit directly into the rendered Markdown before `text`.
    pub(super) rendered_prefix: &'a str,
    /// Number of leading spaces in the source line.
    pub(super) source_indentation: usize,
    /// The line's text after removing leading indentation.
    pub(super) text: &'a str,
}

/// Renders the supported subset of explicit reST hyperlinks in prose.
///
/// This contract was selected from a point-in-time review of 430 embedded HTTP(S) links in
/// docstrings across 16 popular Python repositories. The one-, two-, and three-line shapes
/// below covered 392 occurrences (91.2%); after applying conservative syntax and ambiguity
/// guards, a static replay predicted 389 conversions (90.5%). The goal is therefore roughly
/// 90% coverage of embedded external links without growing this into a general reST parser.
/// This percentage does not include separate link families such as named references or
/// relative targets.
///
/// A supported hyperlink:
///
/// * follows reST's inline-markup boundary rules and contains no backslash escapes;
/// * embeds a non-empty absolute HTTP or HTTPS URI containing no whitespace, control
///   characters, backslashes, angle brackets, or square brackets;
/// * is labeled or uses its URI as the label;
/// * ends in either `_` or `__`;
/// * appears on one line, or spans two or three lines without decreasing the absolute number of
///   leading source spaces and with the URI target starting the final line; and
/// * for a three-line link, uses a non-empty middle label line that contains no backticks or
///   angle brackets and does not resemble an obvious reST or Markdown block start.
///
/// Multiple supported hyperlinks can appear in the same prose fragment.
///
/// # Context recognition
///
/// Recognized single-line inline code spans remain unchanged. The surrounding renderer decides
/// which source lines are prose using its existing block scanner. This renderer does not determine
/// surrounding block membership; in particular, [`Line::source_indentation`] is only the absolute
/// number of leading spaces and does not account for an enclosing list or citation marker. It only
/// rejects obvious block starts while validating a multiline label continuation.
///
/// # Examples
///
/// Supported forms observed in the corpus include a single-line link, a two-line link with a
/// standalone target, and a three-line link with one label-continuation line:
///
/// ```text
/// `Sesame <https://cds.unistra.fr/cgi-bin/Sesame>`_
///
/// `Schechter 1976
/// <https://example.com/paper>`_
///
/// `Citation author and year,
/// "Citation title."
/// <https://example.com/paper>`_
/// ```
///
/// These render as:
///
/// ```text
/// [Sesame](https://cds.unistra.fr/cgi-bin/Sesame)
///
/// [Schechter 1976](https://example.com/paper)
///
/// [Citation author and year, "Citation title."](https://example.com/paper)
/// ```
///
/// ## Unsupported corpus examples
///
/// This Matplotlib form is preserved because label text and the target share the final line:
///
/// ```text
/// `a BCP47
/// language code <https://www.w3.org/International/articles/language-tags/>`_
/// ```
///
/// Recognizing its target would require resuming the label parser partway through a
/// continuation line, rebuilding a more general multiline candidate parser for 30 of the 430
/// occurrences (7.0%).
///
/// This relative target from pandas is also preserved:
///
/// ```text
/// `Table Visualization <../../user_guide/style.ipynb>`_
/// ```
///
/// An IDE-rendered docstring has no reliable Sphinx document base, so turning it into a
/// Markdown link could resolve to an unintended location.
///
/// Only the hyperlink forms and prose contexts described above are intentionally supported.
/// Literal fallback is guaranteed for candidates rejected by this bounded inline grammar, not for
/// text whose enclosing Markdown or reST context the surrounding renderer does not recognize.
/// Other hyperlink forms and block-sensitive interpretations are outside the current contract.
/// Expanding the supported subset is a contract change and requires corresponding tests.
#[derive(Default)]
pub(super) struct Renderer {
    pending_link: Option<PendingLink>,
}

impl Renderer {
    /// Renders a prose line, buffering a supported wrapped hyperlink when necessary.
    pub(super) fn render_line(&mut self, output: &mut String, line: Line<'_>) {
        if let Some(pending_link) = self.pending_link.take() {
            self.render_pending_line(output, pending_link, line);
        } else {
            self.render_new_line(output, line);
        }
    }

    /// Renders a line without converting hyperlinks.
    pub(super) fn render_line_without_links(&mut self, output: &mut String, line: Line<'_>) {
        self.flush_pending_link(output);
        output.push_str(line.rendered_prefix);
        render_line(output, line.text);
    }

    /// Returns whether a hyperlink begun on the preceding line is still pending.
    pub(super) fn has_pending_link(&self) -> bool {
        self.pending_link.is_some()
    }

    /// Emits a buffered hyperlink candidate without converting it.
    pub(super) fn flush_pending_link(&mut self, output: &mut String) {
        if let Some(pending_link) = self.pending_link.take() {
            output.push_str(&pending_link.fallback);
        }
    }

    fn render_pending_line(
        &mut self,
        output: &mut String,
        mut pending_link: PendingLink,
        mut line: Line<'_>,
    ) {
        if line.source_indentation < pending_link.minimum_indentation {
            output.push_str(&pending_link.fallback);
            self.render_new_line(output, line);
            return;
        }

        if let Some(target) = TargetLine::parse(line.text) {
            output.push_str(&pending_link.rendered_before);
            render_markdown_link(output, Some(&pending_link.label), target.uri);
            line.text = &line.text[target.len..];
            self.render_fragment(output, line, "");
            return;
        }

        if pending_link.can_continue_label && is_label_continuation(line.text) {
            pending_link.push_label_line(line);
            self.pending_link = Some(pending_link);
        } else {
            output.push_str(&pending_link.fallback);
            self.render_new_line(output, line);
        }
    }

    fn render_new_line(&mut self, output: &mut String, line: Line<'_>) {
        self.render_fragment(output, line, line.rendered_prefix);
    }

    fn render_fragment(&mut self, output: &mut String, line: Line<'_>, mut prefix: &str) {
        let mut rest = line.text;

        loop {
            match find_link(rest) {
                Some(InlineLink::Complete { start, link }) => {
                    output.push_str(prefix);
                    prefix = "";
                    render_line(output, &rest[..start]);
                    link.render_markdown(output);
                    rest = &rest[start + link.len..];
                }
                Some(InlineLink::Pending { start, label }) => {
                    self.pending_link = Some(PendingLink::new(
                        prefix,
                        rest,
                        start,
                        label,
                        line.source_indentation,
                    ));
                    return;
                }
                None => {
                    output.push_str(prefix);
                    render_line(output, rest);
                    return;
                }
            }
        }
    }
}

struct PendingLink {
    label: String,
    rendered_before: String,
    fallback: String,
    minimum_indentation: usize,
    can_continue_label: bool,
}

impl PendingLink {
    fn new(
        rendered_prefix: &str,
        line: &str,
        candidate_start: usize,
        label: &str,
        minimum_indentation: usize,
    ) -> Self {
        let mut rendered_before = String::with_capacity(rendered_prefix.len() + candidate_start);
        rendered_before.push_str(rendered_prefix);
        render_line(&mut rendered_before, &line[..candidate_start]);

        let mut fallback = String::with_capacity(rendered_prefix.len() + line.len());
        fallback.push_str(rendered_prefix);
        render_line(&mut fallback, line);

        Self {
            label: label
                .trim_end_matches(|char: char| char.is_ascii_whitespace())
                .to_owned(),
            rendered_before,
            fallback,
            minimum_indentation,
            can_continue_label: true,
        }
    }

    fn push_label_line(&mut self, line: Line<'_>) {
        self.label.push(' ');
        self.label.push_str(
            line.text
                .trim_end_matches(|char: char| char.is_ascii_whitespace()),
        );
        self.fallback.push_str(line.rendered_prefix);
        render_line(&mut self.fallback, line.text);
        self.can_continue_label = false;
    }
}

enum InlineLink<'a> {
    Complete { start: usize, link: Hyperlink<'a> },
    Pending { start: usize, label: &'a str },
}

fn find_link(input: &str) -> Option<InlineLink<'_>> {
    let mut offset = 0;

    while let Some(relative_index) = input[offset..].find('`') {
        let index = offset + relative_index;
        let tick_count = leading_backtick_count(&input[index..]);

        if is_escaped(input, index) {
            offset = index + tick_count;
            continue;
        }

        if tick_count == 1 && is_link_start(input, index) {
            match parse_candidate(&input[index..]) {
                Candidate::Complete(link) => {
                    return Some(InlineLink::Complete { start: index, link });
                }
                Candidate::Pending { label } => {
                    return Some(InlineLink::Pending {
                        start: index,
                        label,
                    });
                }
                Candidate::Rejected => {}
            }
        }

        let after_opening = index + tick_count;
        let closing_end = find_closing_backtick_run(&input[after_opening..], tick_count)?;
        offset = after_opening + closing_end;
    }

    None
}

enum Candidate<'a> {
    Complete(Hyperlink<'a>),
    Pending { label: &'a str },
    Rejected,
}

fn parse_candidate(input: &str) -> Candidate<'_> {
    let Some(after_opening) = input.strip_prefix('`') else {
        return Candidate::Rejected;
    };
    if after_opening
        .chars()
        .next()
        .is_none_or(|char| char == '`' || char.is_whitespace())
    {
        return Candidate::Rejected;
    }

    let Some(relative_closing) = after_opening.find('`') else {
        return if after_opening.contains(['\\', '<', '>']) {
            Candidate::Rejected
        } else {
            Candidate::Pending {
                label: after_opening,
            }
        };
    };
    let closing_index = relative_closing + 1;
    if leading_backtick_count(&input[closing_index..]) != 1 {
        return Candidate::Rejected;
    }

    let content = &input[1..closing_index];
    if content.contains('\\') {
        return Candidate::Rejected;
    }

    let after_closing = &input[closing_index + 1..];
    let underscore_count = after_closing
        .bytes()
        .take_while(|byte| *byte == b'_')
        .count();
    let len = closing_index + 1 + underscore_count;
    if !(1..=2).contains(&underscore_count) || !is_link_suffix(&after_closing[underscore_count..]) {
        return Candidate::Rejected;
    }

    Hyperlink::parse(content, len).map_or(Candidate::Rejected, Candidate::Complete)
}

struct Hyperlink<'a> {
    label: Option<&'a str>,
    uri: &'a str,
    len: usize,
}

impl<'a> Hyperlink<'a> {
    fn parse(content: &'a str, len: usize) -> Option<Self> {
        let target_start = content.rfind('<')?;
        if !content.ends_with('>') {
            return None;
        }

        let uri = &content[target_start + 1..content.len() - 1];
        if !is_supported_uri(uri) {
            return None;
        }

        let label = if target_start == 0 {
            None
        } else {
            let before_target = &content[..target_start];
            if !before_target.ends_with(|char: char| char.is_ascii_whitespace()) {
                return None;
            }
            let label = before_target.trim_end_matches(|char: char| char.is_ascii_whitespace());
            if label.is_empty() {
                return None;
            }
            Some(label)
        };

        Some(Self { label, uri, len })
    }

    fn render_markdown(&self, output: &mut String) {
        render_markdown_link(output, self.label, self.uri);
    }
}

struct TargetLine<'a> {
    uri: &'a str,
    len: usize,
}

impl<'a> TargetLine<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let after_opening = line.strip_prefix('<')?;
        let target_end = after_opening.find('>')? + 1;
        let uri = &after_opening[..target_end - 1];
        if !is_supported_uri(uri) {
            return None;
        }

        let after_target = &after_opening[target_end..];
        let after_backtick = after_target.strip_prefix('`')?;
        let underscore_count = after_backtick
            .bytes()
            .take_while(|byte| *byte == b'_')
            .count();
        if !(1..=2).contains(&underscore_count)
            || !is_link_suffix(&after_backtick[underscore_count..])
        {
            return None;
        }

        Some(Self {
            uri,
            len: 1 + target_end + 1 + underscore_count,
        })
    }
}

fn is_supported_uri(uri: &str) -> bool {
    !uri.is_empty()
        && !uri.chars().any(|char| {
            matches!(char, '\\' | '<' | '>' | '[' | ']')
                || char.is_control()
                || char.is_whitespace()
        })
        && ["http://", "https://"].into_iter().any(|scheme| {
            uri.get(..scheme.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(scheme))
        })
}

fn is_label_continuation(line: &str) -> bool {
    let line = line.trim_end_matches(|char: char| char.is_ascii_whitespace());
    !line.is_empty() && !line.contains(['`', '\\', '<', '>']) && !starts_obvious_block(line)
}

fn starts_obvious_block(line: &str) -> bool {
    if line.starts_with(".. ") || line.starts_with([':', '|', '#']) || is_adornment(line) {
        return true;
    }

    let Some(first_word) = line.split_ascii_whitespace().next() else {
        return false;
    };
    matches!(first_word, "-" | "+" | "*" | "•" | "‣" | "⁃")
        || first_word
            .strip_suffix(['.', ')'])
            .is_some_and(|marker| !marker.is_empty() && marker.chars().all(char::is_alphanumeric))
}

fn is_adornment(line: &str) -> bool {
    let mut characters = line.chars();
    let Some(marker) = characters.next() else {
        return false;
    };
    marker.is_ascii_punctuation() && characters.all(|character| character == marker)
}

fn leading_backtick_count(input: &str) -> usize {
    input.bytes().take_while(|byte| *byte == b'`').count()
}

fn find_closing_backtick_run(input: &str, opening_tick_count: usize) -> Option<usize> {
    let mut offset = 0;

    while let Some(relative_index) = input[offset..].find('`') {
        let index = offset + relative_index;
        let tick_count = leading_backtick_count(&input[index..]);
        if tick_count == opening_tick_count {
            return Some(index + tick_count);
        }
        offset = index + tick_count;
    }

    None
}

fn is_link_start(input: &str, index: usize) -> bool {
    let before = input[..index].chars().next_back();
    let after = input[index + 1..].chars().next();

    before.is_none_or(|char| {
        char.is_ascii_whitespace()
            || matches!(char, '-' | '/' | ':' | '"' | '\'' | '(' | '<' | '[' | '{')
    }) && after.is_some_and(|char| char != '`' && !char.is_ascii_whitespace())
}

fn is_link_suffix(input: &str) -> bool {
    input.chars().next().is_none_or(|char| {
        char.is_ascii_whitespace()
            || matches!(
                char,
                '-' | '/' | ':' | '.' | ',' | ';' | '!' | '?' | '"' | '\'' | ')' | '>' | ']' | '}'
            )
    })
}

fn is_escaped(input: &str, index: usize) -> bool {
    !input[..index]
        .bytes()
        .rev()
        .take_while(|byte| *byte == b'\\')
        .count()
        .is_multiple_of(2)
}

fn render_markdown_link(output: &mut String, label: Option<&str>, uri: &str) {
    output.push('[');
    if let Some(label) = label {
        push_link_label(output, label);
    } else {
        push_url_as_link_text(output, uri);
    }
    output.push_str("](");
    push_link_destination(output, uri);
    output.push(')');
}

/// Escapes underscores and HTML-sensitive characters in prose outside inline
/// code spans.
///
/// For example, `__init__` becomes `\_\_init\_\_`, while `` `__init__` ``
/// remains unchanged.
///
/// Conveniently, both reST and Markdown delimit inline code with backticks, so
/// we only have to detect one type of code span.
///
/// Only code spans that open and close on the same source line are recognized; backtick state is
/// intentionally not carried across calls.
pub(super) fn render_line(output: &mut String, line: &str) {
    let mut in_inline_code = false;
    let mut first_chunk = true;
    let mut opening_tick_count = 0;
    let mut current_tick_count = 0;

    for chunk in line.split('`') {
        // First chunk is definitionally not in inline-code and so always plaintext.
        if first_chunk {
            first_chunk = false;
            push_escaped_markdown_text(output, chunk);
            continue;
        }

        // Not in first chunk, emit the ` between the last chunk and this one.
        output.push('`');
        current_tick_count += 1;

        // If we're in an inline block and have enough close-ticks to terminate it, do so.
        // TODO: we parse ``hello```there` as (hello)(there) which probably isn't correct
        // (definitely not for Markdown) but it's close enough for horse grenades in this
        // MVP impl. Notably we're verbatim emitting all the backticks so as long as reST and
        // Markdown agree we're *fine*. The accuracy of this parsing only affects the
        // accuracy of where we apply escaping (so we need to misparse and see escapables
        // for any of this to matter).
        if opening_tick_count > 0 && current_tick_count >= opening_tick_count {
            opening_tick_count = 0;
            current_tick_count = 0;
            in_inline_code = false;
        }

        // If this chunk is completely empty we're just in a run of ticks.
        if chunk.is_empty() {
            continue;
        }

        // Ok the chunk is non-empty, our run of ticks is complete.
        if in_inline_code {
            // The previous check for >= opening_tick_count didn't trip, so these can't close
            // and these ticks will be verbatim rendered in the content.
            current_tick_count = 0;
        } else if current_tick_count > 0 {
            // Ok we're now in inline code.
            opening_tick_count = current_tick_count;
            current_tick_count = 0;
            in_inline_code = true;
        }

        // Finally include the content either escaped or not.
        if in_inline_code {
            output.push_str(chunk);
        } else {
            push_escaped_markdown_text(output, chunk);
        }
    }
    // NOTE: explicitly not "flushing" the ticks here.
    // We respect however the user closed their inline code.
}

fn push_escaped_markdown_text(output: &mut String, input: &str) {
    for char in input.chars() {
        match char {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '_' => output.push_str("\\_"),
            _ => output.push(char),
        }
    }
}

fn push_link_label(output: &mut String, input: &str) {
    let mut pending_whitespace = false;

    for char in input.chars() {
        if char.is_ascii_whitespace() {
            pending_whitespace = true;
            continue;
        }
        if pending_whitespace {
            output.push(' ');
            pending_whitespace = false;
        }
        push_link_text_char(output, char);
    }
}

fn push_url_as_link_text(output: &mut String, input: &str) {
    for char in input.chars() {
        push_link_text_char(output, char);
    }
}

fn push_link_text_char(output: &mut String, char: char) {
    match char {
        '*' | '[' | ']' | '`' | '|' | '~' | '\\' => {
            output.push('\\');
            output.push(char);
        }
        '&' => output.push_str("&amp;"),
        '<' => output.push_str("&lt;"),
        '>' => output.push_str("&gt;"),
        '_' => output.push_str("\\_"),
        _ => output.push(char),
    }
}

fn push_link_destination(output: &mut String, input: &str) {
    for char in input.chars() {
        match char {
            '(' | ')' | '\\' => {
                output.push('\\');
                output.push(char);
            }
            '&' => output.push_str("&amp;"),
            _ => output.push(char),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Line, Renderer, render_line};

    #[test]
    fn renders_supported_single_line_links() {
        assert_rendered(&[
            (
                "See `datetime-like <https://numpy.org/doc/stable/reference/arrays.datetime.html>`_ values.",
                "See [datetime-like](https://numpy.org/doc/stable/reference/arrays.datetime.html) values.",
            ),
            (
                "`project docs <https://example.com/docs>`__",
                "[project docs](https://example.com/docs)",
            ),
            (
                "`HTTP docs <http://example.com/docs>`_",
                "[HTTP docs](http://example.com/docs)",
            ),
            (
                "`<https://example.com/_under_/*>`_",
                r"[https://example.com/\_under\_/\*](https://example.com/_under_/*)",
            ),
            (
                "`code` and `link <https://example.com>`_",
                "`code` and [link](https://example.com)",
            ),
            (
                "`not a link <https://inside.example>` and `link <https://example.com>`_",
                "`not a link <https://inside.example>` and [link](https://example.com)",
            ),
            (
                "(`parenthesized <HTTPS://example.com/a_(b)?x=1&y=2>`_)",
                "([parenthesized](HTTPS://example.com/a_\\(b\\)?x=1&amp;y=2))",
            ),
        ]);
    }

    #[test]
    fn renders_supported_multiline_links() {
        assert_rendered(&[
            (
                "See `the documentation\n<https://example.com/docs>`_ for details.",
                "See [the documentation](https://example.com/docs) for details.",
            ),
            (
                "`Sanjoy Dasgupta and Anupam Gupta, 1999,\n\"An elementary proof of the Johnson-Lindenstrauss Lemma.\"\n<https://example.com/paper>`_",
                "[Sanjoy Dasgupta and Anupam Gupta, 1999, \"An elementary proof of the Johnson-Lindenstrauss Lemma.\"](https://example.com/paper)",
            ),
            (
                "`first\n<https://one.example>`_, `second <https://two.example>`_, and `third\n<https://three.example>`_",
                "[first](https://one.example), [second](https://two.example), and [third](https://three.example)",
            ),
            (
                "`anonymous\n<http://example.com>`__",
                "[anonymous](http://example.com)",
            ),
        ]);

        assert_eq!(
            render_docstring(
                "References\n----------\n.. [1] `Cubic Spline Interpolation\n    <https://en.wikiversity.org/wiki/Cubic_Spline_Interpolation>`_"
            ),
            "References  \n----------  \n.. [1] [Cubic Spline Interpolation](https://en.wikiversity.org/wiki/Cubic_Spline_Interpolation)"
        );
        assert_eq!(
            render_docstring("- `wrapped\n  <https://example.com>`_"),
            "- [wrapped](https://example.com)"
        );
        assert_eq!(
            render_docstring("1. `wrapped\n   <https://example.com>`_"),
            "1. [wrapped](https://example.com)"
        );
    }

    #[test]
    fn preserves_candidates_outside_inline_markup_boundaries() {
        for source in [
            "word`link <https://example.com>`_",
            "`link <https://example.com>`_word",
            r"\`link <https://example.com>`_",
            r"`escaped \label <https://example.com>`_",
        ] {
            assert_eq!(render_docstring(source), render_plain_docstring(source));
        }
    }

    #[test]
    fn preserves_links_with_unsupported_uris() {
        for source in [
            "`link <>`_",
            "`link <../../docs.html>`_",
            "`link <ftp://example.com>`_",
            "`link <https://example.com/a b>`_",
            "`link <https://example.com/\u{7f}>`_",
            r"`link <https://example.com/\path>`_",
            "`link <https://example.com/<tag>>`_",
            "`link <https://example.com/[id]>`_",
        ] {
            assert_eq!(render_docstring(source), render_plain_docstring(source));
        }
    }

    #[test]
    fn preserves_links_with_unsupported_label_continuations() {
        for line in [
            "",
            "   ",
            "`code`",
            "<target>",
            r"escaped \ label",
            ".. note::",
            ":field:",
            "| substitution |",
            "# heading",
            "----",
            "- list item",
            "1. list item",
            "• list item",
        ] {
            let source = format!("`label\n{line}\n<https://example.com>`_");
            assert!(
                !render_docstring(&source).contains("[label]("),
                "line should be rejected: {line:?}"
            );
        }
    }

    #[test]
    fn preserves_representative_links_outside_the_supported_subset() {
        // These are representative fallbacks, not an exhaustive list. The
        // `Renderer` contract defines the complete supported subset.
        for source in [
            "`a BCP47\nlanguage code <https://example.com>`_",
            "`docs\n<https://example.com/\npath>`_",
            "`one\ntwo\nthree\n<https://example.com>`_",
            "`label\n- list item\n<https://example.com>`_",
            "  `docs\n<https://example.com>`_",
            "`Table Visualization <../../user_guide/style.ipynb>`_",
            "`generic type`_\n\n.. _generic type: https://example.com/generics",
        ] {
            assert_eq!(render_docstring(source), render_plain_docstring(source));
        }
    }

    #[test]
    fn uses_a_document_wide_markdown_link_guard() {
        // The outer renderer intentionally applies this guard without parsing
        // inline-code or block context.
        for source in [
            "[outer](https://outer.example)\n`inner <https://inner.example>`_",
            "Use `matrix[i][j]`.\n`docs <https://example.com>`_",
        ] {
            assert_eq!(render_docstring(source), render_plain_docstring(source));
        }
    }

    #[test]
    fn skips_preformatted_blocks() {
        assert_eq!(
            render_docstring(
                "Example::\n\n    `literal <https://inner.example>`_\n\n`docs <https://example.com>`_"
            ),
            "Example:    \n```````````python\n    `literal <https://inner.example>`_\n\n```````````\n[docs](https://example.com)"
        );
    }

    #[test]
    fn preserves_existing_inline_rendering() {
        assert_rendered(&[
            ("__init__", r"\_\_init\_\_"),
            ("`__init__`", "`__init__`"),
            ("``C:\\`` and __dunder__", r"``C:\`` and \_\_dunder\_\_"),
            ("This is `unclosed", "This is `unclosed"),
            (r"\` literal `__dunder__", r"\` literal `\_\_dunder\_\_"),
        ]);
    }

    #[test]
    fn renders_many_links() {
        let source = "`link <https://example.com>`_. ".repeat(10_000);
        let rendered = render(&source);
        assert_eq!(
            rendered.matches("[link](https://example.com)").count(),
            10_000
        );
    }

    fn assert_rendered(cases: &[(&str, &str)]) {
        for &(source, expected) in cases {
            assert_eq!(render(source), expected, "source: {source:?}");
        }
    }

    fn render(source: &str) -> String {
        let mut renderer = Renderer::default();
        let mut output = String::new();

        for (index, line) in source.lines().enumerate() {
            let text = line.trim_start_matches(' ');
            let indentation = line.len() - text.len();
            let mut rendered_prefix = if index == 0 {
                String::new()
            } else {
                "  \n".to_owned()
            };
            for _ in 0..indentation {
                rendered_prefix.push_str("&nbsp;");
            }

            renderer.render_line(
                &mut output,
                Line {
                    rendered_prefix: &rendered_prefix,
                    source_indentation: indentation,
                    text,
                },
            );
        }
        renderer.flush_pending_link(&mut output);
        output
    }

    fn render_docstring(source: &str) -> String {
        let mut output = String::new();
        super::super::render_into(&mut output, source);
        output
    }

    fn render_plain_docstring(source: &str) -> String {
        let mut output = String::new();
        let mut first_line = true;
        for line in source.lines() {
            if !first_line {
                output.push_str("  \n");
            }
            first_line = false;
            let text = line.trim_start_matches(' ');
            for _ in 0..line.len() - text.len() {
                output.push_str("&nbsp;");
            }
            render_line(&mut output, text);
        }
        output
    }
}
