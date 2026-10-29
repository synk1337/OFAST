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
//! 4. **The parameters** ([`weapon_params`]): a set per texture the recipe generates ([`Generated`]: the colour,
//!    `g_tColor`; roughness and metalness, `g_tMetalness`), from its initial container (the paint material) through
//!    its steps, each done when its conditions hold: copy every parameter of a container (the composite inputs, the
//!    loose variables: `COMP_MAT_PROPERTY_MUTATOR_COPY_MATCHING_KEYS`), copy one (`COPY_PROPERTY`: a newer kit's own
//!    paint masks over the weapon's), set one (`SET_VALUE`: a texture switch off when the kit's `g_bUse*` is). The
//!    loose variables hold the rolled values, the seed and the wear. A recipe that generates neither: the paint
//!    material's, the composite inputs', the loose variables', in that order.
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
    /// A colour (`COLOR4`: bytes, as a material's colour parameters hold them, 0 – 1; alpha 1 when not written).
    Color([f32; 4]),
    /// Three or four floats (`FLOAT3`, `FLOAT4`: per-colour metalness and roughness; w 0 for three).
    Vector([f32; 4]),
    /// An integer (`INTEGER1`).
    Int(i64),
    /// A switch (`BOOLEAN`).
    Bool(bool),
    /// A texture (`RESOURCE_TEXTURE`: its compiled resource, without `_c`).
    Texture(String),
    /// A material (`RESOURCE_MATERIAL`): its container holds that material's parameters too (a 2025 glove kit's
    /// `paint`).
    Material(String),
}

/// What a kit's composite material says about painting it: see the [module docs](self).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recipe {
    /// The paint material (the `paint` container): `…/vmats/cu_fireserpent_ak47_bravo.vmat`.
    pub paint_material: Option<String>,
    /// Other containers holding a specific material: `(alias, path)` (a newer glove kit's `paint_compositor`).
    pub other_materials: Vec<(String, String)>,
    /// The loose variables, in the order read (includes first; a later one replaces an earlier of the same container
    /// and name).
    pub variables: Vec<Variable>,
    /// The random rolls, in order (one for a weapon; a glove's two, by which seed its material takes).
    pub rolls: Vec<Roll>,
    /// Containers holding one of the target material's attributes (`CONTAINER_SOURCE_TYPE_VARIABLE_FROM_TARGET_ATTR`):
    /// `(alias, attribute)`, a glove's `handedness` ← `left`, `material_seed` ← `seed_offset`.
    pub attribute_containers: Vec<(String, String)>,
    /// Copies into a container's own variable before the textures are generated (a `COPY_PROPERTY` whose target is
    /// `container.variable`: a left glove's pattern offset).
    pub container_copies: Vec<Step>,
    /// The textures it generates, each with its steps (a later one replaces an earlier for the same texture).
    pub generated: Vec<Generated>,
    /// Copies onto the painted material's own parameters (a `COPY_PROPERTY` whose target is a plain name): the
    /// gunsmith template's own normal map (`g_tNormal` when `g_bUseNormalMap`) and occlusion, glitter, pearlescence.
    pub material_copies: Vec<Step>,
}

/// A random roll (`COMP_MAT_PROPERTY_MUTATOR_RANDOM_ROLL_INPUT_VARIABLES`): a uniform random stream seeded by a
/// container's variable draws each listed variable in order, when the conditions hold.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Roll {
    /// The seed: `(container, variable)`, `econ_instance.g_nRandomSeed`.
    pub seed: (String, String),
    /// When (all of them).
    pub conditions: Vec<Condition>,
    /// The variables drawn, in order: `(container, name, components)`.
    pub variables: Vec<(String, String, usize)>,
}

/// A texture a composite material generates into the weapon's material (`COMP_MAT_PROPERTY_MUTATOR_GENERATE_TEXTURE`):
/// the paint material's parameters for it are its initial container's, then each step's whose conditions hold, in
/// order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Generated {
    /// The weapon parameter it fills: `g_tColor`, `g_tMetalness` (roughness and metalness).
    pub target: String,
    /// The container it starts from: `paint`.
    pub initial: String,
    /// The steps (`m_vecTexGenInstructions`; a conditional group's steps carry its conditions too).
    pub steps: Vec<Step>,
}

