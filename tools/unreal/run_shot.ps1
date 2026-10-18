# Imports an OFAST glTF export into the sandbox Unreal project and saves a screenshot.
#
#   powershell -File tools/unreal/run_shot.ps1 -Gltf test-data/exports/ak47/weapon_rif_ak47.gltf -Shot test-data/shots/ak47.png
#
# An agent exported with clips (`ofast export-model ... --clip ...`) can be posed with one of its animations, one
# screenshot per time (<shot>_<time>.png):
#
#   powershell -File tools/unreal/run_shot.ps1 -Gltf test-data/exports/agents/ctm_sas_anim/ctm_sas.gltf `
#       -Shot test-data/shots/agents/ctm_sas_run.png -Anim run_n_rifle -AnimTimes 0,0.2,0.4
#
# -Camera "x,y,z,pitch,yaw,fov" replaces the automatic framing with a camera at (x, y, z) cm from the mesh's origin
# (Unreal axes), its angles and horizontal field of view: to see an agent from where a CS2 player saw one.
#
# The Unreal editor opens a window while it works (GPU rendering is needed) and closes itself when done.
# The first run on a machine compiles shaders and can take a long time; later runs are much faster.
param(
    [Parameter(Mandatory = $true)][string]$Gltf,
    [Parameter(Mandatory = $true)][string]$Shot,
    [string]$Name = "",
    [string]$Anim = "",
    [string]$AnimTimes = "0",
    [string]$Camera = "",
    [string]$Engine = "C:\Program Files\Epic Games\UE_5.8"
)

$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$project = Join-Path $repo "unreal\Sandbox\OFASTSandbox.uproject"
$script = Join-Path $PSScriptRoot "import_and_shot.py"
$editor = Join-Path $Engine "Engine\Binaries\Win64\UnrealEditor-Cmd.exe"

$env:OFAST_GLTF = (Resolve-Path $Gltf).Path
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Shot) | Out-Null
# Resolve against PowerShell's location (not the .NET process directory, which Set-Location does not change).
$env:OFAST_SHOT = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Shot)
$env:OFAST_NAME = $Name
$env:OFAST_ANIM = $Anim
$env:OFAST_ANIM_TIMES = $AnimTimes
$env:OFAST_CAMERA = $Camera
Remove-Item -ErrorAction SilentlyContinue $env:OFAST_SHOT
# With an animation, one screenshot per time: <shot>_<time>.png (older ones removed first).
$base = [System.IO.Path]::ChangeExtension($env:OFAST_SHOT, $null).TrimEnd(".")
if ($Anim) { Remove-Item -ErrorAction SilentlyContinue "$($base)_*.png" }

& $editor $project "-ExecutePythonScript=$script" -unattended -nosplash -stdout -FullStdOutLogOutput -NoSound 2>&1 |
    Where-Object { $_ -match "OFAST:|Error|Fatal" }

if ($Anim) {
    $shots = @(Get-ChildItem -ErrorAction SilentlyContinue "$($base)_*.png")
    if ($shots.Count -gt 0) {
        $shots | ForEach-Object { Write-Output "Screenshot: $($_.FullName)" }
    } else {
        Write-Error "No screenshot was written (see the OFAST: lines above and unreal/Sandbox/Saved/Logs)."
    }
} elseif (Test-Path $env:OFAST_SHOT) {
    Write-Output "Screenshot: $env:OFAST_SHOT"
} else {
    Write-Error "No screenshot was written (see the OFAST: lines above and unreal/Sandbox/Saved/Logs)."
}
