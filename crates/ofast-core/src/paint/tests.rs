use super::*;
use crate::kv3::text;

/// `_shared_paint_generic.vcompmat`'s shape, trimmed: seed and wear, the wear and grunge ranges, the roll.
const SHARED: &str = r#"<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:generic:version{7412167c-06e9-4698-aff2-e63eb59037e7} -->
{
    _class = "CCompositeMaterialEditorDoc"
    m_Points = [ { m_vecCompositeMaterialAssemblyProcedures = [ {
        m_vecCompositeInputContainers = [
            { m_nCompositeMaterialInputContainerSourceType = "CONTAINER_SOURCE_TYPE_SPECIFIC_MATERIAL" m_strSpecificContainerMaterial = "materials/placeholder.vmat" m_strAlias = "paint" },
            { m_strAlias = "econ_instance" m_vecLooseVariables = [
                { m_strName = "g_nRandomSeed" m_nVariableType = "LOOSE_VARIABLE_TYPE_INTEGER1" m_nValueIntX = 0 },
                { m_strName = "g_flWearAmount" m_nVariableType = "LOOSE_VARIABLE_TYPE_FLOAT1" m_bHasFloatBounds = true m_flValueFloatX = 0.0 m_flValueFloatX_Min = 0.0 m_flValueFloatX_Max = 1.0 },
            ] },
            { m_strAlias = "grunge_wear" m_vecLooseVariables = [
                { m_strName = "g_flWearTexCoordScale" m_nVariableType = "LOOSE_VARIABLE_TYPE_FLOAT1" m_bHasFloatBounds = true m_flValueFloatX = 1.6 m_flValueFloatX_Min = 1.6 m_flValueFloatX_Max = 1.8 },
                { m_strName = "g_vWearTexCoordOffset" m_nVariableType = "LOOSE_VARIABLE_TYPE_FLOAT2" m_bHasFloatBounds = true m_flValueFloatX = 0.0 m_flValueFloatX_Min = 0.0 m_flValueFloatX_Max = 1.0 m_flValueFloatY = 0.0 m_flValueFloatY_Min = 0.0 m_flValueFloatY_Max = 1.0 },
            ] },
        ]
        m_vecPropertyMutators = [
            { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_CONDITIONAL_MUTATORS" m_vecConditionalMutators = [
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_RANDOM_ROLL_INPUT_VARIABLES"
                  m_strRandomRollInputVars_SeedInputVar = "econ_instance.g_nRandomSeed"
                  m_vecRandomRollInputVars_InputVarsToRoll = [
                    "econ_instance.g_vPatternTexCoordOffset.xy",
                    "econ_instance.g_flPatternTexCoordRotation.x",
                    "grunge_wear.g_flWearTexCoordScale.x",
                    "grunge_wear.g_vWearTexCoordOffset.xy",
                  ] },
            ] },
        ]
    } ] } ]
}"#;

/// A legacy kit's own file, as Fade's: the shared procedure included, a rotation range, its paint material.
const FADE: &str = r#"<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:generic:version{7412167c-06e9-4698-aff2-e63eb59037e7} -->
{
    m_Points = [ { m_vecCompositeMaterialAssemblyProcedures = [ {
        m_vecCompMatIncludes = [ "weapons/paints/legacy/_shared_paint_generic.vcompmat" ]
        m_vecCompositeInputContainers = [
            { m_strAlias = "econ_instance" m_vecLooseVariables = [
                { m_strName = "g_flPatternTexCoordRotation" m_nVariableType = "LOOSE_VARIABLE_TYPE_FLOAT1" m_bHasFloatBounds = true m_flValueFloatX = -55.0 m_flValueFloatX_Min = -55.0 m_flValueFloatX_Max = -65.0 },
            ] },
            { m_strSpecificContainerMaterial = "materials/models/weapons/customization/paints/vmats/aa_fade.vmat" m_strAlias = "paint" },
        ]
    } ] } ]
}"#;

fn recipe_of(document: &str) -> Recipe {
    let mut recipe = Recipe::default();
    let root = text::parse(document).expect("kv3 text").root;
    add_document(&root, &mut recipe, &mut |path, recipe| {
        assert!(path.ends_with("_shared_paint_generic.vcompmat"), "{path}");
        let shared = text::parse(SHARED).expect("kv3 text").root;
        add_document(&shared, recipe, &mut |_, _| Ok(()))
    })
    .expect("recipe");
    recipe
}