/// One step of a generated texture, done when all its conditions hold.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// What it does.
    pub action: Action,
    /// When (all of them).
    pub conditions: Vec<Condition>,
}

/// What a step does to the parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Copies every parameter of a container (`COPY_MATCHING_KEYS`).
    CopyKeys(String),
    /// Copies one parameter of a container under a name (`COPY_PROPERTY`): container, its parameter, the name.
    CopyProperty(String, String, String),
    /// Sets a value (`SET_VALUE`).
    Set(String, VariableValue),
}

/// When a step is done.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// A container's parameter has a value, as written (`1`, `0`, `true`): `INPUT_CONTAINER_VALUE_EQUALS`.
    Equals(String, String, String),
    /// A container has a parameter: `INPUT_CONTAINER_VALUE_EXISTS`.
    Exists(String, String),
    /// A container exists: `INPUT_CONTAINER_EXISTS`.
    Container(String),
    /// The other way round (`m_bPassWhenTrue = false`).
    Not(Box<Condition>),
    /// A condition not read: never holds.
    Unknown(String),
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

/// The parameters a weapon material's two generated textures are painted with.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeaponParams {
    /// Its colour's (`g_tColor`).
    pub color: BTreeMap<String, Param>,
    /// Its roughness and metalness's (`g_tMetalness`).
    pub surface: BTreeMap<String, Param>,
    /// Painted with the gloves' shader (`csgo_customglove.vfx`), not the weapons'.
    pub glove: bool,
    /// What the recipe sets on the weapon's own material besides the textures it generates (its
    /// `material_copies` whose conditions hold): a kit's own normal map as `g_tNormal`.
    pub material: BTreeMap<String, Param>,
}

/// The parameters weapon material `material` (one of the weapon model's, `…/ak47.vmat`, or a glove model's) is
/// painted with for `recipe` at `seed` and `wear` (step 4 of the [module docs](self)): the material's
/// `composite_inputs` attribute names the weapon's composite inputs; a glove's material is its own composite target.
pub fn weapon_params(
    content: &Content,
    recipe: &Recipe,
    material: &str,
    seed: u32,
    wear: f32,
) -> Result<WeaponParams, PaintError> {
    let read = |path: &str| {
        read_material(content, path).map_err(|e| match e {
            ExportError::Vpk(_) => PaintError::Missing(path.to_owned()),
            e => PaintError::Unreadable(path.to_owned(), e.to_string()),
        })
    };
    let weapon = read(material)?;
    let inputs = match weapon.strings.get("composite_inputs") {
        Some(path) => read(path)?,
        // A glove's materials are composite targets themselves, with no inputs of their own.
        None if weapon.strings.contains_key("composite_target") => MaterialInfo::default(),
        None => return Err(PaintError::NotPainted(material.to_owned())),
    };
    let paint = match &recipe.paint_material {
        Some(path) => read(path)?,
        None => MaterialInfo::default(),
    };
    // Other specific materials, and the materials loose variables name (their containers hold their parameters: a
    // 2025 glove kit's `paint`; listed later, they replace an empty `paint`).
    let named = recipe.variables.iter().filter_map(|v| match &v.value {
        VariableValue::Material(path) => Some((v.container.as_str(), path)),
        _ => None,
    });
    let others = recipe
        .other_materials
        .iter()
        .map(|(alias, path)| (alias.as_str(), path))
        .chain(named)
        .map(|(alias, path)| Ok((alias, read(path)?)))
        .collect::<Result<Vec<_>, PaintError>>()?;
    let mut materials = vec![
        ("paint", &paint),
        ("composite_inputs", &inputs),
        ("target", &weapon),
        ("target_instance", &weapon),
    ];
    materials.extend(others.iter().map(|(alias, info)| (*alias, info)));
    let containers = recipe.containers(&materials, seed, wear);
    let texture = |target: &str| {
        recipe
            .generate(target, &containers)
            .unwrap_or_else(|| recipe.params(&paint, Some(&inputs), seed, wear))
    };
    // The shader the colour is rendered with: its first container's material.
    let initial = recipe
        .generated
        .iter()
        .find(|g| g.target == "g_tColor")
        .map_or("paint", |g| g.initial.as_str());
    let shader = materials
        .iter()
        .find(|(alias, _)| *alias == initial)
        .map_or(paint.shader.as_str(), |(_, info)| info.shader.as_str());
    Ok(WeaponParams {
        color: texture("g_tColor"),
        surface: texture("g_tMetalness"),
        glove: shader == "csgo_customglove.vfx",
        material: recipe.copied_onto_material(&containers),
    })
}

