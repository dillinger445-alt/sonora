//! Just enough YAML for slskd's settings file (slskd.yml): nested maps by
//! indentation, scalars (plain, 'single' or "double" quoted), lists of
//! scalars ("- item", "[]", "[a, b]") and comments. SONORA reads the values
//! it needs and changes single values in place, so everything else in the
//! file, comments included, stays as the person wrote it.
//!
//! slskd compares keys without underscores, dashes or case ("api_keys",
//! "apiKeys" and "apikeys" are one key), and so does [`Node::get`].

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Map(Vec<(String, Node)>),
    List(Vec<Node>),
    /// A value and the line (0-based) it is on.
    Scalar(String, usize),
    Null,
}

/// A key as slskd compares it.
pub fn normalize_key(k: &str) -> String {
    k.chars().filter(|c| *c != '_' && *c != '-').flat_map(char::to_lowercase).collect()
}

impl Node {
    pub fn get(&self, path: &[&str]) -> Option<&Node> {
        let Some((first, rest)) = path.split_first() else { return Some(self) };
        match self {
            Node::Map(entries) => {
                let want = normalize_key(first);
                // The last of repeated keys wins, as in YAML parsers.
                entries.iter().rev().find(|(k, _)| normalize_key(k) == want).and_then(|(_, v)| v.get(rest))
            }
            _ => None,
        }
    }

    pub fn str_at(&self, path: &[&str]) -> Option<&str> {
        match self.get(path)? {
            Node::Scalar(s, _) => Some(s.as_str()),
            _ => None,
        }
    }

    /// A list of scalars (an absent or empty value is an empty list).
    pub fn strings_at(&self, path: &[&str]) -> Vec<String> {
        match self.get(path) {
            Some(Node::List(items)) => items.iter().filter_map(|n| if let Node::Scalar(s, _) = n { Some(s.clone()) } else { None }).collect(),
            Some(Node::Scalar(s, _)) if !s.is_empty() => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    pub fn line_of(&self, path: &[&str]) -> Option<usize> {
        match self.get(path)? {
            Node::Scalar(_, line) => Some(*line),
            _ => None,
        }
    }

    pub fn keys(&self) -> Vec<&str> {
        match self {
            Node::Map(entries) => entries.iter().map(|(k, _)| k.as_str()).collect(),
            _ => Vec::new(),
        }
    }
}

/// A value written in single quotes ('' stands for one quote).
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[derive(Debug)]
struct Line {
    no: usize,
    indent: usize,
    text: String,
}

/// Removes a comment: `#` at the start or after a space, outside quotes.
fn strip_comment(s: &str) -> &str {
    let b = s.as_bytes();
    let (mut single, mut double) = (false, false);
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'\\' if double => i += 1,
            b'#' if !single && !double && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') => return &s[..i],
            _ => {}
        }
        i += 1;
    }
    s
}

/// One scalar: quotes removed and escapes resolved.
fn scalar(raw: &str) -> String {
    let t = raw.trim();
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        return t[1..t.len() - 1].replace("''", "'");
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = &t[1..t.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('/') => out.push('/'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        }
        return out;
    }
    if t == "~" || t.eq_ignore_ascii_case("null") {
        return String::new();
    }
    t.to_string()
}

/// "[a, 'b', c]" → its items (no nesting).
fn flow_list(t: &str, line: usize) -> Option<Node> {
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Node::List(Vec::new()));
    }
    let mut items = Vec::new();
    let mut cur = String::new();
    let (mut single, mut double) = (false, false);
    for c in inner.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            ',' if !single && !double => {
                items.push(Node::Scalar(scalar(&cur), line));
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    items.push(Node::Scalar(scalar(&cur), line));
    Some(Node::List(items))
}