#[test]
fn a_legacy_kit_adds_its_paint_and_range_to_the_shared_procedure() {
    let recipe = recipe_of(FADE);
    // The kit's own paint material replaces the include's placeholder.
    assert_eq!(
        recipe.paint_material.as_deref(),
        Some("materials/models/weapons/customization/paints/vmats/aa_fade.vmat")
    );
    let names: Vec<(&str, &str)> = recipe
        .variables
        .iter()
        .map(|v| (v.container.as_str(), v.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("econ_instance", "g_nRandomSeed"),
            ("econ_instance", "g_flWearAmount"),
            ("grunge_wear", "g_flWearTexCoordScale"),
            ("grunge_wear", "g_vWearTexCoordOffset"),
            ("econ_instance", "g_flPatternTexCoordRotation"),
        ]
    );
    let rotation = recipe
        .variable("econ_instance", "g_flPatternTexCoordRotation")
        .expect("rotation");
    assert_eq!(rotation.bounds.map(|b| b[0]), Some([-55.0, -65.0]));
    let rolls: Vec<(&str, usize)> = recipe
        .rolls
        .iter()
        .map(|(_, n, c)| (n.as_str(), *c))
        .collect();
    assert_eq!(
        rolls,
        [
            ("g_vPatternTexCoordOffset", 2),
            ("g_flPatternTexCoordRotation", 1),
            ("g_flWearTexCoordScale", 1),
            ("g_vWearTexCoordOffset", 2),
        ]
    );
}

#[test]
fn the_seed_rolls_each_variable_in_its_range_and_undefined_ones_still_draw() {
    let recipe = recipe_of(FADE);
    let rolled = recipe.roll(245);
    // The offset is not defined by this kit: drawn for, but not set.
    assert!(!rolled.contains_key("g_vPatternTexCoordOffset"));
    let rotation = rolled["g_flPatternTexCoordRotation"][0];
    assert!((-65.0..=-55.0).contains(&rotation), "{rotation}");
    let scale = rolled["g_flWearTexCoordScale"][0];
    assert!((1.6..1.8).contains(&scale), "{scale}");
    let [x, y] = rolled["g_vWearTexCoordOffset"];
    assert!((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y));
    // The rotation is the third draw: the offset's two came first.
    let mut random = UniformRandom::new(245);
    random.next_int();
    random.next_int();
    assert_eq!(random.float(-55.0, -65.0), rotation);
    assert_eq!(recipe.roll(245), rolled);
    assert_ne!(recipe.roll(246), rolled);
}

#[test]
fn parameters_layer_paint_inputs_variables_and_the_roll() {
    let recipe = recipe_of(FADE);
    let mut paint = MaterialInfo::default();
    paint.textures.insert(
        "g_tColor".into(),
        "materials/default/default_weapon_color.vtex".into(),
    );
    paint
        .textures
        .insert("g_tPattern".into(), "fade.vtex".into());
    paint.floats.insert("g_flUvScale1".into(), 1.0);
    paint.ints.insert("F_PAINT_STYLE".into(), 5);
    let mut inputs = MaterialInfo::default();
    inputs
        .textures
        .insert("g_tColor".into(), "ak47_color.vtex".into());
    inputs.floats.insert("g_flUvScale1".into(), 0.549);
    let params = recipe.params(&paint, Some(&inputs), 245, 0.0115);
    assert_eq!(params["g_tColor"], Param::Texture("ak47_color.vtex".into()));
    assert_eq!(params["g_tPattern"], Param::Texture("fade.vtex".into()));
    assert_eq!(
        params["g_flUvScale1"],
        Param::Floats([0.549, 0.0, 0.0, 0.0])
    );
    assert_eq!(params["F_PAINT_STYLE"], Param::Int(5));
    assert_eq!(params["g_nRandomSeed"], Param::Int(245));
    assert_eq!(
        params["g_flWearAmount"],
        Param::Floats([0.0115, 0.0, 0.0, 0.0])
    );
    let rotation = recipe.roll(245)["g_flPatternTexCoordRotation"][0];
    assert_eq!(
        params["g_flPatternTexCoordRotation"],
        Param::Floats([rotation, 0.0, 0.0, 0.0])
    );
}

