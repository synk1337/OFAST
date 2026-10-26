//! Playing CS2's pose recipe (decision 020, `docs/formats/demo.md`, "Pose recipe"): the clips the server's graph
//! sampled, found in the round being exported ([`learn_clip_table`]).
//!
//! The recipe's samples name clips by an index into the graph's serialization context, whose order no rule found
//! gives and which changes with the CS2 build, so the table is learned from the demo itself, round by round: OFAST's
//! own bake re-decides the same graph from the same inputs, so the clips it samples ([`super::trace_start`]) come with
//! CS2's on the same ticks. Each index is matched to the baked clip that comes with it most exclusively (the Jaccard
//! index: the player-ticks they share over the player-ticks either plays), strongest matches first, each index and
//! each clip taken once (the context lists each clip once); a match under [`MIN_JACCARD`] is left out. Only the clips
//! OFAST plays before its aim are traced ([`super::untraced`]): the recipe's decoded part ends at the aim, and
//! co-occurrence alone once matched a shot clip's index (restarted with every bullet of a spray) to the flashed pose,
//! played over the aim, that the same player held then. Clips OFAST never plays where CS2 does stay unknown.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::bake::{GraphRig, LocomotionSets, UpperBody, WeaponSet, bake_player, frame_samples};
use super::layers::{BoneMask, add_layer, model_space_layer, slerp};
use super::locomotion::blend_poses;
use super::{Clip, NmSkeleton, Transform, trace_end, trace_start};
use crate::demo::players::RoundTrack;
use crate::demo::recipe::{self, Decoded, MaskTask, Task};

/// The weakest match kept ([`learn_clip_table`]).
pub const MIN_JACCARD: f32 = 0.3;

/// One round's resource index → clip table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipTable {
    /// Index → (clip path, Jaccard index of the match).
    pub clips: BTreeMap<u16, (String, f32)>,
    /// Sample tasks in the round's recipes, and how many of them name an index in the table.
    pub samples: (usize, usize),
}

impl ClipTable {
    /// The clip path of `index`, if matched.
    pub fn path(&self, index: u16) -> Option<&str> {
        self.clips.get(&index).map(|(path, _)| path.as_str())
    }
}

/// Every clip a bake can sample, by address: the graphs' library, the locomotion sets, the weapons' idles and shots.
fn clip_paths(
    sets: &LocomotionSets,
    upper: Option<&UpperBody>,
    graphs: Option<&GraphRig>,
) -> HashMap<usize, String> {
    let mut out = HashMap::new();
    if let Some(graphs) = graphs {
        for (path, clip) in graphs.library.clips() {
            out.insert(std::ptr::from_ref(clip) as usize, path.to_owned());
        }
    }
    for set in WeaponSet::ALL {
        for (path, clip) in &sets.get(set).clips {
            out.insert(std::ptr::from_ref(clip) as usize, path.clone());
        }
    }
    for w in upper.into_iter().flat_map(|u| u.weapons.values()) {
        let clips = w.idle.iter().flatten().chain(w.shoot.as_ref());
        for (clip, path) in clips.zip(&w.paths) {
            out.insert(std::ptr::from_ref(clip) as usize, path.clone());
        }
    }
    out
}

