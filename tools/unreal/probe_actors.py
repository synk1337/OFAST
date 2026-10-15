"""Debug probe: describe imported actors in /Game/OFAST/Maps/<OFAST_NAME>.

OFAST_FILTER = "<text>"   describe actors whose label contains <text> (bounds, visibility, mesh, materials)
OFAST_FILTER = "@empty"   list every OFAST actor without geometry (zero bounds), with its class

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
    else:
        describe(all_actors)
except Exception as error:
    log("FAILED: %s" % error)
