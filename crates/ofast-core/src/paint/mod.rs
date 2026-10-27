//! Weapon finishes (M10): which paint a skin is, and the parameters CS2 paints it with (`docs/research/cs2-skins.md`).
//!
//! # From a demo's skin to paint parameters
//!
//! 1. **The paint kit** ([`PaintKit`], [`paint_kits`]): the demo's attribute 6 indexes `items_game.txt` →
//!    `paint_kits` (name, style, wear range, legacy model, composite material).
//! 2. **Its composite material** ([`Recipe`], [`read_recipe`]): `composite_material_path`, else
//!    `weapons/paints/legacy/<name>.vcompmat` (gloves: `gloves/paints/<name>.vcompmat`), a KV3 document
//!    (`CCompositeMaterialEditorDoc`) whose procedure names input containers — the paint material (`paint`), the
//!    weapon's composite inputs (`composite_inputs`, the material the weapon's own material names in its
//!    `composite_inputs` attribute), loose variables (`econ_instance`: seed and wear; `grunge_wear`: where the wear
//!    and grunge textures sit; `exposed_params`: a newer kit's textures and values) — a list of variables the seed
//!    rolls, and the textures to generate. A legacy kit's own file adds its paint material (and sometimes a range,
//!    Fade's rotation) to an included `weapons/paints/legacy/_shared_paint_generic.vcompmat` that does the rest.
//! 3. **The roll** ([`Recipe::roll`]): a [`random::UniformRandom`] seeded with the pattern seed draws each listed
//!    variable in order, within its bounds (pattern offset and rotation, then wear and grunge scale, offset,
//!    rotation).
//! 4. **The parameters** ([`weapon_params`]): the paint material's, then the composite inputs', then the loose
//!    variables' — the generator copies matching keys in that order (`COMP_MAT_PROPERTY_MUTATOR_COPY_MATCHING_KEYS`)
//!    — with the rolled values, the seed and the wear.
//!
//! What each paint style then does with them is the bake's part (next in M10).
//!
//! Not verified yet (to check against CS2's own weapons): that a listed variable a kit does not define still takes
//! its draw (as CS:GO drew offset x, offset y and rotation for every kit), and that a container's alias defined in
//! both an included file and the kit's own merges by variable.

pub mod random;

use std::collections::{BTreeMap, HashMap};

use crate::content::Content;
use crate::export::ExportError;
use crate::export::gltf::{MaterialInfo, read_material};
use crate::items::section_values;
use crate::kv3::Value;
use crate::resource::{BlockKind, Resource};

use random::UniformRandom;

/// A paint kit from `items_game.txt` → `paint_kits`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PaintKit {
    /// The kit's index (the demo's attribute 6): 180.
    pub id: u32,
    /// Its name: `cu_fireserpent_ak47_bravo`.
    pub name: String,
    /// Its name for people, a language reference: `#PaintKit_cu_fireserpent_ak47_bravo_Tag` (→ "Fire Serpent").
    pub tag: Option<String>,
    /// The paint style, 1 – 9 (the paint material's `F_PAINT_STYLE` + 1): 7 is a custom paint job.
    pub style: Option<u32>,
    /// The composite material named in `items_game` (newer kits).
    pub composite: Option<String>,
    /// The range the kit's wear was drawn in.
    pub wear_range: [Option<f32>; 2],
    /// Painted for the CS:GO-era model (`use_legacy_model`): the demo's mesh groups show that one.
    pub legacy_model: bool,
}

impl PaintKit {
    /// The composite materials that may hold the kit's finish, in the order to try.
    pub fn composite_candidates(&self) -> Vec<String> {
        match &self.composite {
            Some(path) => vec![path.clone()],
            None => vec![
                format!("weapons/paints/legacy/{}.vcompmat", self.name),
                format!("gloves/paints/{}.vcompmat", self.name),
            ],
        }
    }
}

/// Every paint kit of `items_game.txt`'s text, by index.
pub fn paint_kits(items_game: &str) -> BTreeMap<u32, PaintKit> {
    let values = |key: &str| section_values(items_game, "paint_kits", key);
    let float =
        |map: &HashMap<u32, String>, id: &u32| map.get(id).and_then(|v| v.parse::<f32>().ok());
    let (tags, styles, paths) = (
        values("description_tag"),
        values("style"),
        values("composite_material_path"),
    );
    let (wear_min, wear_max, legacy) = (
        values("wear_remap_min"),
        values("wear_remap_max"),
        values("use_legacy_model"),
    );
    values("name")
        .into_iter()
        .map(|(id, name)| {
            let kit = PaintKit {
                id,
                name,
                tag: tags.get(&id).cloned(),
                style: styles.get(&id).and_then(|s| s.parse().ok()),
                composite: paths.get(&id).cloned(),
                wear_range: [float(&wear_min, &id), float(&wear_max, &id)],
                legacy_model: legacy.get(&id).is_some_and(|v| v == "1"),
            };
            (id, kit)
        })
        .collect()
}

