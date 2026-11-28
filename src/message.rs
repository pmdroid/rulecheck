//! Per-finding message templates: plain text with `{name}` references to the
//! named capture groups of a rule's regex pattern.
//!
//! A template is parsed once, when the policy loads, into literal text and
//! capture references. Rendering copies the literal text and the text each
//! referenced group captured, in one pass. Nothing in a template or in captured
//! text is evaluated: a captured `{name}`, `$1` or `.*` stays literal text, and
//! neither the template nor the captured text is ever compiled as a regex.

use regex::{Captures, Regex};

/// Most bytes in a template, the same limit as `remediation`.
pub const MAX_TEMPLATE: usize = 512;
/// Most capture references in one template.
pub const MAX_REFERENCES: usize = 16;
/// Most characters of one capture after escaping, before `…` is appended.
pub const MAX_CAPTURE_CHARS: usize = 64;
/// Most bytes in a rendered message, `…` included.
pub const MAX_MESSAGE: usize = 1024;

const ELLIPSIS: char = '…';

enum Piece {
    Text(String),
    Capture(String),
}

pub struct Template {
    pieces: Vec<Piece>,
}

impl Template {
    /// Parses a template and checks every reference against the named groups
    /// of `regex`. `{{` and `}}` write literal braces.
    pub fn parse(template: &str, regex: &Regex) -> Result<Self, String> {
        if template.trim().is_empty()
            || template.len() > MAX_TEMPLATE
            || template.chars().any(char::is_control)
        {
            return Err(format!(
                "provide a single-line message up to {MAX_TEMPLATE} bytes"
            ));
        }
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut chars = template.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    text.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    text.push('}');
                }
                '{' => {
                    let mut name = String::new();
                    loop {
                        match chars.next() {
                            Some('}') => break,
                            Some(c) => name.push(c),
                            None => {
                                return Err(
                                    "message has an unclosed {; write {{ for a literal brace"
                                        .into(),
                                );
                            }
                        }
                    }
                    if !valid_name(&name) {
                        return Err(format!(
                            "message reference {{{name}}} must name a capture group: a letter or \
                             underscore, then letters, digits or underscores; write {{{{ for a \
                             literal brace"
                        ));
                    }
                    if !regex.capture_names().flatten().any(|group| group == name) {
                        let mut groups: Vec<_> = regex.capture_names().flatten().collect();
                        groups.sort_unstable();
                        groups.dedup();
                        let known = match groups.as_slice() {
                            [] => "the pattern has no named groups; add one such as (?P<name>...)"
                                .to_owned(),
                            groups => format!("use one of {}", groups.join(", ")),
                        };
                        return Err(format!(
                            "message references {{{name}}}, which is not a named group of the \
                             pattern; {known}"
                        ));
                    }
                    if !text.is_empty() {
                        pieces.push(Piece::Text(std::mem::take(&mut text)));
                    }
                    pieces.push(Piece::Capture(name));
                }
                '}' => {
                    return Err("message has an unmatched }; write }} for a literal brace".into());
                }
                c => text.push(c),
            }
        }
        if !text.is_empty() {
            pieces.push(Piece::Text(text));
        }
        let references = pieces
            .iter()
            .filter(|piece| matches!(piece, Piece::Capture(_)))
            .count();
        if references == 0 {
            return Err(
                "message references no capture group; reference one such as {name}, or drop \
                 message and keep the fixed remediation"
                    .into(),
            );
        }
        if references > MAX_REFERENCES {
            return Err(format!(
                "message has {references} capture references; use at most {MAX_REFERENCES}"
            ));
        }
        Ok(Self { pieces })
    }

    /// Renders the message of one match. A group that did not take part in
    /// the match renders as empty text, like a group that matched nothing.
    pub fn render(&self, captures: Option<&Captures>) -> String {
        let mut out = String::new();
        for piece in &self.pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Capture(name) => {
                    let value = captures
                        .and_then(|captures| captures.name(name))
                        .map_or("", |found| found.as_str());
                    push_capture(&mut out, value);
                }
            }
        }
        cap(out)
    }
}

/// The names Rulecheck accepts as references. Rust regex group names may also
/// contain `.`, `[` and `]`; templates refuse those so a reference stays a plain word.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= 32
}

/// Appends captured text with control and invisible formatting characters
/// escaped, keeping at most `MAX_CAPTURE_CHARS` characters and marking a cut with `…`.
fn push_capture(out: &mut String, value: &str) {
    let mut kept = 0;
    for c in value.chars() {
        let piece = escape(c);
        let width = piece.chars().count();
        if kept + width > MAX_CAPTURE_CHARS {
            out.push(ELLIPSIS);
            return;
        }
        kept += width;
        out.push_str(&piece);
    }
}

