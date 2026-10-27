//! CS2's item definitions: weapon names by item definition index, from `scripts/items/items_game.txt`.
//!
//! The file is KeyValues 1 text (8 MB): quoted or bare tokens, `{ }` blocks, `//` comments, optional `[$PLATFORM]`
//! conditions after a value. Items live in `items_game` → `items` → `"<index>"` → `"name"`, e.g. `"7"` → `"name"
//! "weapon_ak47"`. A demo identifies a player's weapon by that index (`m_iItemDefinitionIndex`); CS2's animation
//! graphs select by the name (`weapon_type` = `weapon_ak47`, `weapon_m4a1` for the M4A4, `weapon_knife` for the CT
//! knife…).
//!
//! Only values written in an item's own block are read ([`item_values`]: names, glove models); prefabs are skipped.
//! Other numbered sections read the same way ([`section_values`]): `paint_kits` (a weapon's finish, M10:
//! `"180"` → `"name" "cu_fireserpent_ak47_bravo"`, `"description_tag" "#PaintKit_cu_fireserpent_ak47_bravo_Tag"`),
//! `sticker_kits`, `attributes`. The names people read come from `resource/csgo_english.txt` ([`language_tokens`]):
//! `PaintKit_cu_fireserpent_ak47_bravo_Tag` → "Fire Serpent".

use std::collections::HashMap;

use crate::kv1::{Token, tokens};

/// Item names by definition index, from the text of `items_game.txt`.
pub fn item_names(text: &str) -> HashMap<u32, String> {
    item_values(text, "name")
}

/// Every item's value of `key` written in its own block (not inherited from a prefab), by definition index: e.g.
/// `model_world`, which glove items name directly (`5027` → `agents/models/shared/arms/glove_bloodhound/…vmdl`).
pub fn item_values(text: &str, wanted: &str) -> HashMap<u32, String> {
    section_values(text, "items", wanted)
}

/// The value of `wanted` in each numbered block of the section `section` of `items_game` (`items`, `paint_kits`,
/// `sticker_kits`, `attributes`…), by the block's number: e.g. `paint_kits`, `description_tag` → `180` →
/// `#PaintKit_cu_fireserpent_ak47_bravo_Tag`. A section may appear more than once; all are read.
pub fn section_values(text: &str, section: &str, wanted: &str) -> HashMap<u32, String> {
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
            Token::Text(text) | Token::Quoted(text) => match key.take() {
                None => key = Some(text),
                Some(k) => {
                    // A key–value pair inside items_game/<section>/<index>.
                    if k == wanted
                        && path.len() == 3
                        && path[0] == "items_game"
                        && path[1] == section
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

/// A language file's strings (`resource/csgo_english.txt`: `lang` → `Tokens` → name, text), by lowercase name (CS2
/// looks them up without regard to case). [`localized`] reads one through an item's `#Name` reference.
pub fn language_tokens(text: &str) -> HashMap<String, String> {
    let mut strings = HashMap::new();
    let mut depth = 0;
    let mut in_tokens = false;
    let mut key: Option<&str> = None;
    for token in tokens(text) {
        match token {
            Token::Open => {
                depth += 1;
                in_tokens = depth == 2 && key.is_some_and(|k| k.eq_ignore_ascii_case("Tokens"));
                key = None;
            }
            Token::Close => {
                depth -= 1;
                in_tokens = false;
                key = None;
            }
            Token::Text(text) | Token::Quoted(text) => match key.take() {
                None => key = Some(text),
                Some(k) if in_tokens => {
                    strings.insert(k.to_ascii_lowercase(), text.to_owned());
                }
                Some(_) => {}
            },
        }
    }
    strings
}

/// The text of an item's name reference (`#PaintKit_cu_fireserpent_ak47_bravo_Tag`) in [`language_tokens`]' strings.
pub fn localized<'a>(strings: &'a HashMap<String, String>, reference: &str) -> Option<&'a str> {
    let name = reference.strip_prefix('#').unwrap_or(reference);
    strings.get(&name.to_ascii_lowercase()).map(String::as_str)
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
        "5027" { "name" "studded_bloodhound_gloves" "model_world" "agents/models/shared/arms/glove_bloodhound/glove_bloodhound.vmdl" }
    }
    "prefabs" { "9" { "name" "not an item" } }
}
"#;
        let names = item_names(text);
        assert_eq!(names.len(), 4);
        assert_eq!(names[&7], "weapon_ak47");
        assert_eq!(names[&42], "weapon_knife");
        assert_eq!(names[&507], "weapon_knife_karambit");
        let models = item_values(text, "model_world");
        assert_eq!(models.len(), 1);
        assert!(models[&5027].ends_with("glove_bloodhound.vmdl"));
    }

    #[test]
    fn reads_paint_kits_from_every_section_and_their_names() {
        let text = r##"
"items_game"
{
    "paint_kits" { "180" { "name" "cu_fireserpent_ak47_bravo" "description_tag" "#PaintKit_cu_fireserpent_ak47_bravo_Tag" "style" "7" } }
    "items" { "7" { "name" "weapon_ak47" } }
    "paint_kits" { "44" { "name" "aa_fade" "description_tag" "#PaintKit_aa_fade_Tag" } }
}
"##;
        let tags = section_values(text, "paint_kits", "description_tag");
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[&180], "#PaintKit_cu_fireserpent_ak47_bravo_Tag");
        assert_eq!(section_values(text, "paint_kits", "style")[&180], "7");
        assert!(!section_values(text, "items", "description_tag").contains_key(&7));

        let english = r#"
"lang"
{
    "Language" "English"
    "Tokens"
    {
        "PaintKit_cu_fireserpent_ak47_bravo_Tag"   "Fire Serpent"
        "PaintKit_aa_fade_Tag"  "Fade"
    }
}
"#;
        let strings = language_tokens(english);
        assert_eq!(strings.len(), 2);
        assert_eq!(localized(&strings, &tags[&180]), Some("Fire Serpent"));
        assert_eq!(localized(&strings, "#paintkit_AA_fade_tag"), Some("Fade"));
        assert_eq!(localized(&strings, "#Nothing"), None);
    }
}
