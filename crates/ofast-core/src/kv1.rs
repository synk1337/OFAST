//! KeyValues 1, Valve's older text format: `"key" "value"` pairs and `"key" { … }` blocks, with `//` comments and
//! optional `[$PLATFORM]` conditions after a value. CS2 still writes some files in it: `scripts/items/items_game.txt`
//! ([`crate::items`]), the radar overviews (`resource/overviews/<map>.txt`), and Steam's `libraryfolders.vdf`
//! ([`crate::game`]).
//!
//! [`tokens`] streams a file (items_game is 8 MB); [`parse`] builds the tree of a small one.

/// One KeyValues 1 token.
#[derive(Debug, PartialEq)]
pub enum Token<'a> {
    /// A bare word.
    Text(&'a str),
    /// A quoted string without its quotes, escapes left as written.
    Quoted(&'a str),
    /// `{`
    Open,
    /// `}`
    Close,
}

/// Splits KeyValues 1 text into tokens (quoted strings without their quotes, bare words, braces); comments and
/// `[$…]` conditions are dropped.
pub fn tokens(text: &str) -> impl Iterator<Item = Token<'_>> {
    let bytes = text.as_bytes();
    let mut i = 0;
    std::iter::from_fn(move || {
        loop {
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= bytes.len() {
                return None;
            }
            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                b'{' => {
                    i += 1;
                    return Some(Token::Open);
                }
                b'}' => {
                    i += 1;
                    return Some(Token::Close);
                }
                b'[' => {
                    while i < bytes.len() && bytes[i] != b']' {
                        i += 1;
                    }
                    i += 1;
                }
                b'"' => {
                    let start = i + 1;
                    i = start;
                    while i < bytes.len() && bytes[i] != b'"' {
                        // A backslash escapes the next character (\" inside descriptions).
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    let end = i.min(bytes.len());
                    i = end + 1;
                    return Some(Token::Quoted(&text[start..end]));
                }
                _ => {
                    let start = i;
                    while i < bytes.len()
                        && !bytes[i].is_ascii_whitespace()
                        && !matches!(bytes[i], b'{' | b'}' | b'"')
                    {
                        i += 1;
                    }
                    return Some(Token::Text(&text[start..i]));
                }
            }
        }
    })
}

/// A value: text, or a block of key–value pairs in file order (keys can repeat).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A text value, unescaped (`\\` → `\`, `\"` → `"`, `\n` → a new line, `\t` → a tab).
    Text(String),
    /// A block.
    Block(Vec<(String, Value)>),
}

impl Value {
    /// The value of the first `key` in this block (`None` for text or a missing key). Keys compare without case, as
    /// Valve's reader does.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Block(pairs) => pairs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v),
            Self::Text(_) => None,
        }
    }

    /// The text of the first `key` in this block.
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Self::Text(text) => Some(text),
            Self::Block(_) => None,
        }
    }

    /// The key–value pairs of a block (empty for text).
    pub fn pairs(&self) -> &[(String, Value)] {
        match self {
            Self::Block(pairs) => pairs,
            Self::Text(_) => &[],
        }
    }
}

/// The top-level pairs of KeyValues 1 text, as one block. A file cut short keeps what was read; a key without a value
/// at the end is dropped.
pub fn parse(text: &str) -> Value {
    // The blocks being read, outermost first, each with the key it is the value of.
    let mut stack: Vec<(String, Vec<(String, Value)>)> = vec![(String::new(), Vec::new())];
    let mut key: Option<String> = None;
    for token in tokens(text) {
        let quoted = matches!(token, Token::Quoted(_));
        match token {
            Token::Open => stack.push((key.take().unwrap_or_default(), Vec::new())),
            Token::Close => {
                key = None;
                if stack.len() > 1 {
                    let (k, pairs) = stack.pop().expect("more than one");
                    stack
                        .last_mut()
                        .expect("the top level")
                        .1
                        .push((k, Value::Block(pairs)));
                }
            }
            // Valve's reader resolves escapes in quoted strings only: a bare `C:\Games` stays as written.
            Token::Quoted(text) | Token::Text(text) => {
                let text = if quoted {
                    unescape(text)
                } else {
                    text.to_owned()
                };
                match key.take() {
                    None => key = Some(text),
                    Some(k) => stack
                        .last_mut()
                        .expect("the top level")
                        .1
                        .push((k, Value::Text(text))),
                }
            }
        }
    }
    // Close the blocks a cut-short file left open.
    while stack.len() > 1 {
        let (k, pairs) = stack.pop().expect("more than one");
        stack
            .last_mut()
            .expect("the top level")
            .1
            .push((k, Value::Block(pairs)));
    }
    Value::Block(stack.pop().expect("the top level").1)
}

/// A token's text with its escapes resolved.
fn unescape(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_steam_library_folders() {
        let text = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"apps" { "730" "76050433771" }
	}
	"1" { "path" "D:\\SteamLibrary" }
}
"#;
        let root = parse(text);
        let folders = root.get("libraryfolders").expect("its block");
        let paths: Vec<&str> = folders
            .pairs()
            .iter()
            .filter_map(|(_, f)| f.text("path"))
            .collect();
        assert_eq!(paths, [r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]);
        assert_eq!(
            folders
                .get("0")
                .and_then(|f| f.get("apps"))
                .and_then(|a| a.text("730")),
            Some("76050433771")
        );
    }

    #[test]
    fn reads_an_overview_with_comments_conditions_and_case() {
        let text = "// radar\n\"de_dust2\"\n{\n\t\"material\" \"overviews/de_dust2\" [$WIN32]\n\t\"POS_X\" \"-2476\"\n\tscale 4.4\n}";
        let map = parse(text);
        let map = map.get("de_dust2").expect("its block");
        assert_eq!(map.text("pos_x"), Some("-2476"));
        assert_eq!(map.text("scale"), Some("4.4"));
        assert_eq!(map.text("material"), Some("overviews/de_dust2"));
    }

    #[test]
    fn escapes_are_resolved_in_quoted_strings_only() {
        let root = parse(r#""quoted" "C:\\Games\t1" bare C:\Games"#);
        assert_eq!(root.text("quoted"), Some("C:\\Games\t1"));
        assert_eq!(root.text("bare"), Some(r"C:\Games"));
    }

    #[test]
    fn a_cut_short_file_keeps_what_was_read() {
        let map = parse("\"a\" { \"b\" { \"c\" \"1\"");
        assert_eq!(
            map.get("a")
                .and_then(|a| a.get("b"))
                .and_then(|b| b.text("c")),
            Some("1")
        );
    }
}