/// Learns the round's clip table (module docs): every player of `track` baked at the tick rate under a trace, the
/// samples of their recipes ([`crate::demo::players::RigInputs::pose_recipe`]) matched to the clips baked on the same
/// ticks.
pub fn learn_clip_table(
    track: &RoundTrack,
    sets: &LocomotionSets,
    upper: Option<&UpperBody>,
    graphs: Option<&GraphRig>,
) -> ClipTable {
    let paths = clip_paths(sets, upper, graphs);
    let fps = 1.0 / track.tick_interval;
    let frames = frame_samples(track, fps);
    let mut players: Vec<u32> = track
        .samples
        .iter()
        .flat_map(|s| &s.players)
        .map(|p| p.controller)
        .collect();
    players.sort_unstable();
    players.dedup();
    // (tick, controller) → clips baked, and → indices sampled.
    let mut baked: HashMap<(u32, u32), BTreeSet<&str>> = HashMap::new();
    for &controller in &players {
        trace_start();
        let result = bake_player(
            track,
            controller,
            sets,
            upper,
            graphs,
            fps,
            Default::default(),
        );
        let traced = trace_end();
        // A player who does not bake (an error the export would report anyway) adds nothing.
        if result.is_err() {
            continue;
        }
        for (&(tick, _), addresses) in frames.iter().zip(&traced) {
            let clips = addresses
                .iter()
                .filter_map(|a| paths.get(a).map(String::as_str))
                .collect();
            baked.insert((tick.round() as u32, controller), clips);
        }
    }
    let mut together: HashMap<(u16, &str), usize> = HashMap::new();
    let mut index_ticks: HashMap<u16, usize> = HashMap::new();
    let mut clip_ticks: HashMap<&str, usize> = HashMap::new();
    let mut samples: Vec<u16> = Vec::new();
    for sample in &track.samples {
        for p in &sample.players {
            let Some(decoded) = p
                .rig
                .pose_recipe
                .as_ref()
                .and_then(|r| recipe::decode(r).ok())
            else {
                continue;
            };
            let sampled: Vec<u16> = decoded
                .tasks
                .iter()
                .filter_map(|t| match t {
                    Task::Sample { resource, .. } => Some(*resource),
                    _ => None,
                })
                .collect();
            let indices: BTreeSet<u16> = sampled.iter().copied().collect();
            samples.extend(sampled);
            let Some(clips) = baked.get(&(sample.tick, p.controller)) else {
                continue;
            };
            for &clip in clips {
                *clip_ticks.entry(clip).or_default() += 1;
            }
            for &index in &indices {
                *index_ticks.entry(index).or_default() += 1;
                for &clip in clips {
                    *together.entry((index, clip)).or_default() += 1;
                }
            }
        }
    }
    // Strongest matches first (ties by index, then path, for a stable table), each index and clip once.
    let mut matches: Vec<(f32, u16, &str)> = together
        .iter()
        .map(|(&(index, clip), &n)| {
            let either = index_ticks[&index] + clip_ticks[clip] - n;
            (n as f32 / either.max(1) as f32, index, clip)
        })
        .filter(|&(j, ..)| j >= MIN_JACCARD)
        .collect();
    matches.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    let mut table = ClipTable::default();
    let mut taken: BTreeSet<&str> = BTreeSet::new();
    for (j, index, clip) in matches {
        if table.clips.contains_key(&index) || !taken.insert(clip) {
            continue;
        }
        table.clips.insert(index, (clip.to_owned(), j));
    }
    let named = samples
        .iter()
        .filter(|i| table.clips.contains_key(i))
        .count();
    table.samples = (samples.len(), named);
    table
}

/// Where a demo's clips are: every clip a bake can sample, by path.
pub fn clips_by_path<'a>(
    sets: &'a LocomotionSets,
    upper: Option<&'a UpperBody>,
    graphs: Option<&'a GraphRig>,
) -> HashMap<&'a str, &'a Clip> {
    let mut out: HashMap<&str, &Clip> = HashMap::new();
    if let Some(graphs) = graphs {
        out.extend(graphs.library.clips());
    }
    for set in WeaponSet::ALL {
        out.extend(sets.get(set).clips.iter().map(|(p, c)| (p.as_str(), c)));
    }
    for w in upper.into_iter().flat_map(|u| u.weapons.values()) {
        let clips = w.idle.iter().flatten().chain(w.shoot.as_ref());
        out.extend(w.paths.iter().map(String::as_str).zip(clips));
    }
    out
}

/// Longest run of living ticks without a recipe pose ([`fill_gaps`]) bridged between the poses on either side:
/// 8 ticks, an eighth of a second.
pub const RECIPE_GAP: u32 = 8;

