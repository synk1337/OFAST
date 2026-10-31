# Saves CS2's frames as fast as it can, Count of them, as <OutDir>\run_NNN.jpg (as grab_window.ps1 does one, but in
# a loop with one setup: about 30 a second at 2560 x 1440), and prints each frame's number and its time in seconds
# since the first: frames of a demo playing in slow motion (slow_shots.py).
#
#     powershell -NoProfile -File tools/cs2/grab_run.ps1 -OutDir C:\path\frames -Count 240
param([Parameter(Mandatory = $true)][string]$OutDir, [int]$Count = 60)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class OfastGrabRun {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@
[OfastGrabRun]::SetProcessDPIAware() | Out-Null
$window = (Get-Process cs2 -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } |
    Select-Object -First 1).MainWindowHandle
if (-not $window) { throw "CS2 is not running" }
$rect = New-Object OfastGrabRun+RECT
[OfastGrabRun]::GetClientRect($window, [ref]$rect) | Out-Null
$width = $rect.R - $rect.L
$height = $rect.B - $rect.T
if ($width -le 0 -or $height -le 0) { throw "CS2's window has no area (minimized?)" }
New-Item -ItemType Directory -Force $OutDir | Out-Null
# JPEG at quality 92: a PNG of the full frame takes longer to write than a frame lasts.
$codec = [System.Drawing.Imaging.ImageCodecInfo]::GetImageEncoders() | Where-Object { $_.MimeType -eq "image/jpeg" }
$quality = New-Object System.Drawing.Imaging.EncoderParameters 1
$quality.Param[0] = New-Object System.Drawing.Imaging.EncoderParameter ([System.Drawing.Imaging.Encoder]::Quality), 92L
$clock = [System.Diagnostics.Stopwatch]::StartNew()
for ($n = 0; $n -lt $Count; $n++) {
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $dc = $graphics.GetHdc()
    $at = $clock.Elapsed.TotalSeconds
    # PW_RENDERFULLCONTENT | PW_CLIENTONLY, as grab_window.ps1.
    [OfastGrabRun]::PrintWindow($window, $dc, 3) | Out-Null
    $graphics.ReleaseHdc($dc)
    $bitmap.Save((Join-Path $OutDir ("run_{0:D3}.jpg" -f $n)), $codec, $quality)
    $graphics.Dispose()
    $bitmap.Dispose()
    [string]::Format([Globalization.CultureInfo]::InvariantCulture, "{0} {1:F3}", $n, $at)
}
