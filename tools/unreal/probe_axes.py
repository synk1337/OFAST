"""Measures how Unreal's glTF importer maps glTF axes to Unreal axes.

Imports a glTF scene (OFAST_GLTF) with Interchange's *scene* import (actors keep their node transforms), then logs
every imported static mesh actor's name, location and size. Used to choose OFAST's glTF axis convention
(docs/decisions/003-axes.md). Run with tools/unreal/run_ue.ps1.
"""

import os

import unreal


def log(message):
    unreal.log("OFAST: " + message)


try:
    levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
    actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
    levels.load_level("/Engine/Maps/Templates/Template_Default")
    before = set(a.get_name() for a in actors.get_all_level_actors())

    source = unreal.InterchangeManager.create_source_data(os.environ["OFAST_GLTF"])
    params = unreal.ImportAssetParameters()
    params.is_automated = True
    params.replace_existing = True
    unreal.InterchangeManager.get_interchange_manager_scripted().import_scene("/Game/OFAST/Probe", source, params)

    for actor in actors.get_all_level_actors():
        if actor.get_name() in before:
            continue
        origin, extent = actor.get_actor_bounds(False)
        log(
            "actor %-24s label %-10s location (%.1f, %.1f, %.1f) size (%.1f, %.1f, %.1f)"
            % (
                actor.get_name(),
                actor.get_actor_label(),
                origin.x,
                origin.y,
                origin.z,
                extent.x * 2,
                extent.y * 2,
                extent.z * 2,
            )
        )
except Exception as error:
    log("FAILED: %s" % error)
