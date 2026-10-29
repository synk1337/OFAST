# OFAST user guide

This guide is written for people who have never used Unreal Engine or modding tools. It grows with every milestone
(see the [roadmap](../roadmap.md)).

OFAST turns a moment of a CS2 demo into an Unreal Engine scene: the map, the players with their agents, weapons and
animations, the grenades, and a camera through each player's eyes. You pick the moment in **OFAST Studio**, and it
builds the round in OFAST's Unreal project for you.

## What you will need

- Counter-Strike 2, installed through Steam
- Unreal Engine 5.8 (free, through the Epic Games Launcher: Unreal Engine → Library → the "+" next to Engine
  Versions → 5.8)
- A graphics card that runs Unreal Engine 5 with Lumen comfortably, and plenty of memory: Unreal alone can take
  15–20 GB while it imports a round
- For now, the OFAST folder itself, with Node.js 22 or newer and the Rust toolchain to start the Studio — see
  [apps/studio/README.md](../../apps/studio/README.md)

OFAST has no installer yet. Once it has one, the Studio comes as a setup program and none of the commands below are
needed: no Node.js, Rust or Visual Studio, only Windows 10 or 11, CS2, Unreal Engine 5.8 and Microsoft Edge WebView2
(already part of Windows 11; the setup program installs it if it is missing).

## Starting the Studio

Until then, in the OFAST folder:

```bash
cd apps/studio
npm install
npm run app
```

The first start compiles the app and takes a few minutes; later starts take seconds.

The first page checks three things, each with a green or red dot:

- **Counter-Strike 2**: found through Steam's list of game libraries. If CS2 is installed somewhere Steam does not
  list, set the environment variable `OFAST_CS2_DIR` to its folder (the one that holds `game\`).
- **Unreal Engine 5.8**: found through the Epic Games Launcher's list. Otherwise set `OFAST_UE_DIR` to its folder.
- **OFAST Unreal project**: `unreal/Sandbox` in the OFAST folder, where rounds are built.

## Finding a moment

Below the checks are your demos: CS2 keeps the ones you download (Watch tab → a match → download) in its
`replays` folder. **Other folder…** lists another folder, for example where you keep tournament demos.

Click a demo to open it. The first time, the Studio reads the whole match (a few seconds); it remembers it after
that. You then see:

- **The rounds**, on the left: who won each one (the dot's colour is the winning side, orange for the terrorists,
  blue for the counter-terrorists), the score after it, and its best moments: **3K**, **4K**, **ACE** (one player
  killed three, four or five opponents), **1vX clutch** (the last player alive on a side won against X opponents).
  A round marked with `*` was played again (a restart), and did not count.
- **The finder**, at the top right, to find a moment worth filming in the whole match:
  - **Highlights**: multi-kills, clutches, collaterals (one bullet, two kills), no-scopes, wallbangs, kills through
    a smoke, in the air, while blind, with a knife. The buttons under the tabs keep one kind.
  - **All kills**: every kill.
  - Both can keep one player's moments (**All players**) and one kind of weapon (**All weapons**: the AWP, snipers,
    pistols, knives…).
  Click a line to open its round with its kills highlighted.
- **The round**, at the bottom right: its kill feed. Times count up from the end of freeze time (CS2's own clock
  counts down from 1:55).

## Watching a round on the radar

**2D replay** in the round's panel replays it on the map's radar: each player as a dot of their side's colour with
a line where they look, the dead as crosses, smokes as grey circles, fires in orange, grenades in flight as small
squares. **Play** / **Pause**, the speed (0.5× to 4×) and the timeline (kills marked in red, the round decided in
green) move through it. Click a kill in the feed to watch it from a few seconds before. On maps with two floors
(Nuke, Vertigo, Train) the players below are drawn faded.

The first replay of a round reads the demo up to that round: a few seconds, more for late rounds of a long demo.

## Choosing the cameras

Each kill in the round's feed has a camera to its right. By default a round is cut kill by kill through each
killer's eyes; for any kill you can pick instead, for its last 4 seconds:

- **Victim's eyes**: what the victim saw coming;
- **Behind the killer**: over his right shoulder, looking where he looks;
- **Around the killer**: circling him slowly at chest height;
- **Fixed camera**: a spot that sees both players at the kill, turning to follow them (when no spot sees both, a
  kill through a wall, it is the camera behind the killer instead);
- **From above**: high over the killer, looking down.

These cameras keep out of walls: one that would end up behind a wall comes in front of it.

Above the feed:

- **First-person view**: *as played* is the player's view exactly as CS2 shows it; *smoothed* and *very smooth*
  take the mouse's jitter out. At every shot the view is still exactly where the player aimed, so the crosshair stays
  on the bullets.
- **HLAE camera…**: a camera path you made in CS2 with HLAE (`mirv_campath save`, an `.xml` file) or a camera
  recording (`mirv_camio`, a `.cam` file) of this demo. The round cuts to it for its whole time.
- **Update cameras**: once the round is in Unreal, changes its cameras without sending it again (about a minute
  instead of ten). Cuts you made by hand in Unreal are replaced; cameras you added stay. A round sent before this
  version of OFAST has to be sent again first.

In Unreal every camera can be cut to, also one you add yourself: each player's own camera hides only that player's
body and shows only their arms and gun, so a camera outside the players always shows them right.

## Sending a round to Unreal

**Send to Unreal** builds the round in OFAST's Unreal project. A bar at the bottom of the window follows it, and
you can keep browsing meanwhile. It:

1. exports the round: every player's movement and animation (CS2's own poses), their agent and weapons with
   their skins (the paint, its pattern and wear, as the demo records them: Fire Serpent, Asiimov, Dopplers, Fades,
   Crimson Web, Gungnir, Royal Guard, case-hardened skins…, the glove skins, 2025's too, the stickers, keychains
   (swinging as the weapon moves), name tags and StatTrak counters, which count up on the round's kills),
   the grenades, the shots;
2. the first time a map is used, exports the map from CS2's files and imports it into Unreal (this can take a long
   time: Unreal compiles its shaders the first time);
3. builds the round in Unreal: a **Level Sequence** in the map's level, with a camera through each player's eyes,
   the cameras you chose, the cuts between them, and markers for the freeze time's end, the kills, the bomb and the
   round's end.

Close Unreal before sending (the import needs the project to itself), and if you can, close CS2 too: Unreal needs a
lot of memory. A round takes about ten minutes the first time.

When it is done, **Open in Unreal** opens the project at the map's level with the round's sequence in Sequencer
(the timeline at the bottom of Unreal's window); the viewport shows the sequence's cameras. Press the space bar
in Sequencer to play it. The sequence is `Content/OFAST/Replays/<demo>_r<round>` in Unreal's Content Browser.

## Rendering a round

Once a round is in Unreal, **Render** (under the round's cameras) renders it to images you import into your editor
(After Effects, Premiere, DaVinci Resolve, Vegas), through the cameras and cuts of its sequence. Pick:

- **Preview**: 1280 × 720, quick, to check the cuts and the timing.
- **Final**: 1920 × 1080 with motion blur, plus what compositing needs: the players on a layer of their own (to put
  text or effects behind them, or to colour them alone), the world without them, and the depth (for fog or depth of
  field).
- **4K**: as Final at 3840 × 2160, about four times as long.

and how much of the round: **around the kills** (from 4 seconds before the first to 2 seconds after the last; if you
opened the round from a highlight in the finder, only that highlight's kills) or **the whole round**. The length is
shown beside it. Close Unreal first (the render needs OFAST's project to itself), and if you can, close CS2: the
render needs the graphics card's memory. Unreal loads the map without a window (a minute or two), then opens one
that shows the frames as they render (closing it stops the render); the bar counts the percent done. Final takes
about 2 seconds a frame on a recent graphics card, at 60 frames a second (a 20-second highlight: 1200 frames, about
40 minutes).

The images go to `Videos\OFAST`, a folder per render, and **Open folder** in the bar shows it. In it, a folder per
pass, each an image sequence your editor imports in one go (pick the first image and tick "image sequence"):

- `FinalImage`: the picture.
- `FinalImagePlayers`: the players alone, the rest transparent (Final and 4K).
- `FinalImageDefaultLayer`: the world, transparent where the players are (Final and 4K).
- `FinalImageWorldDepth`: ignore it; the depth is in the EXR files.

Final and 4K also write an EXR file a frame beside the folders, with all the passes as layers in 16-bit colour, and
the depth in centimetres (in After Effects, read its layers with the EXtractoR effect; the depth is
`FinalImageWorldDepth`, channel R). The [render format page](../formats/render.md) has the details.

The motion blur is part of the Final images; sharp frames for slow motion and speed ramps come later.

In Unreal itself, the same presets are Movie Render Queue settings: `Content/OFAST/Render/OFAST_preview`,
`OFAST_final`, `OFAST_4k` (Window → Cinematics → Movie Render Queue, add the round's sequence, then pick one under
Settings). They exist once the Studio has rendered with them.

## Sending a round again

Sending the same round again rebuilds its sequence; anything you changed in it by hand is replaced. A map's level
holds one round at a time: sending another round of the same map replaces the players and effects of the one before
in the level (its sequence stays, but no longer plays them). If something goes wrong, the bar says what and where
Unreal's own log is; a map whose first import did not finish is imported again the next time.

## Where things are kept

- The Studio's cache (summaries of demos read, radar images) and its exports while a round is being sent (a map can
  take a few gigabytes; they are deleted once imported) are in `%LOCALAPPDATA%\com.ofast.studio`. Deleting the
  folder is safe: the Studio makes them again when needed.
- What is imported into Unreal is in `unreal/Sandbox/Content/OFAST` in the OFAST folder: each map once, and each
  round sent with its players' agents (close to a gigabyte per agent look). `Replays` can be emptied from Unreal's
  Content Browser when it grows.
- Renders are in `Videos\OFAST`, a folder per render, and they are big: Preview about 1 MB a frame (a 20-second
  highlight, 1200 frames: over a gigabyte), Final about 20 MB a frame (the same highlight: about 25 GB), 4K about
  four times that. Delete the folders you no longer need.

Everything OFAST makes comes from your own CS2 install and is for your own videos: never share the exported files
(they are Valve's content).