/// A loose variable of a composite material's container.
#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    /// The container's alias: `econ_instance`, `grunge_wear`, `exposed_params`.
    pub container: String,
    /// The parameter it sets: `g_flWearTexCoordScale`.
    pub name: String,
    /// Its value.
    pub value: VariableValue,
    /// For floats with bounds: the range a roll draws each component in, `[min, max]` (written backwards for a
    /// range drawn backwards: Fade's rotation, −55 to −65).
    pub bounds: Option<[[f32; 2]; 2]>,
}

/// A loose variable's value.
#[derive(Debug, Clone, PartialEq)]
pub enum VariableValue {
    /// One or two floats (`FLOAT1`, `FLOAT2`; y is 0 for one).
    Float([f32; 2]),
    /// An integer (`INTEGER1`).
    Int(i64),
    /// A switch (`BOOLEAN`).
    Bool(bool),
    /// A texture (`RESOURCE_TEXTURE`: its compiled resource, without `_c`).
    Texture(String),
}

/// What a kit's composite material says about painting it: see the [module docs](self).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recipe {
    /// The paint material (the `paint` container): `…/vmats/cu_fireserpent_ak47_bravo.vmat`.
    pub paint_material: Option<String>,
    /// The loose variables, in the order read (includes first; a later one replaces an earlier of the same container
    /// and name).
    pub variables: Vec<Variable>,
    /// The variables the seed rolls, in order: `(container, name, components)`.
    pub rolls: Vec<(String, String, usize)>,
}

/// Errors reading a composite material.
#[derive(Debug, thiserror::Error)]
pub enum PaintError {
    /// The file is not in the archives.
    #[error("{0}: not found")]
    Missing(String),
    /// The file could not be read.
    #[error("{0}: {1}")]
    Unreadable(String, String),
    /// A weapon material without `composite_inputs`: CS2 paints only those.
    #[error("{0}: no composite_inputs, not painted")]
    NotPainted(String),
}

/// The parameters weapon material `material` (one of the weapon model's, `…/ak47.vmat`) is painted with for `recipe`
/// at `seed` and `wear` (step 4 of the [module docs](self)): the material's `composite_inputs` attribute names the weapon's
/// composite inputs.
pub fn weapon_params(
    content: &Content,
    recipe: &Recipe,
    material: &str,
    seed: u32,
    wear: f32,
) -> Result<BTreeMap<String, Param>, PaintError> {
    let read = |path: &str| {
        read_material(content, path).map_err(|e| match e {
            ExportError::Vpk(_) => PaintError::Missing(path.to_owned()),
            e => PaintError::Unreadable(path.to_owned(), e.to_string()),
        })
    };
    let weapon = read(material)?;
    let inputs = weapon
        .strings
        .get("composite_inputs")
        .ok_or_else(|| PaintError::NotPainted(material.to_owned()))?;
    let inputs = read(inputs)?;
    let paint = match &recipe.paint_material {
        Some(path) => read(path)?,
        None => MaterialInfo::default(),
    };
    Ok(recipe.params(&paint, Some(&inputs), seed, wear))
}

/// How deep composite materials may include each other.
const MAX_INCLUDES: usize = 8;

/// Reads the composite material at `path` (`.vcompmat`, with or without `_c`) and what it includes.
pub fn read_recipe(content: &Content, path: &str) -> Result<Recipe, PaintError> {
    let mut recipe = Recipe::default();
    add_file(content, path, &mut recipe, 0)?;
    Ok(recipe)
}

/// The recipe of the first of `kit`'s composite materials that exists.
pub fn kit_recipe(content: &Content, kit: &PaintKit) -> Result<Recipe, PaintError> {
    let candidates = kit.composite_candidates();
    for path in &candidates {
        match read_recipe(content, path) {
            Err(PaintError::Missing(_)) => continue,
            other => return other,
        }
    }
    Err(PaintError::Missing(candidates.join(" or ")))
}

