# ofast-capture (planned — M8)

A Rust DLL loaded into CS2 for movie-making: free camera, camera paths, multi-pass recording (color, depth, mattes)
through FFmpeg, and live skeleton capture for exact player animation in Unreal.

## Safety rules (enforced in code)

- Refuses to start unless CS2 was launched with `-insecure` (which blocks every VAC-secured server).
- Offline and local demo playback only. It is a movie tool, not a gameplay tool.

## Design notes

- Field offsets come from CS2's own schema system at runtime, not hardcoded tables, so most game updates do not
  break it.
- Direct3D 11 render hooks, like HLAE's CS2 support.