/// Bridges each run of at most `max` ticks of `alive` (in order) without a pose in `poses` between the poses on either
/// side (blended by how far in), so a tick the recipe does not give is not a lone frame of another pose.
pub fn fill_gaps(poses: &mut BTreeMap<u32, Vec<Transform>>, alive: &[u32], max: u32) {
    let mut before: Option<(u32, usize)> = None;
    for (k, &tick) in alive.iter().enumerate() {
        if !poses.contains_key(&tick) {
            continue;
        }
        if let Some((start, from)) = before {
            let missing = &alive[from + 1..k];
            // Only a run of consecutive living ticks: a death in between is not bridged.
            if !missing.is_empty()
                && missing.len() as u32 <= max
                && tick - start == missing.len() as u32 + 1
            {
                let (a, b) = (poses[&start].clone(), poses[&tick].clone());
                for &t in missing {
                    let w = (t - start) as f32 / (tick - start) as f32;
                    poses.insert(t, super::bake::mix(a.clone(), b.clone(), w));
                }
            }
        }
        before = Some((tick, k));
    }
}

/// A pose made from a recipe ([`RecipePlayer::pose`]).
#[derive(Debug, Clone, PartialEq)]
pub struct RecipePose {
    /// Local transforms on the clip skeleton, before the aim (the recipe's type 14 and what follows it are not
    /// played: their payloads are not known).
    pub pose: Vec<Transform>,
    /// Sample tasks whose clip the table does not know (left out of their blends).
    pub unknown: usize,
}

/// A task's result.
#[derive(Debug, Clone)]
struct Made {
    pose: Vec<Transform>,
    /// A delta (from an additive clip), to layer over a pose.
    additive: bool,
    /// Something known went into it: a clip the table names (or a cached pose of one), not only the empty pose.
    known: bool,
}

/// Plays one player's recipes, tick after tick (cached poses carry from one tick to the next).
pub struct RecipePlayer<'a> {
    /// Index → clip, from the table.
    clips: HashMap<u16, &'a Clip>,
    /// The skeleton's bone masks, in their order (the recipe names them by index).
    masks: &'a [BoneMask],
    skeleton: &'a NmSkeleton,
    /// Poses kept by ID (type 0), read back by later recipes (type 6).
    cache: HashMap<u8, Made>,
}

impl<'a> RecipePlayer<'a> {
    /// A player over `table`, the clips of `clips` ([`clips_by_path`]), the skeleton's masks in its order and the
    /// skeleton.
    pub fn new(
        table: &ClipTable,
        clips: &HashMap<&'a str, &'a Clip>,
        masks: &'a [BoneMask],
        skeleton: &'a NmSkeleton,
    ) -> Self {
        let clips = table
            .clips
            .iter()
            .filter_map(|(&index, (path, _))| Some((index, *clips.get(path.as_str())?)))
            .collect();
        Self {
            clips,
            masks,
            skeleton,
            cache: HashMap::new(),
        }
    }

    /// A bone mask list's weights per bone: its last entry, built from the earlier ones.
    fn mask(&self, list: &[MaskTask]) -> Option<Vec<f32>> {
        let bones = self.skeleton.bones.len();
        let mut made: Vec<Vec<f32>> = Vec::with_capacity(list.len());
        for entry in list {
            let get = |i: u8| {
                made.get(usize::from(i))
                    .cloned()
                    .unwrap_or_else(|| vec![0.0; bones])
            };
            made.push(match *entry {
                MaskTask::Mask(i) => self
                    .masks
                    .get(usize::from(i))
                    .map_or_else(|| vec![0.0; bones], |m| m.0.clone()),
                MaskTask::Generate(w) => vec![w; bones],
                MaskTask::Blend(a, b, w) => get(a)
                    .iter()
                    .zip(get(b))
                    .map(|(x, y)| x + (y - x) * w)
                    .collect(),
                MaskTask::Scale(a, w) => get(a).iter().map(|x| x * w).collect(),
                MaskTask::Combine(a, b) => {
                    get(a).iter().zip(get(b)).map(|(x, y)| x.max(y)).collect()
                }
            });
        }
        made.pop()
    }