fn add_file(
    content: &Content,
    path: &str,
    recipe: &mut Recipe,
    depth: usize,
) -> Result<(), PaintError> {
    let data = content
        .read_resource(path)
        .map_err(|_| PaintError::Missing(path.to_owned()))?;
    let unreadable = |e: String| PaintError::Unreadable(path.to_owned(), e);
    let resource = Resource::parse(&data).map_err(|e| unreadable(e.to_string()))?;
    let root = resource
        .kv3(BlockKind::DATA)
        .map_err(|e| unreadable(e.to_string()))?
        .root;
    add_document(&root, recipe, &mut |included, recipe| {
        if depth < MAX_INCLUDES {
            add_file(content, included, recipe, depth + 1)
        } else {
            Ok(())
        }
    })
}

/// Something that adds an included composite material (by path) to a recipe.
type Include<'a> = dyn FnMut(&str, &mut Recipe) -> Result<(), PaintError> + 'a;

/// Adds a composite material document's procedures to `recipe`, each one's includes (through `include`) before its
/// own containers.
fn add_document(
    root: &Value,
    recipe: &mut Recipe,
    include: &mut Include<'_>,
) -> Result<(), PaintError> {
    let array = |v: &Value, key: &str| {
        v.get(key)
            .and_then(Value::as_array)
            .unwrap_or_default()
            .to_vec()
    };
    for point in array(root, "m_Points") {
        for procedure in array(&point, "m_vecCompositeMaterialAssemblyProcedures") {
            for included in array(&procedure, "m_vecCompMatIncludes") {
                if let Some(path) = included.as_str() {
                    include(path, recipe)?;
                }
            }
            for container in array(&procedure, "m_vecCompositeInputContainers") {
                add_container(&container, recipe);
            }
            add_mutators(&array(&procedure, "m_vecPropertyMutators"), recipe);
        }
    }
    Ok(())
}

fn add_container(container: &Value, recipe: &mut Recipe) {
    let text = |key: &str| {
        container
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let alias = text("m_strAlias");
    if alias == "paint"
        && let Some(material) = container
            .get("m_strSpecificContainerMaterial")
            .and_then(Value::as_str)
    {
        recipe.paint_material = Some(material.to_owned());
    }
    for variable in container
        .get("m_vecLooseVariables")
        .and_then(Value::as_array)
        .unwrap_or_default()
    {
        if let Some(read) = read_variable(alias, variable) {
            recipe
                .variables
                .retain(|v| !(v.container == read.container && v.name == read.name));
            recipe.variables.push(read);
        }
    }
}

fn read_variable(container: &str, v: &Value) -> Option<Variable> {
    let name = v.get("m_strName")?.as_str()?.to_owned();
    let float = |key: &str| v.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let kind = v.get("m_nVariableType")?.as_str()?;
    let value = match kind {
        "LOOSE_VARIABLE_TYPE_FLOAT1" | "LOOSE_VARIABLE_TYPE_FLOAT2" => {
            VariableValue::Float([float("m_flValueFloatX"), float("m_flValueFloatY")])
        }
        "LOOSE_VARIABLE_TYPE_INTEGER1" => {
            VariableValue::Int(v.get("m_nValueIntX").and_then(Value::as_i64).unwrap_or(0))
        }
        "LOOSE_VARIABLE_TYPE_BOOLEAN" => VariableValue::Bool(
            v.get("m_bValueBoolean")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ),
        "LOOSE_VARIABLE_TYPE_RESOURCE_TEXTURE" => VariableValue::Texture(
            v.get("m_strTextureRuntimeResourcePath")?
                .as_str()?
                .to_owned(),
        ),
        _ => return None,
    };
    let bounds = v
        .get("m_bHasFloatBounds")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        .then(|| {
            [
                [float("m_flValueFloatX_Min"), float("m_flValueFloatX_Max")],
                [float("m_flValueFloatY_Min"), float("m_flValueFloatY_Max")],
            ]
        });
    Some(Variable {
        container: container.to_owned(),
        name,
        value,
        bounds,
    })
}

/// The random rolls among `mutators` (also inside conditional ones), in order.
fn add_mutators(mutators: &[Value], recipe: &mut Recipe) {
    for mutator in mutators {
        let kind = mutator.get("m_nMutatorCommandType").and_then(Value::as_str);
        if kind == Some("COMP_MAT_PROPERTY_MUTATOR_RANDOM_ROLL_INPUT_VARIABLES") {
            for target in mutator
                .get("m_vecRandomRollInputVars_InputVarsToRoll")
                .and_then(Value::as_array)
                .unwrap_or_default()
            {
                if let Some(roll) = target.as_str().and_then(parse_roll) {
                    recipe.rolls.push(roll);
                }
            }
        }
        if let Some(inner) = mutator
            .get("m_vecConditionalMutators")
            .and_then(Value::as_array)
        {
            add_mutators(inner, recipe);
        }
    }
}

/// `econ_instance.g_vPatternTexCoordOffset.xy` → (`econ_instance`, `g_vPatternTexCoordOffset`, 2).
fn parse_roll(target: &str) -> Option<(String, String, usize)> {
    let mut parts = target.split('.');
    let (container, name) = (parts.next()?, parts.next()?);
    let components = parts.next().map_or(1, |swizzle| swizzle.len().clamp(1, 2));
    Some((container.to_owned(), name.to_owned(), components))
}

/// A parameter's value for painting: what a material or a loose variable sets.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// A number, or a vector of up to four (`g_v*`, rolled offsets).
    Floats([f32; 4]),
    /// An integer or switch (`F_*`, `g_b*`, `g_n*`).
    Int(i64),
    /// A texture (its compiled resource, without `_c`).
    Texture(String),
}

