# 028 — Renders as ProRes video, beside the images

- **Date:** 2026-11-08
- **Status:** accepted

## Context

Renders were image sequences only: a folder of PNGs (and an EXR a frame) per pass, thousands of files for a highlight.
The user asked for "an uncompressed video file instead of an image sequence, or even better a choice". Most CS
editors cut in VEGAS Pro (the user too, VEGAS Pro 16 here); others use Premiere, After Effects or Resolve.

What Unreal 5.8's Movie Render Queue can write as video, read from its plugins' code:

- **Apple ProRes** (`AppleProResMedia`, Windows and Mac): .mov, codecs 422 Proxy to 4444 XQ; 4444 and 4444 XQ keep
  4:4:4 colour at up to 12 bits and an alpha channel (`bWriteAlpha`).
- **Avid DNx** (`AvidDNxMedia`, Windows): .mxf, DNxHR, or "uncompressed" without compression, which is 8-bit 4:2:2
  YCbCr (`DNX_UNCOMPRESSED_CCO_YCbYCr`, `CT_UCHAR`) without alpha: half the colour detail of the 8-bit RGB PNGs it
  would replace, and bigger.
- **H.264 MP4** (`MovieRenderPipelineMP4Encoder`): 8-bit, lossy; for sharing, not editing.
- **FFmpeg** (the command-line encoder): needs FFmpeg installed.

## Decision

- **A choice, images or ProRes** (the user's pick, 2026-11-08): `OFAST_OUTPUT` images | prores, the Studio's **As**
  (Images, ProRes video), remembered like the other render settings; images by default.
- **ProRes 4444 XQ**: visually lossless (the format studios master in), 4:4:4 at 12 bits, the players layer's files
  with their alpha, and about the size of the PNGs (1.3 MB a Final frame against about 2 MB for the PNG, de_nuke round
  17). One .mov a pass, every camera cut in it, in the render's folder (`<demo>_r<round>_<pass>.mov`, the folder
  ending in `_prores`); no depth (a video of clipped centimetres), no sound. The project enables Unreal's
  `AppleProResMedia` plugin (the Studio's project template carries it to installed projects with the next version).

## Alternatives considered

- **Unreal's Avid "uncompressed"** — 8-bit 4:2:2 without alpha: less than the PNGs hold.
- **True uncompressed RGB, written by OFAST** (an AVI from the frames): exactly the PNGs' pixels, but about three
  times their size (1.5 GB a second of 4K at 60 frames a second); offered, not picked.
- **H.264 MP4** — lossy; offered for quick sharing, not picked.

## Consequences

- One file to import, and the frame rate is in it: no image sequence settings in the editor.
- ProRes in VEGAS Pro 16 is not checked yet (newer VEGAS Pro, Premiere, After Effects and Resolve read ProRes 4444).
- The players layer's alpha in ProRes is not checked yet in a render (the writer writes it for 4444 and 4444 XQ).
- Unreal's warning "Failed to evaluate camera to create cur/prev camera locations" at a render's first camera cut is
  older than this (in image renders too) and harmless.