/// A newer kit's composite, as Royal Guard's, trimmed: its exposed parameters and the colour's generation steps.
const EXTENDED: &str = r#"<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:generic:version{7412167c-06e9-4698-aff2-e63eb59037e7} -->
{
    m_Points = [ { m_vecCompositeMaterialAssemblyProcedures = [ {
        m_vecCompositeInputContainers = [
            { m_strSpecificContainerMaterial = "workshop/paintkits/templates/so_extended_template.vmat" m_strAlias = "paint" },
            { m_strAlias = "exposed_params" m_vecLooseVariables = [
                { m_strName = "g_bUseMetalness" m_nVariableType = "LOOSE_VARIABLE_TYPE_BOOLEAN" m_bValueBoolean = false },
                { m_strName = "g_bOverrideDefaultMasks" m_nVariableType = "LOOSE_VARIABLE_TYPE_BOOLEAN" m_bValueBoolean = true },
                { m_strName = "g_tPaintByNumberMasks" m_nVariableType = "LOOSE_VARIABLE_TYPE_RESOURCE_TEXTURE" m_strTextureRuntimeResourcePath = "usps_anodized_masks.vtex" },
                { m_strName = "g_bUseRoughnessByColor" m_nVariableType = "LOOSE_VARIABLE_TYPE_BOOLEAN" m_bValueBoolean = true },
            ] },
        ]
        m_vecPropertyMutators = [
            { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_GENERATE_TEXTURE"
              m_strGenerateTexture_TargetParam = "g_tColor"
              m_strGenerateTexture_InitialContainer = "paint"
              m_vecTexGenInstructions = [
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_COPY_MATCHING_KEYS" m_strCopyMatchingKeys_InputContainerSrc = "composite_inputs" },
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_COPY_MATCHING_KEYS" m_strCopyMatchingKeys_InputContainerSrc = "econ_instance" },
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_SET_VALUE"
                  m_nSetValue_Value = { m_strName = "F_METALNESS_TEXTURE" m_nVariableType = "LOOSE_VARIABLE_TYPE_INTEGER1" m_nValueIntX = 0 }
                  m_vecConditions = [ { m_nMutatorCondition = "COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_VALUE_EQUALS" m_strMutatorConditionContainerName = "exposed_params" m_strMutatorConditionContainerVarName = "g_bUseMetalness" m_strMutatorConditionContainerVarValue = "0" } ] },
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_COPY_MATCHING_KEYS" m_strCopyMatchingKeys_InputContainerSrc = "exposed_params" },
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_COPY_PROPERTY"
                  m_strCopyProperty_InputContainerSrc = "exposed_params" m_strCopyProperty_InputContainerProperty = "g_tPaintByNumberMasks" m_strCopyProperty_TargetProperty = "g_tMasks"
                  m_vecConditions = [ { m_nMutatorCondition = "COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_VALUE_EQUALS" m_strMutatorConditionContainerName = "exposed_params" m_strMutatorConditionContainerVarName = "g_bOverrideDefaultMasks" m_strMutatorConditionContainerVarValue = "1" } ] },
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_SET_VALUE"
                  m_nSetValue_Value = { m_strName = "F_PEARLESCENCE_MASK" m_nVariableType = "LOOSE_VARIABLE_TYPE_INTEGER1" m_nValueIntX = 1 }
                  m_vecConditions = [ { m_nMutatorCondition = "COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_VALUE_EQUALS" m_strMutatorConditionContainerName = "exposed_params" m_strMutatorConditionContainerVarName = "g_bUseMetalness" m_strMutatorConditionContainerVarValue = "0" m_bPassWhenTrue = false } ] },
                { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_CONDITIONAL_MUTATORS" m_vecConditionalMutators = [
                    { m_nMutatorCommandType = "COMP_MAT_PROPERTY_MUTATOR_SET_VALUE"
                      m_nSetValue_Value = { m_strName = "F_ROUGHNESS_PER_COLOR" m_nVariableType = "LOOSE_VARIABLE_TYPE_INTEGER1" m_nValueIntX = 1 }
                      m_vecConditions = [ { m_nMutatorCondition = "COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_VALUE_EQUALS" m_strMutatorConditionContainerName = "exposed_params" m_strMutatorConditionContainerVarName = "g_bUseRoughnessByColor" m_strMutatorConditionContainerVarValue = "true" } ] },
                  ]
                  m_vecConditions = [ { m_nMutatorCondition = "COMP_MAT_MUTATOR_CONDITION_INPUT_CONTAINER_EXISTS" m_strMutatorConditionContainerName = "override_vars" } ] },
              ] },
        ]
    } ] } ]
}"#;