impl Recipe {
    /// The variable `name` of container `container`.
    pub fn variable(&self, container: &str, name: &str) -> Option<&Variable> {
        self.variables
            .iter()
            .find(|v| v.container == container && v.name == name)
    }

    /// The values the seed gives the rolled variables, by name: each listed variable's components drawn in order
    /// within its bounds; a variable the kit does not define, or without bounds, still takes its draws and keeps
    /// its value (see "Not verified yet" in the [module docs](self)).
    pub fn roll(&self, seed: u32) -> BTreeMap<String, [f32; 2]> {
        let mut random = UniformRandom::new(seed as i32);
        let mut rolled = BTreeMap::new();
        for (container, name, components) in &self.rolls {
            let variable = self.variable(container, name);
            let mut value = match variable.map(|v| &v.value) {
                Some(VariableValue::Float(xy)) => *xy,
                _ => [0.0; 2],
            };
            for (c, slot) in value.iter_mut().enumerate().take(*components) {
                let draw =
                    |random: &mut UniformRandom, [low, high]: [f32; 2]| random.float(low, high);
                match variable.and_then(|v| v.bounds) {
                    Some(bounds) => *slot = draw(&mut random, bounds[c]),
                    None => {
                        random.next_int();
                    }
                }
            }
            if variable.is_some() {
                rolled.insert(name.clone(), value);
            }
        }
        rolled
    }

    /// The parameters the paint style reads, by name: the paint material's (`paint`), then the weapon's composite
    /// inputs' (`inputs`), then the loose variables, the rolled values, the seed and the wear — each replacing what
    /// came before under the same name (step 4 of the [module docs](self)).
    pub(crate) fn params(
        &self,
        paint: &MaterialInfo,
        inputs: Option<&MaterialInfo>,
        seed: u32,
        wear: f32,
    ) -> BTreeMap<String, Param> {
        let mut params = BTreeMap::new();
        for info in std::iter::once(paint).chain(inputs) {
            for (name, path) in &info.textures {
                params.insert(name.clone(), Param::Texture(path.clone()));
            }
            for (name, value) in &info.ints {
                params.insert(name.clone(), Param::Int(*value));
            }
            for (name, value) in &info.floats {
                params.insert(name.clone(), Param::Floats([*value as f32, 0.0, 0.0, 0.0]));
            }
            for (name, value) in &info.vectors {
                params.insert(name.clone(), Param::Floats(value.map(|c| c as f32)));
            }
        }
        for variable in &self.variables {
            let param = match &variable.value {
                VariableValue::Float([x, y]) => Param::Floats([*x, *y, 0.0, 0.0]),
                VariableValue::Int(value) => Param::Int(*value),
                VariableValue::Bool(value) => Param::Int(i64::from(*value)),
                VariableValue::Texture(path) => Param::Texture(path.clone()),
            };
            params.insert(variable.name.clone(), param);
        }
        for (name, [x, y]) in self.roll(seed) {
            params.insert(name, Param::Floats([x, y, 0.0, 0.0]));
        }
        params.insert("g_nRandomSeed".into(), Param::Int(i64::from(seed)));
        params.insert(
            "g_flWearAmount".into(),
            Param::Floats([wear, 0.0, 0.0, 0.0]),
        );
        params
    }
}

#[cfg(test)]
mod tests;