/// Escapes a character that would change how a finding line reads: control
/// characters (C0, DEL and C1, so no captured newline, carriage return or
/// terminal escape reaches the output) and the Unicode format characters that
/// reorder or hide text (bidirectional controls, zero-width characters, BOM).
fn escape(c: char) -> String {
    match c {
        '\n' => "\\n".into(),
        '\r' => "\\r".into(),
        '\t' => "\\t".into(),
        c if c.is_control() || hidden(c) => format!("\\u{{{:x}}}", u32::from(c)),
        c => c.into(),
    }
}

fn hidden(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2028}'..='\u{202e}'
            | '\u{2060}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// Cuts a rendered message to `MAX_MESSAGE` bytes on a character boundary,
/// ending a cut message with `…`.
fn cap(mut message: String) -> String {
    if message.len() <= MAX_MESSAGE {
        return message;
    }
    let mut end = MAX_MESSAGE - ELLIPSIS.len_utf8();
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message.push(ELLIPSIS);
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regex(pattern: &str) -> Regex {
        Regex::new(pattern).unwrap()
    }

    fn render(template: &str, pattern: &str, text: &str) -> String {
        let regex = regex(pattern);
        let template = Template::parse(template, &regex).unwrap();
        template.render(regex.captures(text).as_ref())
    }

    #[test]
    fn references_and_escaped_braces_render_literally() {
        assert_eq!(
            render(
                "{{{key}}} is {value}",
                r"(?P<key>\w+)=(?P<value>\w+)",
                "mode=on"
            ),
            "{mode} is on"
        );
    }

    #[test]
    fn captured_text_is_never_expanded() {
        assert_eq!(
            render("got {v}", r"v=(?P<v>.*)", "v={v} $1 ${v} .* {{"),
            "got {v} $1 ${v} .* {{"
        );
    }

    #[test]
    fn unset_groups_render_empty() {
        assert_eq!(render("[{a}][{b}]", r"(?P<a>x)|(?P<b>y)", "x"), "[x][]");
    }

    #[test]
    fn controls_and_hidden_characters_are_escaped() {
        assert_eq!(
            render(
                "<{v}>",
                r"(?s)v=(?P<v>.*)",
                "v=a\nb\r\t\u{1b}[31m\u{202e}z\u{7f}\u{85}"
            ),
            "<a\\nb\\r\\t\\u{1b}[31m\\u{202e}z\\u{7f}\\u{85}>"
        );
    }

    #[test]
    fn long_captures_and_messages_are_capped() {
        let long = "é".repeat(100);
        let rendered = render("{v}", r"(?P<v>.*)", &long);
        assert_eq!(rendered, format!("{}…", "é".repeat(MAX_CAPTURE_CHARS)));

        let escaped = render("{v}", r"(?s)(?P<v>.*)", &"\u{1b}".repeat(20));
        // Ten six-character escapes fit in 64 characters; an escape is never split.
        assert_eq!(escaped, format!("{}…", "\\u{1b}".repeat(10)));

        // The longest template with the most references still fits the message cap.
        let regex = regex(r"(?P<v>.*)");
        let wide = Template {
            pieces: (0..MAX_REFERENCES)
                .flat_map(|_| [Piece::Text("x".repeat(30)), Piece::Capture("v".into())])
                .collect(),
        };
        let message = wide.render(regex.captures(&"y".repeat(100)).as_ref());
        assert_eq!(message.len(), MAX_MESSAGE);
        assert!(message.ends_with('…'));
    }

    #[test]
    fn invalid_templates_are_refused() {
        let regex = regex(r"(?P<key>\w+)=(?P<value>\w+)|(\d)");
        for (template, error) in [
            ("", "single-line message"),
            ("a\nb {key}", "single-line message"),
            ("no references", "references no capture group"),
            (
                "{missing}",
                "not a named group of the pattern; use one of key, value",
            ),
            ("{1}", "must name a capture group"),
            ("{}", "must name a capture group"),
            ("{key", "unclosed {"),
            ("key}", "unmatched }"),
            ("{ key }", "must name a capture group"),
        ] {
            let found = Template::parse(template, &regex).err().unwrap_or_default();
            assert!(found.contains(error), "{template:?}: {found}");
        }
        let unnamed = Template::parse("{x}", &self::regex(r"(\d)")).err().unwrap();
        assert!(unnamed.contains("has no named groups"), "{unnamed}");
        let many = "{key}".repeat(MAX_REFERENCES + 1);
        let found = Template::parse(&many, &regex).err().unwrap();
        assert!(found.contains("use at most 16"), "{found}");
        let long = format!("{}{{key}}", "x".repeat(MAX_TEMPLATE));
        assert!(Template::parse(&long, &regex).is_err());
    }
}
