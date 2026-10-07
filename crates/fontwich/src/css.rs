//! CSS syntax fontwich reads: a `font-family` list.

use alloc::borrow::Cow;
use alloc::string::String;

use parlance::{FontFamilyName, GenericFamily};

/// Parses a CSS `font-family` list.
///
/// Returns entries in source order. Generic keywords are unquoted and ASCII
/// case-insensitive. Quoted keywords are named families. Whitespace between
/// unquoted identifiers is collapsed to one space, and quoted names support
/// CSS escapes. CSS-wide keywords and `default` are rejected.
///
/// Parsing stops at the first invalid entry, preserving earlier entries.
/// This differs from CSS declaration parsing, which rejects the whole
/// value.
///
/// Names borrow from `source` when possible. Normalizing whitespace or
/// decoding escapes allocates an owned string.
pub fn parse_font_family(source: &str) -> impl Iterator<Item = FontFamilyName<'_>> + Clone {
    let mut rest = source;
    let mut done = false;
    core::iter::from_fn(move || {
        if done {
            return None;
        }
        match entry(rest) {
            Some((name, after)) => {
                rest = after;
                if name.is_none() {
                    done = true;
                }
                name
            }
            None => {
                done = true;
                None
            }
        }
    })
}

/// The next entry of `source` and what follows it: `None` for an entry that
/// is not valid, `Some((None, _))` at the end of the list.
fn entry(source: &str) -> Option<(Option<FontFamilyName<'_>>, &str)> {
    let source = source.trim_start_matches(is_space);
    if source.is_empty() {
        return Some((None, source));
    }
    let (name, rest) = match source.as_bytes()[0] {
        quote @ (b'"' | b'\'') => {
            let (text, rest) = string(&source[1..], quote as char)?;
            (FontFamilyName::Named(text), rest)
        }
        _ => {
            let end = source.find(',').unwrap_or(source.len());
            let (item, rest) = source.split_at(end);
            (identifiers(item)?, rest)
        }
    };
    // Then the end of the list, or a comma and another entry.
    let rest = rest.trim_start_matches(is_space);
    match rest.strip_prefix(',') {
        Some(after) if !after.trim_start_matches(is_space).is_empty() => Some((Some(name), after)),
        // A trailing comma leaves nothing to be an entry.
        Some(_) => None,
        None if rest.is_empty() => Some((Some(name), rest)),
        None => None,
    }
}

/// The body of a string opened by `quote`, and what follows its close.
///
/// A string runs to its closing quote, or to the end of the input, which
/// CSS Syntax also allows. A newline in it makes it invalid.
fn string(source: &str, quote: char) -> Option<(Cow<'_, str>, &str)> {
    let end = source.find(quote).unwrap_or(source.len());
    let body = &source[..end];
    let rest = source.get(end + 1..).unwrap_or("");
    if body.contains('\n') {
        return None;
    }
    if !body.contains('\\') {
        return Some((Cow::Borrowed(body), rest));
    }
    // An escaped quote does not close the string: walk it properly.
    let mut text = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            c if c == quote => return Some((Cow::Owned(text), &source[at + 1..])),
            '\n' => return None,
            '\\' => unescape(&mut chars, source, &mut text),
            c => text.push(c),
        }
    }
    Some((Cow::Owned(text), ""))
}

/// Decodes the escape after a backslash into `text`: up to six hex digits
/// and one whitespace after them, a line continuation, or the character
/// itself.
fn unescape(
    chars: &mut core::iter::Peekable<core::str::CharIndices<'_>>,
    source: &str,
    text: &mut String,
) {
    let Some(&(start, first)) = chars.peek() else {
        return;
    };
    if first == '\n' {
        chars.next();
        return;
    }
    if !first.is_ascii_hexdigit() {
        chars.next();
        text.push(first);
        return;
    }
    let mut end = start;
    while let Some(&(at, c)) = chars.peek()
        && c.is_ascii_hexdigit()
        && at - start < 6
    {
        end = at + 1;
        chars.next();
    }
    if chars.peek().is_some_and(|&(_, c)| is_space(c)) {
        chars.next();
    }
    let value = u32::from_str_radix(&source[start..end], 16).unwrap_or(0xFFFD);
    text.push(match value {
        0 => '\u{FFFD}',
        value => char::from_u32(value).unwrap_or('\u{FFFD}'),
    });
}

