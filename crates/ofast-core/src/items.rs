//! CS2's item definitions: weapon names by item definition index, from `scripts/items/items_game.txt`.
//!
//! The file is KeyValues 1 text (8 MB): quoted or bare tokens, `{ }` blocks, `//` comments, optional `[$PLATFORM]`
//! conditions after a value. Items live in `items_game` → `items` → `"<index>"` → `"name"`, e.g. `"7"` → `"name"
//! "weapon_ak47"`. A demo identifies a player's weapon by that index (`m_iItemDefinitionIndex`); CS2's animation
//! graphs select by the name (`weapon_type` = `weapon_ak47`, `weapon_m4a1` for the M4A4, `weapon_knife` for the CT
//! knife…).
//!
//! Only values written in an item's own block are read ([`item_values`]: names, glove models); prefabs, paint kits
//! and the rest are skipped.

use std::collections::HashMap;

use crate::kv1::{Token, tokens};

/// Item names by definition index, from the text of `items_game.txt`.
pub fn item_names(text: &str) -> HashMap<u32, String> {
    item_values(text, "name")
}

/// Every item's value of `key` written in its own block (not inherited from a prefab), by definition index: e.g.
/// `model_world`, which glove items name directly (`5027` → `agents/models/shared/arms/glove_bloodhound/…vmdl`).
pub fn item_values(text: &str, wanted: &str) -> HashMap<u32, String> {
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
                    // A key–value pair inside items_game/items/<index>.
                    if k == wanted
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
}
