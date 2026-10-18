//! CS2's item definitions: weapon names by item definition index, from `scripts/items/items_game.txt`.
//!
//! The file is KeyValues 1 text (8 MB): quoted or bare tokens, `{ }` blocks, `//` comments, optional `[$PLATFORM]`
//! conditions after a value. Items live in `items_game` → `items` → `"<index>"` → `"name"`, e.g. `"7"` → `"name"
//! "weapon_ak47"`. A demo identifies a player's weapon by that index (`m_iItemDefinitionIndex`); CS2's animation
//! graphs select by the name (`weapon_type` = `weapon_ak47`, `weapon_m4a1` for the M4A4, `weapon_knife` for the CT
//! knife…).
//!
//! Only the item names are read; prefabs, paint kits and the rest are skipped.

use std::collections::HashMap;

/// One KeyValues 1 token.
#[derive(Debug, PartialEq)]
enum Token<'a> {
    Text(&'a str),
    Open,
    Close,
}

/// Splits KeyValues 1 text into tokens (quoted strings without their quotes, bare words, braces); comments and
/// `[$…]` conditions are dropped.
fn tokens(text: &str) -> impl Iterator<Item = Token<'_>> {
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
                    return Some(Token::Text(&text[start..end]));
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

/// Item names by definition index, from the text of `items_game.txt`.
pub fn item_names(text: &str) -> HashMap<u32, String> {
    let mut names = HashMap::new();
    // Keys of the open blocks, outermost first.
    let mut path: Vec<&str> = Vec::new();
    let mut key: Option<&str> = None;
    for token in tokens(text) {
        match token {
            Token::Open => {
                path.push(key.take().unwrap_or_default());
            }
            Token::Close => {
                path.pop();
                key = None;
            }
            Token::Text(text) => match key.take() {
                None => key = Some(text),
                Some(k) => {
                    // A key–value pair inside items_game/items/<index>.
                    if k == "name"
                        && path.len() == 3
                        && path[0] == "items_game"
                        && path[1] == "items"
                        && let Ok(index) = path[2].parse::<u32>()
                    {
                        names.insert(index, text.to_owned());
                    }
                }
            },
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_item_names_and_skips_the_rest() {
        let text = r#"
"items_game"
{
    // a comment with "quotes" and { braces }
    "game_info" { "first_valid_class" "2" }
    "items"
    {
        "default" { "name" "default" }
        "7"
        {
            "name"      "weapon_ak47"
            "prefab"    "weapon_ak47_prefab"
            "attributes" { "name" "not an item name" }
        }
        "42" { "name" "weapon_knife" "item_description" "a \"quoted\" word" }
        "507" { "name" "weapon_knife_karambit" [$WIN32] }
    }
    "prefabs" { "9" { "name" "not an item" } }
}
"#;
        let names = item_names(text);
        assert_eq!(names.len(), 3);
        assert_eq!(names[&7], "weapon_ak47");
        assert_eq!(names[&42], "weapon_knife");
        assert_eq!(names[&507], "weapon_knife_karambit");
    }
}
