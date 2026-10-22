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
    # For import_demo.py: extra screenshots through players' eyes, "tick:controller,tick:controller".
    [string]$PovShots = "",
    # For import_demo.py: close-ups of players, "tick:controller,tick:controller".
    [string]$CloseShots = "",
    # For import_demo.py: views from cameras placed as CS2's getpos prints them, "tick:x:y:z:pitch:yaw,...".
    [string]$ViewShots = "",
    [string]$Engine = "C:\Program Files\Epic Games\UE_5.8"
)

$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$project = Join-Path $repo "unreal\Sandbox\OFASTSandbox.uproject"
$editor = Join-Path $Engine "Engine\Binaries\Win64\UnrealEditor-Cmd.exe"
$resolve = { param($p) $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($p) }

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
$env:OFAST_POV_SHOTS = $PovShots
$env:OFAST_CLOSE_SHOTS = $CloseShots
$env:OFAST_VIEW_SHOTS = $ViewShots
if ($Shot) {
    # -Shot is either a .png file (single screenshot) or a folder (scripts that take several).
    if ($env:OFAST_SHOT.ToLower().EndsWith(".png")) {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $env:OFAST_SHOT) | Out-Null
        if (Test-Path -PathType Leaf $env:OFAST_SHOT) { Remove-Item $env:OFAST_SHOT }
    } else {
        New-Item -ItemType Directory -Force -Path $env:OFAST_SHOT | Out-Null
    }
}

& $editor $project "-ExecutePythonScript=$(& $resolve $Script)" -unattended -nosplash -stdout -FullStdOutLogOutput -NoSound 2>&1 |
    Where-Object { $_ -match "OFAST:" }