/// An unquoted entry: identifiers separated by whitespace, a family name, or
/// one identifier that is a generic family's keyword.
fn identifiers(item: &str) -> Option<FontFamilyName<'_>> {
    let item = item.trim_matches(is_space);
    let mut words = item.split(is_space).filter(|word| !word.is_empty());
    let first = words.next()?;
    if !words.clone().chain([first]).all(identifier) {
        return None;
    }
    if words.clone().next().is_none() {
        if let Some(generic) = generic(first) {
            return Some(FontFamilyName::Generic(generic));
        }
        if RESERVED.iter().any(|word| first.eq_ignore_ascii_case(word)) {
            return None;
        }
    }
    // One space between words: borrowed where it already is.
    let single = item
        .split(' ')
        .all(|word| !word.is_empty() && !word.contains(is_space));
    let name = if single && !item.contains('\\') {
        Cow::Borrowed(item)
    } else {
        let mut name = String::new();
        for word in item.split(is_space).filter(|word| !word.is_empty()) {
            if !name.is_empty() {
                name.push(' ');
            }
            if word.contains('\\') {
                let mut chars = word.char_indices().peekable();
                while let Some((_, c)) = chars.next() {
                    match c {
                        '\\' => unescape(&mut chars, word, &mut name),
                        c => name.push(c),
                    }
                }
            } else {
                name.push_str(word);
            }
        }
        Cow::Owned(name)
    };
    Some(FontFamilyName::Named(name))
}

/// Keywords that cannot name a family: the CSS-wide ones, and `default`.
const RESERVED: &[&str] = &[
    "inherit",
    "initial",
    "unset",
    "revert",
    "revert-layer",
    "default",
];

/// The generic family `word` is the keyword of, ignoring ASCII case.
fn generic(word: &str) -> Option<GenericFamily> {
    // Every keyword is ASCII and short: lowered on the stack.
    let mut lower = [0u8; 16];
    let bytes = word.as_bytes();
    if bytes.len() > lower.len() || !word.is_ascii() {
        return None;
    }
    for (into, byte) in lower.iter_mut().zip(bytes) {
        *into = byte.to_ascii_lowercase();
    }
    GenericFamily::parse(core::str::from_utf8(&lower[..bytes.len()]).ok()?)
}

/// Whether `word` could be a CSS identifier: no quotes or other punctuation
/// CSS would read as something else, and not starting with a digit or with
/// a hyphen then a digit. Escapes are allowed.
fn identifier(word: &str) -> bool {
    let bytes = word.as_bytes();
    let starts_badly = matches!(bytes, [b'0'..=b'9', ..] | [b'-', b'0'..=b'9', ..] | [b'-']);
    !starts_badly
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '\\') || !c.is_ascii())
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0C')
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    fn parse(source: &str) -> Vec<FontFamilyName<'_>> {
        parse_font_family(source).collect()
    }

    fn named(name: &str) -> FontFamilyName<'_> {
        FontFamilyName::named(name)
    }

    #[test]
    fn generic_keywords_ignore_case_unless_quoted() {
        assert_eq!(parse("Sans-Serif"), [GenericFamily::SansSerif.into()]);
        assert_eq!(
            parse("SERIF, Arial"),
            [GenericFamily::Serif.into(), named("Arial")]
        );
        assert_eq!(parse("'Sans-Serif'"), [named("Sans-Serif")]);
        assert_eq!(parse(" system-ui "), [GenericFamily::SystemUi.into()]);
    }

    #[test]
    fn whitespace_between_identifiers_is_one_space() {
        let names = parse("Times   New\tRoman, serif");
        assert_eq!(
            names,
            [named("Times New Roman"), GenericFamily::Serif.into()]
        );
        // Borrowed where nothing had to change.
        assert!(matches!(
            parse("Noto Sans CJK JP , emoji")[0],
            FontFamilyName::Named(Cow::Borrowed("Noto Sans CJK JP"))
        ));
    }

    #[test]
    fn keywords_are_not_names() {
        assert!(parse("inherit").is_empty());
        assert_eq!(parse("Arial, default, serif"), [named("Arial")]);
        // A keyword among others is part of a name.
        assert_eq!(parse("Default Sans"), [named("Default Sans")]);
    }

    #[test]
    fn strings_hold_escapes() {
        assert_eq!(parse(r#""Foo\"Bar""#), [named("Foo\"Bar")]);
        assert_eq!(
            parse(r"'\41 rial', serif"),
            [named("Arial"), GenericFamily::Serif.into()]
        );
        assert_eq!(parse("\"Unterminated"), [named("Unterminated")]);
    }

    #[test]
    fn an_invalid_entry_ends_the_list() {
        assert_eq!(parse("Arial,,serif"), [named("Arial")]);
        assert_eq!(parse("Arial, 'Brand Sans' junk, serif"), [named("Arial")]);
        assert_eq!(parse("Arial, 12px"), [named("Arial")]);
        assert_eq!(parse("Arial,"), Vec::<FontFamilyName<'_>>::new());
        assert!(parse("").is_empty());
    }
}