/// The key of a `key: value` line, and the rest after the colon. Quoted
/// keys may hold colons; plain ones end at the first ": " (or a final ":").
fn split_key(t: &str) -> Option<(String, &str)> {
    if t.starts_with('\'') || t.starts_with('"') {
        let q = t.chars().next()?;
        let end = t[1..].find(q)? + 1;
        let rest = t[end + 1..].trim_start();
        let rest = rest.strip_prefix(':')?;
        return Some((scalar(&t[..=end]), rest));
    }
    let i = t.find(": ").or_else(|| if t.ends_with(':') { Some(t.len() - 1) } else { None })?;
    Some((t[..i].trim().to_string(), &t[i + 1..]))
}

fn parse_block(lines: &[Line], pos: &mut usize, indent: usize) -> Node {
    let Some(first) = lines.get(*pos) else { return Node::Null };
    if first.text.starts_with("- ") || first.text == "-" {
        let mut items = Vec::new();
        while let Some(l) = lines.get(*pos) {
            if l.indent != indent || !(l.text.starts_with("- ") || l.text == "-") {
                break;
            }
            let value = l.text[1..].trim();
            items.push(Node::Scalar(scalar(value), l.no));
            *pos += 1;
            // Anything nested under a list item is not used by slskd's settings SONORA reads.
            while lines.get(*pos).is_some_and(|n| n.indent > indent) {
                *pos += 1;
            }
        }
        return Node::List(items);
    }
    let mut entries = Vec::new();
    while let Some(l) = lines.get(*pos) {
        if l.indent < indent {
            break;
        }
        if l.indent > indent {
            // Badly indented: skip it rather than guess.
            *pos += 1;
            continue;
        }
        let Some((key, rest)) = split_key(&l.text) else {
            *pos += 1;
            continue;
        };
        let rest = rest.trim();
        *pos += 1;
        let value = if !rest.is_empty() {
            flow_list(rest, l.no).unwrap_or_else(|| Node::Scalar(scalar(rest), l.no))
        } else if let Some(next) = lines.get(*pos) {
            // A list may sit at the key's own indentation ("key:\n- a").
            if next.indent > indent || (next.indent == indent && (next.text.starts_with("- ") || next.text == "-")) {
                let child_indent = next.indent;
                parse_block(lines, pos, child_indent)
            } else {
                Node::Null
            }
        } else {
            Node::Null
        };
        entries.push((key, value));
    }
    Node::Map(entries)
}

pub fn parse(text: &str) -> Node {
    let lines: Vec<Line> = text
        .lines()
        .enumerate()
        .filter_map(|(no, raw)| {
            if raw.trim_start().starts_with("---") || raw.trim_start().starts_with("...") {
                return None;
            }
            let stripped = strip_comment(raw).trim_end();
            let indent = stripped.len() - stripped.trim_start().len();
            let t = stripped.trim_start();
            if t.is_empty() {
                None
            } else {
                Some(Line { no, indent, text: t.to_string() })
            }
        })
        .collect();
    let mut pos = 0;
    let indent = lines.first().map(|l| l.indent).unwrap_or(0);
    parse_block(&lines, &mut pos, indent)
}