#[test]
fn generated_textures_follow_their_steps_and_conditions() {
    let recipe = recipe_of(EXTENDED);
    assert_eq!(recipe.generated.len(), 1);
    let steps = &recipe.generated[0].steps;
    assert_eq!(steps.len(), 7);
    // The conditional group's step carries the group's condition and its own.
    assert_eq!(steps[6].conditions.len(), 2);
    let mut paint = MaterialInfo::default();
    paint.ints.insert("F_METALNESS_TEXTURE".into(), 1);
    paint.ints.insert("F_PAINT_STYLE".into(), 0);
    let mut inputs = MaterialInfo::default();
    inputs
        .textures
        .insert("g_tMasks".into(), "usp_masks.vtex".into());
    inputs.ints.insert("F_SEPARATE_CHANNEL_INPUTS".into(), 1);
    let containers = recipe.containers(
        &[("paint", &paint), ("composite_inputs", &inputs)],
        700,
        0.0513,
    );
    let params = recipe.generate("g_tColor", &containers).expect("generated");
    // The template's switch is off, the kit's own masks replace the weapon's, the inputs' feature came through.
    assert_eq!(params["F_METALNESS_TEXTURE"], Param::Int(0));
    assert_eq!(
        params["g_tMasks"],
        Param::Texture("usps_anodized_masks.vtex".into())
    );
    assert_eq!(params["F_SEPARATE_CHANNEL_INPUTS"], Param::Int(1));
    assert_eq!(params["g_nRandomSeed"], Param::Int(700));
    // No `override_vars` container: the group's step is not done.
    assert!(!params.contains_key("F_ROUGHNESS_PER_COLOR"));
    // A condition passing when false: metalness is not used, so this one is not set.
    assert!(!params.contains_key("F_PEARLESCENCE_MASK"));
    // A texture the recipe does not generate has no steps.
    assert_eq!(recipe.generate("g_tMetalness", &containers), None);
}

#[test]
fn conditions_compare_values_as_written() {
    assert!(Param::Int(1).is("1") && Param::Int(1).is("true"));
    assert!(Param::Int(0).is("false") && !Param::Int(0).is("true"));
    assert!(Param::Floats([0.1, 0.0, 0.0, 0.0]).is("0.1"));
    assert!(!Param::Floats([0.1, 0.0, 0.0, 0.0]).is("0.2"));
    assert!(Param::Texture("masks.vtex".into()).is("masks.vtex_c"));
}

#[test]
fn paint_kits_come_with_their_style_range_and_composite_material() {
    let items = r##"
"items_game"
{
    "paint_kits"
    {
        "180" { "name" "cu_fireserpent_ak47_bravo" "use_legacy_model" "1" "description_tag" "#PaintKit_cu_fireserpent_ak47_bravo_Tag" "style" "7" "wear_remap_max" "0.760000" }
        "1229" { "name" "cu_m4a1s_vaporwave" "style" "7" "composite_material_path" "weapons/paints/community/community_34/m4a1s_vaporwave.vcompmat" "wear_remap_min" "0.00" "wear_remap_max" "0.597321" }
    }
}
"##;
    let kits = paint_kits(items);
    let serpent = &kits[&180];
    assert_eq!((serpent.style, serpent.legacy_model), (Some(7), true));
    assert_eq!(serpent.wear_range, [None, Some(0.76)]);
    assert_eq!(
        serpent.composite_candidates()[0],
        "weapons/paints/legacy/cu_fireserpent_ak47_bravo.vcompmat"
    );
    let vaporwave = &kits[&1229];
    assert!(!vaporwave.legacy_model);
    assert_eq!(
        vaporwave.composite_candidates(),
        ["weapons/paints/community/community_34/m4a1s_vaporwave.vcompmat"]
    );
}