/// Every container's parameters, by alias.
type Containers = BTreeMap<String, BTreeMap<String, Param>>;

/// A material's parameters as painting reads them.
fn material_params(info: &MaterialInfo) -> BTreeMap<String, Param> {
    let mut params = BTreeMap::new();
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
    params
}

impl From<&VariableValue> for Param {
    fn from(value: &VariableValue) -> Self {
        match value {
            VariableValue::Float([x, y]) => Param::Floats([*x, *y, 0.0, 0.0]),
            VariableValue::Color(rgba) | VariableValue::Vector(rgba) => Param::Floats(*rgba),
            VariableValue::Int(value) => Param::Int(*value),
            VariableValue::Bool(value) => Param::Int(i64::from(*value)),
            VariableValue::Texture(path) | VariableValue::Material(path) => {
                Param::Texture(path.clone())
            }
        }
    }
}

impl Param {
    /// Whether this is the value a condition writes (`1`, `0.5`, `true` or `True`, a texture's path). The gunsmith
    /// template writes `True` (`g_bLegacyPatina`): read as an unknown word, the four kits that set it kept patina
    /// age where CS2 paints the legacy patina.
    fn is(&self, written: &str) -> bool {
        let number = if written.eq_ignore_ascii_case("true") {
            Some(1.0)
        } else if written.eq_ignore_ascii_case("false") {
            Some(0.0)
        } else {
            written.parse::<f64>().ok()
        };
        match self {
            Param::Int(i) => number == Some(*i as f64),
            // As stored: a written 0.1 is the f32 nearest it.
            Param::Floats(v) => number.map(|n| n as f32) == Some(v[0]),
            Param::Texture(path) => path == written || written.strip_suffix("_c") == Some(path),
        }
    }
}