/// Replaces the value of the scalar at `path` with `value` (already quoted
/// as it should appear), keeping the line's indentation and key. `None` when
/// the file has no such scalar.
pub fn replace_scalar(text: &str, path: &[&str], value: &str) -> Option<String> {
    let line = parse(text).line_of(path)?;
    let mut out = String::with_capacity(text.len() + value.len());
    for (i, l) in text.split_inclusive('\n').enumerate() {
        if i != line {
            out.push_str(l);
            continue;
        }
        let ending = if l.ends_with("\r\n") {
            "\r\n"
        } else if l.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let body = &l[..l.len() - ending.len()];
        let indent = &body[..body.len() - body.trim_start().len()];
        let t = body.trim_start();
        let key_end = if t.starts_with('\'') || t.starts_with('"') {
            let q = t.chars().next()?;
            t[1..].find(q).map(|e| e + 2)?
        } else {
            t.find(':')?
        };
        out.push_str(indent);
        out.push_str(&t[..key_end]);
        out.push_str(": ");
        out.push_str(value);
        out.push_str(ending);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# slskd settings, written by SONORA's setup script.\n\
soulseek:\n  username: 'dj ''night'''\n  password: \"p#ss \\\"x\\\"\" # a comment\n\
directories:\n  downloads: /Users/me/Music/Soulseek\n\
shares:\n  directories: []\n\
web:\n  ip_address: 127.0.0.1\n  https:\n    disabled: true\n  authentication:\n    username: slskd\n    password: 'w3b'\n    api_keys:\n      sonora:\n        key: 0123456789abcdef0123456789abcdef\n        role: readwrite\n\
retention:\n  search: 1440\n";

    #[test]
    fn reads_slskd_settings() {
        let n = parse(SAMPLE);
        assert_eq!(n.str_at(&["soulseek", "username"]), Some("dj 'night'"));
        assert_eq!(n.str_at(&["soulseek", "password"]), Some("p#ss \"x\""));
        assert_eq!(n.str_at(&["directories", "downloads"]), Some("/Users/me/Music/Soulseek"));
        assert_eq!(n.strings_at(&["shares", "directories"]), Vec::<String>::new());
        assert_eq!(n.str_at(&["web", "authentication", "apiKeys", "sonora", "key"]), Some("0123456789abcdef0123456789abcdef"));
        assert_eq!(n.str_at(&["WEB", "IpAddress"]), Some("127.0.0.1"));
        assert_eq!(n.str_at(&["retention", "search"]), Some("1440"));
        assert!(n.get(&["nothing", "here"]).is_none());
    }

    #[test]
    fn lists_in_both_styles() {
        let n = parse("shares:\n  directories:\n    - '/a b'\n    - /c\nother:\n- x\n- \"y\"\nflow: [one, 'two, three']\n");
        assert_eq!(n.strings_at(&["shares", "directories"]), vec!["/a b".to_string(), "/c".to_string()]);
        assert_eq!(n.strings_at(&["other"]), vec!["x".to_string(), "y".to_string()]);
        assert_eq!(n.strings_at(&["flow"]), vec!["one".to_string(), "two, three".to_string()]);
    }

    #[test]
    fn changes_one_value_and_keeps_the_rest() {
        let changed = replace_scalar(SAMPLE, &["web", "authentication", "api_keys", "sonora", "key"], &quote("NEWKEY0123456789")).unwrap();
        assert_eq!(parse(&changed).str_at(&["web", "authentication", "api_keys", "sonora", "key"]), Some("NEWKEY0123456789"));
        assert_eq!(changed.lines().count(), SAMPLE.lines().count());
        assert!(changed.starts_with("# slskd settings"));
        assert!(changed.contains("        key: 'NEWKEY0123456789'\n"));
        let pw = replace_scalar(&changed, &["soulseek", "password"], &quote("it's new")).unwrap();
        let n = parse(&pw);
        assert_eq!(n.str_at(&["soulseek", "password"]), Some("it's new"));
        assert_eq!(n.str_at(&["soulseek", "username"]), Some("dj 'night'"));
        assert!(replace_scalar(SAMPLE, &["soulseek", "description"], "'x'").is_none());
        // Windows line endings stay.
        let crlf = SAMPLE.replace('\n', "\r\n");
        let c = replace_scalar(&crlf, &["soulseek", "username"], &quote("b")).unwrap();
        assert!(c.contains("  username: 'b'\r\n"));
    }

    #[test]
    fn quoting_round_trips() {
        for s in ["plain", "it's", "a: b # c", "''", "-dash", "ünïcödé"] {
            let doc = format!("k: {}\n", quote(s));
            assert_eq!(parse(&doc).str_at(&["k"]), Some(s), "{s}");
        }
    }
}
