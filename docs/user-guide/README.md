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
- Windows 10 or 11, with Microsoft Edge WebView2 (part of Windows 11; the setup program installs it if it is
  missing)

## Installing the Studio

Download OFAST Studio's setup program (`…_x64-setup.exe`) from the
[latest release](https://github.com/synk1337/OFAST/releases/latest) and run it. It installs for your Windows user
only (no administrator rights needed) and adds OFAST Studio to the Start menu. The setup program is not signed yet:
Windows SmartScreen may warn about an unknown publisher (**More info → Run anyway**).

The Studio keeps its Unreal project in `%LOCALAPPDATA%\OFAST\Unreal` (made the first time it starts; set the
environment variable `OFAST_PROJECT_DIR` to keep it elsewhere).

The version you are running shows next to the Studio's title (from 1.0.6): **OFAST Studio 1.0.6**, for example.

**Updates** (from 1.0.3): when the Studio starts it checks GitHub for a newer release. If there is one, a bar under
its title says so: **What's new** shows the release notes, **Update** downloads it, checks it was signed by OFAST's
release, installs it and restarts the Studio (not while a round is being sent or rendered: that would stop it), and
**Later** hides the bar until the next start. Without internet nothing shows. Running a newer setup program over the
Studio by hand works too. Either way the maps and rounds already sent stay.

From the source code instead (to work on OFAST: Node.js 22 or newer and the Rust toolchain, see
[apps/studio/README.md](../../apps/studio/README.md)), in the OFAST folder:

```bash
cd apps/studio
npm install
npm run app
```

The first start compiles the app and takes a few minutes; later starts take seconds. Run this way, the Studio uses
the OFAST folder's own Unreal project, `unreal/Sandbox`.

## Starting the Studio

The first page checks four things, each with a green or red dot:

- **Counter-Strike 2**: found through Steam's list of game libraries. If CS2 is installed somewhere Steam does not
  list, set the environment variable `OFAST_CS2_DIR` to its folder (the one that holds `game\`).
- **CS2 version**: green when OFAST was checked on your CS2's version. Orange when CS2 updated since: OFAST still
  runs, but the update may have changed something it reads (a broken first person, a weapon missing). An OFAST
  update for the new CS2 version turns it green again.
- **Unreal Engine 5.8**: found through the Epic Games Launcher's list. Otherwise set `OFAST_UE_DIR` to its folder.
- **OFAST Unreal project**: where rounds are built: `%LOCALAPPDATA%\OFAST\Unreal` for an installed Studio,
  `unreal/Sandbox` in the OFAST folder when it runs from the source code.

## Finding a moment

Below the checks are your demos: CS2 keeps the ones you download (Watch tab → a match → download) in its
`replays` folder. **Other folder…** lists another folder, for example where you keep tournament demos. Demos you
download or copy there while the Studio is open show up by themselves within a few seconds.

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

A round is about one player: **Point of view**, above the feed, picks whose. The round is seen through their eyes
from its start to 2 seconds after their death (to the end if they live), and Unreal's sequence stops there. OFAST
picks the player of the highlight you clicked in the finder, or else the one with the most kills in the round. Pick
**Each kill's killer** to have the round cut kill by kill through each killer's eyes instead.

Each of that player's kills has a camera to its right (with *Each kill's killer*, every kill has one). By default it
is their eyes; for any kill you can pick instead, for its last 4 seconds:

- **Victim's eyes**: what the victim saw coming;
- **Behind the killer**: over his right shoulder, looking where he looks; in tight places (a stairwell, his back
  to a wall) it rises over his head instead of pressing against it;
- **Around the killer**: circling him slowly at chest height;
- **Fixed camera**: a spot that sees both players at the kill, turning to follow them, and that neither player
  walks right past during the shot (when no spot sees both, a kill through a wall, it is the camera behind the
  killer instead);
- **From above**: high over the killer, looking down.

These cameras keep out of walls: one that would end up behind a wall comes in front of it.

Above the feed:

- **First-person view**: *as played* is the player's view exactly as CS2 shows it; *smoothed* and *very smooth*
  take the mouse's jitter out. At every shot the view is still exactly where the player aimed, so the crosshair stays
  on the bullets. The gun in hand moves as CS2's does: it kicks with the recoil, bobs and sits lower while the player
  runs, sinks while they fall, and trails a little behind fast turns (from 1.0.6; send a round again to get it). How
  it is drawn is set in Unreal's Viewmodel panel (below).
- **HLAE camera…**: a camera path you made in CS2 with HLAE (`mirv_campath save`, an `.xml` file) or a camera
  recording (`mirv_camio`, a `.cam` file) of this demo. The round cuts to it for its whole time.
- **Update cameras**: once the round is in Unreal, changes its cameras without sending it again (about a minute
  instead of ten). Cuts you made by hand in Unreal are replaced; cameras you added stay. A round sent before this
  version of OFAST has to be sent again first.

In Unreal every camera can be cut to, also one you add yourself: each player's own camera hides only that player's
body and shows only their arms and gun, so a camera outside the players always shows them right.

## Sending a round to Unreal

**Send to Unreal** builds the round in OFAST's Unreal project. A bar at the bottom of the window follows it, with a
progress bar and how much is done (also on the Studio's button in the taskbar), and you can keep browsing meanwhile,
or do something else: when it is done (or failed), the Studio plays a sound, and its taskbar button flashes if its
window is behind others. The progress is an estimate: each step takes a share of the bar as long as it usually takes,
and the bar moves on as the step says it goes (Unreal's start, the players' files, the map's textures); a map's first
import can take much longer than its share. It:

1. exports the round: every player's movement and animation (CS2's own poses, their feet on the steps and slopes
   under them), their agent and weapons with
   their skins (the paint, its pattern and wear, as the demo records them: Fire Serpent, Asiimov, Dopplers, Fades,
   Crimson Web, Gungnir, Royal Guard, case-hardened skins…, the glove skins, 2025's too, the stickers, keychains
   (swinging as the weapon moves), name tags and StatTrak counters, which count up on the round's kills),
   the grenades, the shots;
2. the first time a map is used, exports the map from CS2's files and imports it into Unreal (this can take a long
   time: Unreal compiles its shaders the first time); a new OFAST that draws maps better imports each map again
   once, the first time you send a round on it;
3. builds the round in Unreal: a **Level Sequence** in the map's level, with a camera through each player's eyes,
   the cameras you chose, the cuts between them, and markers for the freeze time's end, the kills, the bomb and the
   round's end.

The round in Unreal starts 3 seconds before the freeze time ends (the rest of it, players standing at their spawn,
is left out; those 3 seconds leave room for a cinematic) and runs until right before the players respawn for the next
round: the seconds after the round is decided are in it, the match's last round included. (A round about one player
still ends 2 seconds after their death, if they die.)

Close Unreal before sending (the import needs the project to itself), and if you can, close CS2 too: Unreal needs a
lot of memory. A round takes about ten minutes the first time.

The select beside **Send to Unreal** picks the map's look:

- **CS2's look**: the picture CS2 shows, its sun, sky, lights and colours.
- **Realistic**: the map lit like the real world (a real sun and sky, real light levels, a camera's exposure), with an
  ambiance: a warm haze the sun's beams show in through doorways and roofs, light shafts round the sun, a little film
  grain, and depth of field on the players' eyes, which focus on what they look at. The first round you send in
  this look on a map imports that map again in it (a while, once per map); after that rounds take as long as in
  CS2's look. A round is in one look at a time: sending it in the other look builds it again there.

The Studio remembers the look you picked.

When it is done, **Open in Unreal** opens the project at the map's level with the round's sequence in Sequencer
(the timeline at the bottom of Unreal's window); the viewport shows the sequence's cameras. Press the space bar
in Sequencer to play it. The sequence is `Content/OFAST/Replays/<demo>_r<round>` in Unreal's Content Browser.

### The Look panel

**Open in Unreal** also opens the **Look** panel, a window of sliders for the map's look; you can dock it anywhere
in Unreal's window. It is in the menu bar too: **OFAST → Look**. Drag a slider and the viewport changes at once:

- **Sun**: its height (raise it, or lower it for a golden hour: in the realistic look the sky follows), its
  direction round the map, its strength, its colour (warmer or cooler), and how bright the shade is.
- **Atmosphere** (realistic look): how thick the haze is, and how strongly the sun's beams show in it.
- **Camera**: brightness (in camera stops: +1 is twice as bright), depth of field (blur away from what the player
  looks at, through their eyes: 0 is everything sharp, 0.8 a strong cinematic blur of the map behind what they look
  at; the gun stays sharp), motion blur. How wide their view is: **View field of view** in the Viewmodel panel.
  Tick **Blur with distance** for the blur of a depth pass in After Effects instead: the gun and what is near stay
  sharp, and the map blurs more the further away it is, whatever the player looks at (what they aim at far away
  blurs too). **Depth of field** then sets how blurred the far map gets, and **Blur distance** how far away it is half
  that (20 m at first; nearer blurs more of the map). It is for the players' eyes when the round has no focus keys
  (below, *Depth of field over the round*). **Scope overlay** (ticked at first): through a player's eyes, while they
  look through a scope, CS2's scope over the view: the AWP's, SSG 08's and auto-snipers' black ring with its thin
  lines, and through the AUG's and SG 553's sight, brought up to the eye, the blur around it and its green dot. Untick
  it to keep only the zoom and add your own scope in editing. The zoom itself (40 then 10 for the AWP, 45 for the
  AUG…), the gun hidden behind a sniper's scope and the AUG's sight at the eye are always as in CS2.
- **Colour and film**: warm or cool, saturation (0 is black and white), contrast, vignette, film grain; in the
  realistic look also bloom and lens flare.
- **Reset everything** puts every slider back to the map's own look. Each slider's own reset arrow puts it back to
  its neutral value.

Each map level (CS2's look and the realistic one apart) keeps its own settings, for every round you send there; the
renders and the next **Open in Unreal** use them, even if you did not save the level. They are kept in the Unreal
project's `Saved/OFAST/Looks` folder.

### The gun in first person: the Viewmodel panel

Every player's gun is drawn with CS2's default viewmodel (field of view 68, offsets 2.5, 0, −1.5), so a movie made of
several players' views looks like one game. **Open in Unreal** also opens the **Viewmodel** panel (**OFAST →
Viewmodel**) to change it, for every round of the project; the viewport changes a moment after you let go of a
slider:

- **View field of view**: how wide the view through the players' eyes is (the POV shots), in CS2's degrees: 90 is
  CS2's, more shows more of the map, less zooms in. The gun keeps its own (**Gun field of view**, below).
- **Viewmodel**: **Same for everyone** (the settings below) or **Each player's own** (what each player had set:
  their gun as they saw it while playing).

The gun is drawn as CS2 draws it in a game. CS2's demo playback (and so HLAE) shows guns a little smaller and further
away than that, whatever the settings.
- **Gun field of view**: smaller makes the gun bigger (CS2's `viewmodel_fov`: 54 to 68); the view stays as it is.
- **Offset X (right)**, **Offset Y (forward)**, **Offset Z (up)**: where the gun sits (CS2's `viewmodel_offset_x`,
  `_y`, `_z`).
- **Hand**: **As played** (some players play left-handed), or **Right** or **Left** for everyone.
- **Gun motion**: how much the gun bobs, sits lower and trails behind the player's moves; 1 is CS2's, 0 a still gun.
- **CS2's default** puts everything back.

The settings are kept in the Unreal project's `Saved/OFAST/Viewmodel.json`; renders and rounds sent later use them.
A round sent before this version has to be sent again first.

## Making your own cinematic shots

The **Cameras** panel opens with **Open in Unreal**, beside the Look panel (also **OFAST → Cameras** in the menu bar).
It makes a camera move as HLAE's campath does, without knowing Unreal's cameras: you put the camera where it should
be at a few moments of the round, and OFAST makes a smooth move through them and cuts to it in the round.

1. In Sequencer (the timeline under the viewport), move the playhead to where the shot starts.
2. Fly the view there: hold the right mouse button and use **W A S D** (forward, left, back, right), **Q** and **E**
   (down, up), the mouse to turn; the mouse wheel while flying sets the speed.
3. **New cinematic**: a camera ("Cinematic 1") where the view is, its first point at the playhead. From now on the
   viewport is that camera (its name shows at the viewport's top; the eject button beside it lets go of it).
4. Move the playhead on (a second, two), fly the camera to where it should be then, **Add point here**. Again for
   each point. A point at the playhead's moment is replaced; **Earlier point** and **Later point** jump between them,
   **Remove point here** takes one out. **Field of view** is how wide the camera sees at the next point you add (less
   zooms in); between points it changes smoothly.
5. **Preview** plays the round through its cuts from a second before your shot (Space stops it); **Fly the camera**
   takes the camera again to change it.

From two points on, the round cuts to your camera from its first point to its last, and back to the round's own
cameras after. Under **Players**, each cinematic can:

- **Keep in frame** a player: the camera turns to keep them in the middle of the picture (where it looks at each
  point no longer matters, only where it is).
- **Follow** a player: the camera moves with them, as a drone would, smoothly (it does not jump when they jump); each
  point is kept as a place round the player.
- **Focus on them**: the player kept in frame (or followed) is sharp and the rest blurred; **Depth of field** sets how
  strongly.

Several cinematics can be made (pick one in **Cinematic**); **Remove cinematic** takes one out. The **Points** box
says how many points it has, how long its cut is and where the playhead is among them. **Render** in the Studio
renders the cinematics with the round, at the frame rate you pick.

### Depth of field over the round

Under **Depth of field** in the same panel, you set the camera blur for the whole round, changing over time like your
points, on every camera (first person, the round's other cameras and your cinematics):

1. Put the playhead at a moment, pick **Focus on**: **Where the player looks** (in first person, what they aim at;
   a cinematic keeps its own focus), a player (sharp on them wherever they go), or **A distance** (in metres from the
   camera, **Distance (m)**). Set the **Blur strength** (0 about everything sharp, 0.8 a strong cinematic blur).
2. **Add focus key here**. Move on in time, change the settings, add the next key.

Between two keys the focus glides from one to the other (a focus pull from one player to another, from the gun to
the street behind) and the blur strength with it; before the first key and after the last, that key holds. With the
playhead on a key, its settings show in the panel and changing them changes it. **Earlier focus key** and **Later
focus key** jump between them; **Remove focus key here** takes one out. Without any key, each camera keeps its own
depth of field (the Look panel's slider and its **Blur with distance**, a cinematic's **Focus on them**); with keys,
the players' eyes focus as the keys say, never blurred by distance.

Two limits apply to the whole round: **Foreground blur limit**, how blurred things in front of the focus can get (0.05
keeps the gun sharp in first person; raise it to blur the gun, or what passes close to a cinematic camera), and
**Background blur limit**, how blurred things behind it can get (2.5 is Unreal's usual; 0 keeps the background sharp:
no depth of field on the map at all). Both are in % of the picture's width.

The points and focus keys are kept in a file for each round (`Saved/OFAST/Cinematics` in the Unreal project), so they
come back if you send the round to Unreal again or change its cameras, and the render uses them even if you closed
Unreal without saving. The panel makes the camera's keys from its points each time they change: keys changed by hand in Sequencer
are replaced at the next change.

## Rendering a round

Once a round is in Unreal, **Render** (under the round's cameras) renders it to images or a video you import into your
editor (Vegas, After Effects, Premiere, DaVinci Resolve), through the cameras and cuts of its sequence. Pick the
size: **720p** (1280 × 720), **1080p** (1920 × 1080), **2K** (2560 × 1440) or **4K** (3840 × 2160); every size is
rendered with the same quality (8 samples a frame, motion blur or sharp: see below, and the depth for fog or depth of
field), so 720p is simply the quickest, to check the cuts and the timing. Then how much of the round: **around the
kills** (from 4 seconds before the first to 2 seconds after the last; if you opened the round from a highlight in the
finder, only that highlight's kills) or **the whole round**. The length is shown beside it. Close Unreal first (the
render needs OFAST's project to itself), and if you can, close CS2 and games: the render needs the graphics card's
memory. Unreal loads the map without a window (a minute or two), then opens one that shows the frames as they render
(closing it stops the render); the bar counts the percent done, and the sound plays at the end, as for a sending.
1080p takes about a second a frame on a recent graphics card at full power (about 2 with the players layer), at 60
frames a second (a 20-second highlight: 1200 frames, about 20 minutes); 2K and 4K about 2 seconds on a 12 GB card.

**Players layer** (off by default) adds what compositing needs: the players on a layer of their own (to put text or
effects behind them, or to colour them alone) and the world without them. Each is one more render of every frame, so
it takes about three times the graphics memory, and longer.

Every render keeps its size, but Unreal shades the frames smaller when your graphics card's memory would not hold
them, and its TSR scales them up from the 8 samples of each frame: on a 12 GB card, 720p and 1080p are shaded whole,
2K at 85 % and 4K at 50 % (1920 × 1080); on 16 GB every size is whole, on 24 GB everything. The bar says so when it
happens. A render too big for the card spills into the PC's memory, which makes every frame slow and the whole PC
with it; the bar warns about that too: the players layer on a 12 GB card (it needs 16 GB), any render on 8 GB. The
players layer also takes 18 to 25 GB of the PC's own memory: close games and other big programs before such a render.

**As** says what the render writes: **Images** (a PNG a frame, and an EXR a frame with the depth), or **ProRes
video**: one Apple ProRes 4444 file (.mov) with every cut in it, visually lossless, a tenth of the images' size (it
has no EXR or depth), the players layer as two more videos with their transparency. Import the .mov like any video;
VEGAS Pro, Premiere, After Effects and Resolve read it.

**Power** says how much of the PC the render may take: **Full** as fast as it goes; **Balanced** (the default) rests
the graphics card now and then and runs Unreal below your other programs, so the PC stays usable while it renders
(about 15 % longer); **Low** rests the card as long as it works (a third less power, cooler and quieter, about 1.7
times as long). The frames are the same at any power.

The **fps** box beside it is the frames a second: 60 plays at normal speed. For slow motion made in your editing
software (Vegas, Premiere, After Effects), render more, 250 for example: slowed to a quarter, the clip still has over
60 frames a second, each a real picture of its own moment, none blended from its neighbours. Type any number from 10
to 1000, or pick a common one from its list; the Studio remembers it. The frame count beside it grows in proportion,
and so does the render's time (that 20-second highlight at 250: 5000 frames, about 1.5 hours in 1080p). Import the
images at the frame rate you rendered (in Vegas: the frame rate in the image sequence's import dialog, or its Media
Properties; in Premiere: Clip → Modify → Interpret Footage), then slow the clip down there.

The images go to `Videos\OFAST`, a folder per render, and **Open folder** in the bar shows it. In it, a folder per
pass, each an image sequence your editor imports in one go (pick the first image and tick "image sequence"):

- `FinalImage`: the picture.
- `FinalImagePlayers`: the players alone, the rest transparent (with the players layer).
- `FinalImageDefaultLayer`: the world, transparent where the players are (with the players layer).
- `FinalImageWorldDepth`: ignore it; the depth is in the EXR files.

Images also come as an EXR file a frame beside the folders, with all the passes as layers in 16-bit colour, and
the depth in centimetres (in After Effects, read its layers with the EXtractoR effect; the depth is
`FinalImageWorldDepth`, channel R). The [render format page](../formats/render.md) has the details.

**Motion blur** (ticked by default) blurs what moves, as a camera does. For a velocity edit, untick it: every frame
comes out sharp, a picture of its exact moment. Ramp the speed in your editing software, then add the blur there
(frame blending, RSMB, Twixtor): a blur rendered in at normal speed would smear the slowed parts and be too light in
the fast ones. With 250 fps or more it gives what HLAE gave for this. A sharp render goes to its own folder (ending in
`_sharp`), beside a blurred one of the same moment.

In Unreal itself, the same sizes are Movie Render Queue settings: `Content/OFAST/Render/OFAST_720p`, `OFAST_1080p`,
`OFAST_2k`, `OFAST_4k` (Window → Cinematics → Movie Render Queue, add the round's sequence, then pick one under
Settings). They exist once the Studio has rendered with them.

## Sending a round again

Sending the same round again rebuilds its sequence; anything you changed in it by hand is replaced, except the
Look panel's settings and the Cameras panel's cinematics, which are made again from their files. A map's level
holds one round at a time: sending another round of the same map replaces the players and effects of the one before
in the level (its sequence stays, but no longer plays them). If something goes wrong, the bar says what and where
Unreal's own log is; a map whose first import did not finish is imported again the next time.

## Where things are kept

- The Studio's cache (summaries of demos read, radar images) and its exports while a round is being sent (a map can
  take a few gigabytes; they are deleted once imported) are in `%LOCALAPPDATA%\com.ofast.studio`. Deleting the
  folder is safe: the Studio makes them again when needed.
- What is imported into Unreal is in the project's `Content/OFAST` (`%LOCALAPPDATA%\OFAST\Unreal\Content\OFAST`,
  or `unreal/Sandbox/Content/OFAST` from the source code): each map once, and each round sent with its players'
  looks (their textures shared). `Replays` can be emptied from Unreal's Content Browser when it grows.
- The panels' settings are in the project's `Saved/OFAST`: `Looks` (each map's look), `Cinematics` (each round's
  points and focus keys), `Viewmodel.json` (the gun in first person) and `Viewmodels` (each round's data for it).
- Renders are in `Videos\OFAST`, a folder per render, and they are big as images: 720p about 5 MB a frame, 1080p
  about 12 MB (a 20-second highlight, 1200 frames: about 14 GB), 2K about 16 MB, 4K about 35 MB (a minute of 4K is
  over 100 GB); the players layer more than doubles it. As ProRes video, about a tenth of that. The bar warns when a
  render will not fit on the disk. Delete the folders you no longer need.

Everything OFAST makes comes from your own CS2 install and is for your own videos: never share the exported files
(they are Valve's content).