impl Condition {
    fn holds(&self, containers: &Containers) -> bool {
        let value = |container: &str, name: &str| containers.get(container)?.get(name);
        match self {
            Condition::Equals(container, name, written) => {
                value(container, name).is_some_and(|v| v.is(written))
            }
            Condition::Exists(container, name) => value(container, name).is_some(),
            Condition::Container(container) => containers.contains_key(container),
            Condition::Not(condition) => !condition.holds(containers),
            Condition::Unknown(_) => false,
        }
    }
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
    if let Some(material) = container
        .get("m_strSpecificContainerMaterial")
        .and_then(Value::as_str)
    {
        if alias == "paint" {
            recipe.paint_material = Some(material.to_owned());
        } else {
            recipe.other_materials.retain(|(a, _)| a != alias);
            recipe
                .other_materials
                .push((alias.to_owned(), material.to_owned()));
        }
    }
    if text("m_nCompositeMaterialInputContainerSourceType")
        == "CONTAINER_SOURCE_TYPE_VARIABLE_FROM_TARGET_ATTR"
    {
        let attribute = text("m_strAttrNameForVar").to_owned();
        recipe.attribute_containers.retain(|(a, _)| a != alias);
        recipe
            .attribute_containers
            .push((alias.to_owned(), attribute));
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
        "LOOSE_VARIABLE_TYPE_FLOAT3" | "LOOSE_VARIABLE_TYPE_FLOAT4" => VariableValue::Vector([
            float("m_flValueFloatX"),
            float("m_flValueFloatY"),
            float("m_flValueFloatZ"),
            float("m_flValueFloatW"),
        ]),
        "LOOSE_VARIABLE_TYPE_COLOR4" => {
            let bytes = v
                .get("m_cValueColor4")
                .and_then(Value::as_array)
                .unwrap_or_default();
            let byte = |i: usize, default: f64| {
                bytes.get(i).and_then(Value::as_f64).unwrap_or(default) as f32 / 255.0
            };
            VariableValue::Color([byte(0, 0.0), byte(1, 0.0), byte(2, 0.0), byte(3, 255.0)])
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
        "LOOSE_VARIABLE_TYPE_RESOURCE_MATERIAL" => {
            VariableValue::Material(v.get("m_strResourceMaterial")?.as_str()?.to_owned())
        }
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

/// The random rolls and generated textures among `mutators` (also inside conditional ones), in order.
fn add_mutators(mutators: &[Value], recipe: &mut Recipe) {
    for mutator in mutators {
        let text = |key: &str| {
            mutator
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        match mutator.get("m_nMutatorCommandType").and_then(Value::as_str) {
            Some("COMP_MAT_PROPERTY_MUTATOR_RANDOM_ROLL_INPUT_VARIABLES") => {
                let seed = text("m_strRandomRollInputVars_SeedInputVar");
                let seed = match seed.split_once('.') {
                    Some((container, name)) => (container.to_owned(), name.to_owned()),
                    None => ("econ_instance".to_owned(), "g_nRandomSeed".to_owned()),
                };
                recipe.rolls.push(Roll {
                    seed,
                    conditions: conditions_of(mutator),
                    variables: mutator
                        .get("m_vecRandomRollInputVars_InputVarsToRoll")
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|target| target.as_str().and_then(parse_roll))
                        .collect(),
                });
            }
            // Into a container's own variable (`container.variable`), before the textures; a plain property is the
            // painted material's own.
            Some("COMP_MAT_PROPERTY_MUTATOR_COPY_PROPERTY") => {
                let target = text("m_strCopyProperty_TargetProperty");
                let step = Step {
                    action: Action::CopyProperty(
                        text("m_strCopyProperty_InputContainerSrc"),
                        text("m_strCopyProperty_InputContainerProperty"),
                        target.clone(),
                    ),
                    conditions: conditions_of(mutator),
                };
                if target.contains('.') {
                    recipe.container_copies.push(step);
                } else {
                    recipe.material_copies.push(step);
                }
            }
            Some("COMP_MAT_PROPERTY_MUTATOR_GENERATE_TEXTURE") => {
                let mut steps = Vec::new();
                add_steps(
                    mutator
                        .get("m_vecTexGenInstructions")
                        .and_then(Value::as_array)
                        .unwrap_or_default(),
                    &[],
                    &mut steps,
                );
                let generated = Generated {
                    target: text("m_strGenerateTexture_TargetParam"),
                    initial: text("m_strGenerateTexture_InitialContainer"),
                    steps,
                };
                recipe.generated.retain(|g| g.target != generated.target);
                recipe.generated.push(generated);
            }
            _ => {}
        }
        if let Some(inner) = mutator
            .get("m_vecConditionalMutators")
            .and_then(Value::as_array)
        {
            add_mutators(inner, recipe);
        }
    }
}

/// The steps of a generated texture's `instructions`, each with `inherited` conditions and its own.
fn add_steps(instructions: &[Value], inherited: &[Condition], steps: &mut Vec<Step>) {
    for instruction in instructions {
        let text = |key: &str| {
            instruction
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let mut conditions = inherited.to_vec();
        conditions.extend(conditions_of(instruction));
        let action = match instruction
            .get("m_nMutatorCommandType")
            .and_then(Value::as_str)
        {
            Some("COMP_MAT_PROPERTY_MUTATOR_COPY_MATCHING_KEYS") => {
                Action::CopyKeys(text("m_strCopyMatchingKeys_InputContainerSrc"))
            }
            Some("COMP_MAT_PROPERTY_MUTATOR_COPY_PROPERTY") => Action::CopyProperty(
                text("m_strCopyProperty_InputContainerSrc"),
                text("m_strCopyProperty_InputContainerProperty"),
                text("m_strCopyProperty_TargetProperty"),
            ),
            Some("COMP_MAT_PROPERTY_MUTATOR_SET_VALUE") => {
                match instruction
                    .get("m_nSetValue_Value")
                    .and_then(|v| read_variable("", v))
                {
                    Some(v) => Action::Set(v.name, v.value),
                    None => continue,
                }
            }
            Some("COMP_MAT_PROPERTY_MUTATOR_CONDITIONAL_MUTATORS") => {
                add_steps(
                    instruction
                        .get("m_vecConditionalMutators")
                        .and_then(Value::as_array)
                        .unwrap_or_default(),
                    &conditions,
                    steps,
                );
                continue;
            }
            _ => continue,
        };
        steps.push(Step { action, conditions });
    }
}

/// A mutator's own conditions (`m_vecConditions`).
fn conditions_of(mutator: &Value) -> Vec<Condition> {
    mutator
        .get("m_vecConditions")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .map(read_condition)
        .collect()
}

fn read_condition(condition: &Value) -> Condition {
    let text = |key: &str| {
        condition
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let container = text("m_strMutatorConditionContainerName");
    let name = text("m_strMutatorConditionContainerVarName");
    let read = match condition.get("m_nMutatorCondition").and_then(Value::as_str) {
        Some("COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_VALUE_EQUALS") => Condition::Equals(
            container,
            name,
            text("m_strMutatorConditionContainerVarValue"),
        ),
        Some("COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_VALUE_EXISTS") => {
            Condition::Exists(container, name)
        }
        Some("COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_EXISTS") => {
            Condition::Container(container)
        }
        other => Condition::Unknown(other.unwrap_or_default().to_owned()),
    };
    let pass_when_true = condition
        .get("m_bPassWhenTrue")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    // A condition not read never holds, either way round.
    if pass_when_true || matches!(read, Condition::Unknown(_)) {
        read
    } else {
        Condition::Not(Box::new(read))
    }
}

/// `econ_instance.g_vPatternTexCoordOffset.xy` → (`econ_instance`, `g_vPatternTexCoordOffset`, 2).
fn parse_roll(target: &str) -> Option<(String, String, usize)> {
    let mut parts = target.split('.');
    let (container, name) = (parts.next()?, parts.next()?);
    // Up to four draws: a 2025 glove kit rolls `consumable_random_numbers.xyz` only to advance the stream.
    let components = parts.next().map_or(1, |swizzle| swizzle.len().clamp(1, 4));
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
        self.rolls
            .iter()
            .flat_map(|roll| self.draw(roll, seed))
            .map(|((_, name), value)| (name, value))
            .collect()
    }

    /// `roll`'s draws from a stream seeded with `seed`, by container and name.
    fn draw(&self, roll: &Roll, seed: u32) -> BTreeMap<(String, String), [f32; 2]> {
        let mut random = UniformRandom::new(seed as i32);
        let mut rolled = BTreeMap::new();
        for (container, name, components) in &roll.variables {
            let variable = self.variable(container, name);
            let mut value = match variable.map(|v| &v.value) {
                Some(VariableValue::Float(xy)) => *xy,
                _ => [0.0; 2],
            };
            // Each component takes a draw: within its bounds into the value (x and y), else only advancing the stream.
            for c in 0..*components {
                match (variable.and_then(|v| v.bounds), value.get_mut(c)) {
                    (Some(bounds), Some(slot)) => {
                        let [low, high] = bounds[c];
                        *slot = random.float(low, high);
                    }
                    _ => {
                        random.next_int();
                    }
                }
            }
            if variable.is_some() {
                rolled.insert((container.clone(), name.clone()), value);
            }
        }
        rolled
    }

    /// The containers a generated texture's steps read, by alias: `materials`' parameters (the `target`'s attributes
    /// too, in [`Recipe::attribute_containers`]), and the loose variables' — `econ_instance` holding the seed and
    /// `wear`, then each roll whose conditions hold drawn from its seed variable, then the copies into containers.
    ///
    /// `g_nRandomSeedAlt` (the second glove's seed) is set by the game, not by any file: OFAST takes the pattern seed
    /// until it is checked against CS2.
    pub(crate) fn containers(
        &self,
        materials: &[(&str, &MaterialInfo)],
        seed: u32,
        wear: f32,
    ) -> Containers {
        let mut containers: Containers = materials
            .iter()
            .map(|(alias, info)| ((*alias).to_owned(), material_params(info)))
            .collect();
        let target = materials
            .iter()
            .find(|(alias, _)| *alias == "target")
            .map(|(_, info)| info);
        for (alias, attribute) in &self.attribute_containers {
            let container = containers.entry(alias.clone()).or_default();
            if let Some(value) = target.and_then(|t| t.int_attributes.get(attribute)) {
                container.insert(attribute.clone(), Param::Int(*value));
            }
        }
        for variable in &self.variables {
            containers
                .entry(variable.container.clone())
                .or_default()
                .insert(variable.name.clone(), Param::from(&variable.value));
        }
        let econ = containers.entry("econ_instance".into()).or_default();
        for name in ["g_nRandomSeed", "g_nRandomSeedAlt"] {
            econ.insert(name.into(), Param::Int(i64::from(seed)));
        }
        econ.insert(
            "g_flWearAmount".into(),
            Param::Floats([wear, 0.0, 0.0, 0.0]),
        );
        for roll in &self.rolls {
            if !roll.conditions.iter().all(|c| c.holds(&containers)) {
                continue;
            }
            let roll_seed = match containers
                .get(&roll.seed.0)
                .and_then(|c| c.get(&roll.seed.1))
            {
                Some(Param::Int(s)) => *s as u32,
                _ => seed,
            };
            for ((container, name), [x, y]) in self.draw(roll, roll_seed) {
                containers
                    .entry(container)
                    .or_default()
                    .insert(name, Param::Floats([x, y, 0.0, 0.0]));
            }
        }
        for step in &self.container_copies {
            if let Action::CopyProperty(source, property, target) = &step.action
                && step.conditions.iter().all(|c| c.holds(&containers))
                && let Some((container, name)) = target.split_once('.')
                && let Some(value) = containers
                    .get(source)
                    .and_then(|c| c.get(property))
                    .cloned()
            {
                containers
                    .entry(container.to_owned())
                    .or_default()
                    .insert(name.to_owned(), value);
            }
        }
        containers
    }

    /// The parameters the recipe copies onto the painted material's own (`material_copies` whose conditions hold, a
    /// later one replacing an earlier).
    fn copied_onto_material(&self, containers: &Containers) -> BTreeMap<String, Param> {
        let mut params = BTreeMap::new();
        for step in &self.material_copies {
            if let Action::CopyProperty(source, property, target) = &step.action
                && step.conditions.iter().all(|c| c.holds(containers))
                && let Some(value) = containers.get(source).and_then(|c| c.get(property))
            {
                params.insert(target.clone(), value.clone());
            }
        }
        params
    }

    /// The parameters texture `target` (`g_tColor`, `g_tMetalness`) is generated with: its initial container's,
    /// then each step whose conditions hold; `None` when the recipe does not generate it.
    pub(crate) fn generate(
        &self,
        target: &str,
        containers: &Containers,
    ) -> Option<BTreeMap<String, Param>> {
        let generated = self.generated.iter().find(|g| g.target == target)?;
        let mut params = containers
            .get(&generated.initial)
            .cloned()
            .unwrap_or_default();
        for step in &generated.steps {
            if !step.conditions.iter().all(|c| c.holds(containers)) {
                continue;
            }
            match &step.action {
                Action::CopyKeys(container) => {
                    if let Some(source) = containers.get(container) {
                        params.extend(source.iter().map(|(k, v)| (k.clone(), v.clone())));
                    }
                }
                Action::CopyProperty(container, property, name) => {
                    if let Some(value) = containers.get(container).and_then(|c| c.get(property)) {
                        params.insert(name.clone(), value.clone());
                    }
                }
                Action::Set(name, value) => {
                    params.insert(name.clone(), Param::from(value));
                }
            }
        }
        Some(params)
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
            params.extend(material_params(info));
        }
        for variable in &self.variables {
            params.insert(variable.name.clone(), Param::from(&variable.value));
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