    /// Forgets the cached poses (a player who died: a new life starts afresh).
    pub fn reset(&mut self) {
        self.cache.clear();
    }

    /// Plays every recipe of `controller` in `track`, tick by tick, the cached poses forgotten at each death, and
    /// bridges the short gaps between them ([`fill_gaps`], [`RECIPE_GAP`]): the poses by tick, and the ticks the player
    /// was alive on.
    pub fn play_round(
        &mut self,
        track: &RoundTrack,
        controller: u32,
    ) -> (BTreeMap<u32, Vec<Transform>>, Vec<u32>) {
        let mut poses = BTreeMap::new();
        let mut alive = Vec::new();
        for sample in &track.samples {
            let Some(p) = sample
                .players
                .iter()
                .find(|p| p.controller == controller && p.alive)
            else {
                self.reset();
                continue;
            };
            alive.push(sample.tick);
            let made = p
                .rig
                .pose_recipe
                .as_ref()
                .and_then(|r| recipe::decode(r).ok())
                .and_then(|d| self.pose(&d));
            if let Some(made) = made {
                poses.insert(sample.tick, made.pose);
            }
        }
        fill_gaps(&mut poses, &alive, RECIPE_GAP);
        (poses, alive)
    }

    /// The pose `decoded` gives: its tasks played in order, each blend laying its second input over its first (an
    /// additive one added; type 8 in model space; the others bone by bone), under its bone mask. A sample whose clip
    /// is unknown gives nothing; a blend without its first input gives nothing, without its second gives the first;
    /// a task not decoded (the aim and after) passes its
    /// first input on; a cached pose written from nothing is forgotten. `None` when the last task gives nothing, a
    /// delta, or nothing known went into it (the empty pose alone).
    pub fn pose(&mut self, decoded: &Decoded) -> Option<RecipePose> {
        let mut unknown = 0;
        let mut made: Vec<Option<Made>> = Vec::with_capacity(decoded.topology.len());
        let mut writes: Vec<(u8, Option<Made>)> = Vec::new();
        for (i, node) in decoded.topology.iter().enumerate() {
            let input = |k: usize| {
                node.inputs
                    .get(k)
                    .and_then(|&j| made[usize::from(j)].clone())
            };
            let result: Option<Made> = match decoded.tasks.get(i) {
                Some(Task::Sample { resource, time }) => match self.clips.get(resource) {
                    Some(clip) => clip
                        .sample(time * clip.duration, false)
                        .ok()
                        .map(|pose| Made {
                            pose,
                            additive: clip.is_additive(),
                            known: true,
                        }),
                    None => {
                        unknown += 1;
                        None
                    }
                },
                Some(Task::Blend { kind, weight, mask }) => {
                    let mask = self.mask(mask);
                    match (input(0), input(1)) {
                        (None, _) => None,
                        // The layer left out, whatever its weight: giving nothing instead (the re-decided pose)
                        // fit the hitbox fit's hits worse (364 of 505 within an inch against 371, rule only on
                        // masked layers over a quarter), the layers left out being mostly wrong matches rather
                        // than clips OFAST does not play.
                        (Some(a), None) => Some(a),
                        (Some(mut a), Some(b)) => {
                            let known = a.known || b.known;
                            match (a.additive, b.additive) {
                                (false, true) => {
                                    add_layer(
                                        &mut a.pose,
                                        &b.pose,
                                        *weight,
                                        mask.map(BoneMask).as_ref(),
                                    );
                                }
                                (false, false) if *kind == 8 => {
                                    let mask =
                                        BoneMask(mask.unwrap_or_else(|| vec![1.0; a.pose.len()]));
                                    let parents = &self.skeleton.parents;
                                    model_space_layer(
                                        &mut a.pose,
                                        &b.pose,
                                        *weight,
                                        &mask,
                                        parents,
                                    );
                                }
                                (false, false) | (true, true) => {
                                    a.pose = masked_mix(a.pose, &b.pose, *weight, mask.as_deref());
                                }
                                // A whole pose over a delta: nothing to make of it.
                                (true, false) => {}
                            }
                            (!a.additive || b.additive).then_some(Made { known, ..a })
                        }
                    }
                }
                Some(Task::CachedPoseWrite(id)) => {
                    let pose = input(0);
                    writes.push((*id, pose.clone()));
                    pose
                }
                Some(Task::CachedPoseRead(id)) => self.cache.get(id).cloned(),
                Some(Task::EmptyPose) => Some(Made {
                    pose: self.skeleton.reference.clone(),
                    additive: false,
                    known: false,
                }),
                Some(Task::Raw { .. }) | None => input(0),
            };
            made.push(result);
        }
        // Poses kept this tick are read from the next one on; one written from nothing forgets the ID's older pose.
        for (id, pose) in writes {
            match pose {
                Some(pose) => self.cache.insert(id, pose),
                None => self.cache.remove(&id),
            };
        }
        let last = made.pop().flatten()?;
        (!last.additive && last.known).then_some(RecipePose {
            pose: last.pose,
            unknown,
        })
    }
}

