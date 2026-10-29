# Runs a Python script inside the OFAST sandbox Unreal project and prints its "OFAST:" log lines.
#
#   powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/probe_axes.py -Gltf some.gltf
#
# Scripts that render (screenshots) keep the editor alive themselves and quit when done; other scripts run once
# and the editor exits on the next tick.
param(
    [Parameter(Mandatory = $true)][string]$Script,
    [string]$Gltf = "",
    [string]$Shot = "",
    [string]$Name = "",
    [string]$Views = "",
    # For import_map.py: lighting style, "standard" (CS2's look, default) or "realistic" (physical lighting).
    [string]$Style = "",
    # Diagnostics for import_map.py: light sources to leave out, e.g. "sun,sky" (see diagnostic_manifest).
    [string]$Disable = "",
    # For shoot_views.py: only these views, e.g. "ct0,palace".
    [string]$Only = "",
    # For shoot_views.py: what-if settings, best as a .json file (PowerShell strips the quotes of inline JSON).
    [string]$Try = "",
    # For import_demo.py: the replay file written by `ofast export-demo`.
    [string]$Replay = "",
    # For import_cameras.py: the round's cameras file written by `ofast export-demo`; for export_camera.py, the file to
    # write.
    [string]$Cameras = "",
    # For export_camera.py and shoot_sequence.py: the round's sequence, /Game/OFAST/Replays/<demo>_r<round>.
    [string]$Sequence = "",
    # For shoot_sequence.py: demo ticks to screenshot, "153200,154050"; for render_round.py: the range, "start,end".
    [string]$Ticks = "",
    # For render_round.py: preview, final or 4k.
    [string]$Preset = "",
    # For import_demo.py: extra screenshots through players' eyes, "tick:controller,tick:controller".
    [string]$PovShots = "",
    # For import_demo.py: close-ups of players, "tick:controller,tick:controller".
    [string]$CloseShots = "",
    # For import_demo.py and shoot_sequence.py: views from cameras placed as CS2's getpos prints them,
    # "tick:x:y:z:pitch:yaw,...".
    [string]$ViewShots = "",
    # For shoot_sequence.py: actor labels to hide in its shots, "CT Wicadia,T molodoy".
    [string]$Hide = "",
    # Without a renderer (-nullrhi): no GPU memory, so a big round imports where a drawn one may run out of video
    # memory (round 17 peaks at 10.8 GB of an 11 GB budget); nothing can be shot (shoot_sequence.py shoots the saved
    # round afterwards).
    [switch]$NullRhi,
    [string]$Engine = "C:\Program Files\Epic Games\UE_5.8",
    # Another project than the repository's (unreal/Sandbox): an installed Studio's, %LOCALAPPDATA%\OFAST\Unreal.
    [string]$Project = ""
)

$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$editor = Join-Path $Engine "Engine\Binaries\Win64\UnrealEditor-Cmd.exe"
$resolve = { param($p) $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) }
$project = if ($Project) {
    $p = & $resolve $Project
    if (Test-Path -PathType Container $p) { Join-Path $p "OFASTSandbox.uproject" } else { $p }
} else { Join-Path $repo "unreal\Sandbox\OFASTSandbox.uproject" }

$env:OFAST_GLTF = if ($Gltf) { & $resolve $Gltf } else { "" }
$env:OFAST_SHOT = if ($Shot) { & $resolve $Shot } else { "" }
$env:OFAST_NAME = $Name
$env:OFAST_VIEWS = if ($Views) { & $resolve $Views } else { "" }
# Always set (empty by default) so a value from an earlier run in the same shell never leaks into this one.
$env:OFAST_STYLE = $Style
$env:OFAST_DISABLE = $Disable
$env:OFAST_ONLY = $Only
# A .json file is resolved here: Unreal changes its working directory at startup, so relative paths would break.
$env:OFAST_TRY = if ($Try.ToLower().EndsWith(".json")) { & $resolve $Try } else { $Try }
$env:OFAST_REPLAY = if ($Replay) { & $resolve $Replay } else { "" }
$env:OFAST_CAMERAS = if ($Cameras) { & $resolve $Cameras } else { "" }
$env:OFAST_SEQUENCE = $Sequence
$env:OFAST_TICKS = $Ticks
$env:OFAST_PRESET = $Preset
$env:OFAST_POV_SHOTS = $PovShots
$env:OFAST_CLOSE_SHOTS = $CloseShots
$env:OFAST_VIEW_SHOTS = $ViewShots
$env:OFAST_HIDE = $Hide
if ($Shot) {
    # -Shot is either a .png file (single screenshot) or a folder (scripts that take several).
    if ($env:OFAST_SHOT.ToLower().EndsWith(".png")) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $env:OFAST_SHOT) | Out-Null
        if (Test-Path -PathType Leaf $env:OFAST_SHOT) { Remove-Item $env:OFAST_SHOT }
    } else {
        New-Item -ItemType Directory -Force -Path $env:OFAST_SHOT | Out-Null
    }
}

$extra = @(if ($NullRhi) { "-nullrhi" })
& $editor $project "-ExecutePythonScript=$(& $resolve $Script)" -unattended -nosplash -stdout -FullStdOutLogOutput -NoSound @extra 2>&1 |
    Where-Object { $_ -match "OFAST:" }
