# Imports an OFAST glTF export into the sandbox Unreal project and saves a screenshot.
#
#   powershell -File tools/unreal/run_shot.ps1 -Gltf test-data/exports/ak47/weapon_rif_ak47.gltf -Shot test-data/shots/ak47.png
#
# The Unreal editor opens a window while it works (GPU rendering is needed) and closes itself when done.
# The first run on a machine compiles shaders and can take a long time; later runs are much faster.
param(
    [Parameter(Mandatory = $true)][string]$Gltf,
    [Parameter(Mandatory = $true)][string]$Shot,
    [string]$Name = "",
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
Remove-Item -ErrorAction SilentlyContinue $env:OFAST_SHOT

& $editor $project "-ExecutePythonScript=$script" -unattended -nosplash -stdout -FullStdOutLogOutput -NoSound 2>&1 |
    Where-Object { $_ -match "OFAST:|Error|Fatal" }

if (Test-Path $env:OFAST_SHOT) {
    Write-Output "Screenshot: $env:OFAST_SHOT"
} else {
    Write-Error "No screenshot was written (see the OFAST: lines above and unreal/Sandbox/Saved/Logs)."
}