/// `a` towards `b` by `weight`, bone by bone, times the mask's weight for each bone (all bones without one).
fn masked_mix(
    mut a: Vec<Transform>,
    b: &[Transform],
    weight: f32,
    mask: Option<&[f32]>,
) -> Vec<Transform> {
    if mask.is_none() {
        return blend_poses(&[(a, 1.0 - weight), (b.to_vec(), weight)]);
    }
    for (bone, (x, y)) in a.iter_mut().zip(b).enumerate() {
        let w = weight * mask.and_then(|m| m.get(bone)).copied().unwrap_or(0.0);
        if w <= 0.0 {
            continue;
        }
        x.translation =
            std::array::from_fn(|k| x.translation[k] + (y.translation[k] - x.translation[k]) * w);
        x.rotation = slerp(x.rotation, y.rotation, w);
        x.scale += (y.scale - x.scale) * w;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::recipe::TopologyTask;

    /// Two bones; the reference pose moves the second one 10 units up.
    fn skeleton() -> NmSkeleton {
        let mut up = Transform::IDENTITY;
        up.translation = [0.0, 0.0, 10.0];
        NmSkeleton {
            bones: vec!["root".into(), "child".into()],
            parents: vec![None, Some(0)],
            reference: vec![Transform::IDENTITY, up],
        }
    }

    fn decoded(topology: Vec<(u8, Vec<u8>)>, tasks: Vec<Task>) -> Decoded {
        Decoded {
            tick: 0,
            topology: topology
                .into_iter()
                .map(|(kind, inputs)| TopologyTask { kind, inputs })
                .collect(),
            tasks,
            end_bit: 0,
        }
    }

    #[test]
    fn only_known_poses_come_out_and_cached_ones_carry_until_a_reset() {
        let skeleton = skeleton();
        let masks = [BoneMask(vec![0.0, 1.0])];
        let clips = HashMap::new();
        let mut player = RecipePlayer::new(&ClipTable::default(), &clips, &masks, &skeleton);
        let mut moved = skeleton.reference.clone();
        moved[0].translation = [1.0, 2.0, 3.0];
        let known = Made {
            pose: moved.clone(),
            additive: false,
            known: true,
        };
        // An unknown clip blended over the empty pose, kept under ID 3, the aim (not decoded) after it: nothing known
        // went into it, so no pose; the empty pose is still kept.
        let first = decoded(
            vec![
                (13, vec![]),
                (1, vec![]),
                (7, vec![0, 1]),
                (0, vec![2]),
                (14, vec![3]),
            ],
            vec![
                Task::EmptyPose,
                Task::Sample {
                    resource: 5,
                    time: 0.5,
                },
                Task::Blend {
                    kind: 7,
                    weight: 0.5,
                    mask: Vec::new(),
                },
                Task::CachedPoseWrite(3),
            ],
        );
        assert!(player.pose(&first).is_none());
        assert!(player.cache.contains_key(&3));
        // ID 3 written from an unknown clip alone: its older pose is forgotten.
        let unknown_write = decoded(
            vec![(1, vec![]), (0, vec![0])],
            vec![
                Task::Sample {
                    resource: 5,
                    time: 0.0,
                },
                Task::CachedPoseWrite(3),
            ],
        );
        assert!(player.pose(&unknown_write).is_none());
        assert!(!player.cache.contains_key(&3));
        // An unknown layer over a known pose is left out.
        player.cache.insert(4, known.clone());
        let over = decoded(
            vec![(6, vec![]), (1, vec![]), (8, vec![0, 1])],
            vec![
                Task::CachedPoseRead(4),
                Task::Sample {
                    resource: 5,
                    time: 0.0,
                },
                Task::Blend {
                    kind: 8,
                    weight: 1.0,
                    mask: vec![MaskTask::Mask(0)],
                },
            ],
        );
        let layered_over = player.pose(&over).expect("a pose");
        assert_eq!(
            (layered_over.pose, layered_over.unknown),
            (moved.clone(), 1)
        );
        // A blend whose first input is unknown gives nothing, whatever its second.
        let layered = decoded(
            vec![(1, vec![]), (13, vec![]), (8, vec![0, 1])],
            vec![
                Task::Sample {
                    resource: 5,
                    time: 0.0,
                },
                Task::EmptyPose,
                Task::Blend {
                    kind: 8,
                    weight: 1.0,
                    mask: Vec::new(),
                },
            ],
        );
        assert!(player.pose(&layered).is_none());
        // A known pose kept earlier reads back, until the player's cache is reset; an ID never kept gives nothing.
        let read = decoded(vec![(6, vec![])], vec![Task::CachedPoseRead(4)]);
        let made = player.pose(&read).expect("a pose");
        assert_eq!((made.pose, made.unknown), (moved, 0));
        let missing = decoded(vec![(6, vec![])], vec![Task::CachedPoseRead(9)]);
        assert!(player.pose(&missing).is_none());
        player.reset();
        assert!(player.pose(&read).is_none());
    }

    #[test]
    fn masks_weigh_blends_bone_by_bone() {
        let skeleton = skeleton();
        let masks = [BoneMask(vec![0.0, 1.0]), BoneMask(vec![1.0, 0.5])];
        let clips = HashMap::new();
        let player = RecipePlayer::new(&ClipTable::default(), &clips, &masks, &skeleton);
        assert_eq!(player.mask(&[MaskTask::Mask(1)]), Some(vec![1.0, 0.5]));
        assert_eq!(
            player.mask(&[
                MaskTask::Mask(0),
                MaskTask::Mask(1),
                MaskTask::Blend(0, 1, 0.5)
            ]),
            Some(vec![0.5, 0.75])
        );
        assert_eq!(
            player.mask(&[
                MaskTask::Mask(0),
                MaskTask::Mask(1),
                MaskTask::Combine(0, 1)
            ]),
            Some(vec![1.0, 1.0])
        );
        // Mask 0 keeps the root and moves only the child halfway to the other pose.
        let mut moved = skeleton.reference.clone();
        moved[0].translation = [4.0, 0.0, 0.0];
        moved[1].translation = [0.0, 0.0, 20.0];
        let mixed = masked_mix(skeleton.reference.clone(), &moved, 0.5, Some(&[0.0, 1.0]));
        assert_eq!(mixed[0].translation, [0.0, 0.0, 0.0]);
        assert_eq!(mixed[1].translation, [0.0, 0.0, 15.0]);
    }
}
