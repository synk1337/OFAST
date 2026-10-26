# Saves CS2's current frame as a PNG at the window's full resolution, whatever covers the window or holds the focus.
#
#     powershell -NoProfile -File tools/cs2/grab_window.ps1 -Out C:\path\frame.png
#
# PrintWindow with PW_RENDERFULLCONTENT (2) | PW_CLIENTONLY (1) asks the window itself for its DirectX content, so
# other windows over the game do not show (a desktop capture would include them) and no key press is needed (Steam's
# F12 needs the game in front). The demo bar stays in the frame: it shows the tick drawn, two ticks before the one
# `demo_step_tick` reports as paused.
param([Parameter(Mandatory = $true)][string]$Out)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class OfastGrab {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
"@
[OfastGrab]::SetProcessDPIAware() | Out-Null
$window = (Get-Process cs2 -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } |
    Select-Object -First 1).MainWindowHandle
if (-not $window) { throw "CS2 is not running" }
$rect = New-Object OfastGrab+RECT
[OfastGrab]::GetClientRect($window, [ref]$rect) | Out-Null
if ($rect.R - $rect.L -le 0 -or $rect.B - $rect.T -le 0) { throw "CS2's window has no area (minimized?)" }
$bitmap = New-Object System.Drawing.Bitmap ($rect.R - $rect.L), ($rect.B - $rect.T)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
$printed = [OfastGrab]::PrintWindow($window, $dc, 3)
$graphics.ReleaseHdc($dc)
if (-not $printed) {
    $graphics.Dispose()
    $bitmap.Dispose()
    throw "PrintWindow failed: nothing saved"
}
$bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$size = "$($bitmap.Width) x $($bitmap.Height)"
$graphics.Dispose()
$bitmap.Dispose()
"$size -> $Out"
