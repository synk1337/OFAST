"""Debug probe: describe imported actors in /Game/OFAST/Maps/<OFAST_NAME>.

OFAST_FILTER = "<text>"   describe actors whose label contains <text> (bounds, visibility, mesh, materials)
OFAST_FILTER = "@empty"   list every OFAST actor without geometry (zero bounds), with its class
OFAST_FILTER = "@lights"  describe imported lights (direction, intensity, units, radius); OFAST_NEAR = "x,y,z"
                          (Unreal cm) keeps the ones within 15 m of that point

Run with tools/unreal/run_ue.ps1 (-Name <map>).
"""

import os

import unreal

NAME = os.environ["OFAST_NAME"]
FILTER = os.environ.get("OFAST_FILTER", "")
TAG = unreal.Name("OFAST")


def log(message):
    unreal.log("OFAST: " + message)


def list_empty(actors):
    empty = []
    for actor in actors:
        if TAG not in actor.tags or isinstance(actor, unreal.Light):
            continue
        origin, extent = actor.get_actor_bounds(False)
        if extent.x == 0 and extent.y == 0 and extent.z == 0:
            empty.append("%s [%s]" % (actor.get_actor_label(), actor.get_class().get_name()))
    log("%d empty actors" % len(empty))
    for line in empty:
        log("EMPTY " + line)


def list_lights(actors):
    near = os.environ.get("OFAST_NEAR")
    centre = unreal.Vector(*[float(v) for v in near.split(",")]) if near else None
    for actor in actors:
        if TAG not in actor.tags or not isinstance(actor, unreal.Light):
            continue
        location = actor.get_actor_location()
        if centre is not None and (location - centre).length() > 1500:
            continue
        light = actor.light_component
        line = "%s [%s] at (%.0f, %.0f, %.0f) forward %s intensity %.2f" % (
            actor.get_actor_label(),
            actor.get_class().get_name(),
            location.x,
            location.y,
            location.z,
            actor.get_actor_forward_vector(),
            light.intensity,
        )
        for prop in ("intensity_units", "attenuation_radius", "inner_cone_angle", "outer_cone_angle",
                     "source_radius", "cast_shadows", "use_inverse_squared_falloff", "mobility"):
            try:
                line += " %s=%s" % (prop, light.get_editor_property(prop))
            except Exception:
                pass
        log(line)


def describe(actors):
    for actor in actors:
        if FILTER not in actor.get_actor_label():
            continue
        origin, extent = actor.get_actor_bounds(False)
        line = "%s [%s] hidden=%s size=(%.0f, %.0f, %.0f)" % (
            actor.get_actor_label(),
            actor.get_class().get_name(),
            actor.is_hidden_ed(),
            extent.x * 2,
            extent.y * 2,
            extent.z * 2,
        )
        components = actor.get_components_by_class(unreal.StaticMeshComponent)
        for component in components:
            mesh = component.static_mesh
            line += " | component %s mesh=%s" % (component.get_name(), mesh.get_name() if mesh else "None")
            if mesh:
                line += " sections=%d" % mesh.get_num_sections(0)
        log(line)


try:
    unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).load_level("/Game/OFAST/Maps/" + NAME)
    all_actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem).get_all_level_actors()
    if FILTER == "@empty":
        list_empty(all_actors)
    elif FILTER == "@lights":
        list_lights(all_actors)
    else:
        describe(all_actors)
except Exception as error:
    log("FAILED: %s" % error)
