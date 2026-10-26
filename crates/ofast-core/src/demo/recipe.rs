//! CS2's pose recipe: the AnimGraph2 tasks a player's graph ran on the server each tick, as the demo sends them
//! (decision 020, `docs/formats/demo.md`, "Pose recipe").
//!
//! Each `CCSPlayerPawn` keeps 32 topology slots (`m_animationController.m_SerializePoseRecipeAG2Slots[i].m_topology`,
//! byte blobs) and sends, every tick, which slot its tasks follow (`m_nSerializePoseRecipeAG2ActiveSlot`) and their
//! data (`m_SerializePoseRecipeAG2Dynamic`, a `uint8` vector: its length, then one element per byte) ([`read`]).
//! Both are written the way Esoterica's task serializer writes them (Bobby Anguelov's open engine, whose design
//! AnimGraph2 follows: `Animation_TaskSerializer`, `BitSerialization.h`, `Animation_BoneMaskTask.cpp`; studied, not
//! copied), bits least-significant first:
//!
//! - A topology ([`parse_topology`]): a 4-bit width `D` (at most 8), a `D`-bit task count, then per task its 5-bit
//!   type and its inputs (`D` bits each, indices of earlier tasks). How many inputs a type takes is not written; it
//!   is fixed per type ([`inputs_of`], checked by [`check_topology`]).
//! - The data ([`decode`]): the server tick (`u32`), then each task's payload in the topology's order.
//!
//! Only some payloads are known: samples (type 1: a resource index and a 16-bit normalized time), blends (types 7 and
//! 8: an 8-bit normalized weight, a flag and a bone mask list) and type 0's 6 bits. Payloads follow each other with
//! nothing to mark where one ends, so decoding stops at the first task of another type ([`Decoded::end_bit`]).

use std::fmt;

use super::decoder::FieldValue;
use super::entity::Entity;
use super::fieldpath::FieldPath;
use super::serializer::Serializers;

/// The recipe format this decoder reads (`m_nSerializePoseRecipeVersionAG2`).
pub const RECIPE_VERSION: i64 = 2;

/// Longest recipe data read (bytes); the demos send 32 – 96. A longer length is refused, not allocated.
pub const MAX_RECIPE_BYTES: u64 = 4096;

/// One player's recipe on one tick: the active slot's topology and the tick's task data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoseRecipe {
    /// The format version ([`RECIPE_VERSION`]).
    pub version: i64,
    /// The active slot (0 – 31).
    pub slot: u32,
    /// The active slot's topology bytes.
    pub topology: Vec<u8>,
    /// The tick's data bytes (the server tick first).
    pub data: Vec<u8>,
}

/// Where a pawn keeps its recipe ([`read`]).
#[derive(Debug, Clone)]
pub struct RecipePaths {
    dynamic: FieldPath,
    slots: FieldPath,
    active_slot: FieldPath,
    version: FieldPath,
    /// `m_topology`'s index in the slot struct.
    topology_member: i32,
}

impl RecipePaths {
    /// The paths on `CCSPlayerPawn`, if its serializer has them.
    pub fn find(serializers: &Serializers) -> Option<Self> {
        let pawn = serializers.by_name("CCSPlayerPawn")?;
        let path = |field: &str| {
            serializers.path_of(
                pawn,
                &format!("CBodyComponent.m_animationController.{field}"),
            )
        };
        // The slot struct's own serializer names `m_topology` (its only member in the demos checked).
        let topology_member = serializers
            .by_name("AnimGraph2SerializedPoseRecipeSlot_t")
            .and_then(|s| serializers.path_of(s, "m_topology"))
            .and_then(|p| p.indices().first().copied())
            .unwrap_or(0);
        Some(Self {
            dynamic: path("m_SerializePoseRecipeAG2Dynamic")?,
            slots: path("m_SerializePoseRecipeAG2Slots")?,
            active_slot: path("m_nSerializePoseRecipeAG2ActiveSlot")?,
            version: path("m_nSerializePoseRecipeVersionAG2")?,
            topology_member,
        })
    }
}

/// A pawn's recipe as it stands: `None` without one (a dead player's data is empty; data of 4 bytes or less holds
/// no task), when its length passes [`MAX_RECIPE_BYTES`], or without the active slot's topology.
pub fn read(pawn: &Entity, paths: &RecipePaths) -> Option<PoseRecipe> {
    let length = pawn.get(&paths.dynamic)?.as_u64()?;
    if length <= 4 || length > MAX_RECIPE_BYTES {
        return None;
    }
    let data: Vec<u8> = (0..length as i32)
        .map(|i| {
            paths
                .dynamic
                .child(i)
                .and_then(|p| pawn.get(&p))
                .and_then(FieldValue::as_u64)
                .unwrap_or(0) as u8
        })
        .collect();
    let slot = u32::try_from(pawn.get(&paths.active_slot)?.as_u64()?).ok()?;
    let topology = paths
        .slots
        .child(i32::try_from(slot).ok()?)
        .and_then(|p| p.child(paths.topology_member))
        .and_then(|p| match pawn.get(&p)? {
            FieldValue::Bytes(b) => Some(b.to_vec()),
            _ => None,
        })?;
    let version = pawn
        .get(&paths.version)
        .and_then(|v| v.as_i64().or(v.as_u64().map(|u| u as i64)))
        .unwrap_or(0);
    Some(PoseRecipe {
        version,
        slot,
        topology,
        data,
    })
}

/// A task of a topology: its type and the earlier tasks it takes poses from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyTask {
    /// The 5-bit task type.
    pub kind: u8,
    /// Indices of the tasks whose poses it uses.
    pub inputs: Vec<u8>,
}

/// One entry of a blend's bone mask list (Esoterica's `BoneMaskTask`).
#[derive(Debug, Clone, PartialEq)]
pub enum MaskTask {
    /// Kind 0: one of the graph's bone masks, by index ([`MASK_INDEX_BITS`]).
    Mask(u8),
    /// Kind 1: every bone at one weight (8-bit normalized).
    Generate(f32),
    /// Kind 2: two earlier entries blended by an 8-bit normalized weight.
    Blend(u8, u8, f32),
    /// Kind 3: an earlier entry scaled by an 8-bit normalized weight.
    Scale(u8, f32),
    /// Kind 4: two earlier entries combined.
    Combine(u8, u8),
}

/// A decoded task.
#[derive(Debug, Clone, PartialEq)]
pub enum Task {
    /// Type 1: sample a clip.
    Sample {
        /// Index into the graph's serialization context: not the graph's own resource order, and different from
        /// one CS2 build to another (`docs/formats/demo.md`).
        resource: u16,
        /// Normalized time, 0 – 1.
        time: f32,
    },
    /// Types 7 and 8: blend the poses of the two inputs.
    Blend {
        /// 7 or 8 (8 lays the masked layers over the body).
        kind: u8,
        /// Normalized weight of the second input.
        weight: f32,
        /// The bone mask, if any.
        mask: Vec<MaskTask>,
    },
    /// Type 0: one input and 6 bits whose meaning is not known yet.
    Type0(u8),
}

/// What went wrong reading a recipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipeError {
    /// The topology's bytes ran out.
    TopologyEnds,
    /// The data's bytes ran out.
    DataEnds,
    /// A topology's index width over 8 (Esoterica allows at most 255 tasks).
    Width(u32),
    /// A topology task's input that is not an earlier task.
    BadInput {
        /// The task.
        task: usize,
        /// The input it names.
        input: u32,
    },
    /// A recipe format other than [`RECIPE_VERSION`].
    Version(i64),
    /// A bone mask entry of a kind that does not exist (5 – 7).
    MaskKind(u32),
    /// A bone mask entry naming an entry that is not an earlier one.
    MaskInput {
        /// The entry.
        entry: usize,
        /// The entry it names.
        input: u32,
    },
}

impl fmt::Display for RecipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TopologyEnds => write!(f, "the topology's bytes end early"),
            Self::DataEnds => write!(f, "the data's bytes end early"),
            Self::Width(w) => write!(f, "a topology index width of {w} bits (at most 8)"),
            Self::BadInput { task, input } => {
                write!(f, "task {task} takes task {input}, not an earlier one")
            }
            Self::Version(v) => write!(f, "recipe version {v}, not {RECIPE_VERSION}"),
            Self::MaskKind(k) => write!(f, "a bone mask entry of kind {k}"),
            Self::MaskInput { entry, input } => {
                write!(
                    f,
                    "bone mask entry {entry} takes entry {input}, not an earlier one"
                )
            }
        }
    }
}

impl std::error::Error for RecipeError {}

/// How many inputs a task type takes: the counts with which every topology of a round parses and fits
/// ([`check_topology`]: the 618 different topologies of round 5 of the de_nuke pro demo).
pub fn inputs_of(kind: u8) -> usize {
    match kind {
        7 | 8 | 10 => 2,
        0 | 5 | 14 | 15 => 1,
        _ => 0,
    }
}

/// Bits of a sample's resource index in the demos checked: Esoterica sizes it to the serialization context's
/// resource count (the main graph's clips and graphs, 909 + 67 here), so a CS2 build with more than 1024 would
/// widen it.
const RESOURCE_BITS: usize = 10;

/// Bits of a bone mask index in the demos checked: Esoterica sizes it to the skeleton's bone mask count (8 – 15 for
/// 4 bits), so a CS2 build could change it.
pub const MASK_INDEX_BITS: usize = 4;

/// The value of `width` bits (at most 32) starting at bit `at` of `bytes`, least-significant first; `None` past the
/// end. For looking at the bits where [`decode`] stopped.
pub fn read_bits(bytes: &[u8], at: usize, width: usize) -> Option<u32> {
    (width <= 32 && at + width <= bytes.len() * 8).then(|| {
        (0..width)
            .map(|k| u32::from((bytes[(at + k) / 8] >> ((at + k) % 8)) & 1) << k)
            .sum()
    })
}

/// Least-significant-first bit reader over the topology or the data.
struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
    ends: RecipeError,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8], ends: RecipeError) -> Self {
        Self { bytes, at: 0, ends }
    }

    fn take(&mut self, width: usize) -> Result<u32, RecipeError> {
        let value = read_bits(self.bytes, self.at, width).ok_or_else(|| self.ends.clone())?;
        self.at += width;
        Ok(value)
    }

    fn normalized(&mut self, width: usize) -> Result<f32, RecipeError> {
        Ok(self.take(width)? as f32 / ((1u32 << width) - 1) as f32)
    }
}

fn topology_tasks(bits: &mut Bits) -> Result<Vec<TopologyTask>, RecipeError> {
    let width = bits.take(4)?;
    if width > 8 {
        return Err(RecipeError::Width(width));
    }
    let width = width as usize;
    let count = bits.take(width)? as usize;
    let mut tasks = Vec::with_capacity(count);
    for index in 0..count {
        let kind = bits.take(5)? as u8;
        let mut inputs = Vec::with_capacity(inputs_of(kind));
        for _ in 0..inputs_of(kind) {
            let input = bits.take(width)?;
            if input as usize >= index {
                return Err(RecipeError::BadInput { task: index, input });
            }
            inputs.push(input as u8);
        }
        tasks.push(TopologyTask { kind, inputs });
    }
    Ok(tasks)
}

/// A topology's tasks.
pub fn parse_topology(bytes: &[u8]) -> Result<Vec<TopologyTask>, RecipeError> {
    topology_tasks(&mut Bits::new(bytes, RecipeError::TopologyEnds))
}

/// How well a topology fits [`inputs_of`]: with the right input counts every task but the last feeds a later one,
/// and the bits end in the blob's last byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyFit {
    /// The tasks.
    pub tasks: Vec<TopologyTask>,
    /// Every task but the last is some later task's input.
    pub all_used: bool,
    /// Bits left after the last task (0 – 7 when the input counts are right).
    pub spare_bits: usize,
}

/// A topology's tasks and how well they fit ([`TopologyFit`]).
pub fn check_topology(bytes: &[u8]) -> Result<TopologyFit, RecipeError> {
    let mut bits = Bits::new(bytes, RecipeError::TopologyEnds);
    let tasks = topology_tasks(&mut bits)?;
    let mut used = vec![false; tasks.len()];
    for task in &tasks {
        for &input in &task.inputs {
            used[usize::from(input)] = true;
        }
    }
    let all_used = used.iter().rev().skip(1).all(|&u| u);
    Ok(TopologyFit {
        tasks,
        all_used,
        spare_bits: bytes.len() * 8 - bits.at,
    })
}

/// A bone mask list (Esoterica's `BoneMaskTaskList`): a 5-bit count, then per entry a 3-bit kind and its values;
/// entries name earlier entries with as many bits as the count needs, weights with 8.
fn mask_list(bits: &mut Bits) -> Result<Vec<MaskTask>, RecipeError> {
    let count = bits.take(5)?;
    let index_bits = (32 - count.leading_zeros()).max(1) as usize;
    let mut out: Vec<MaskTask> = Vec::with_capacity(count as usize);
    for entry in 0..count as usize {
        let earlier = |bits: &mut Bits| -> Result<u8, RecipeError> {
            let input = bits.take(index_bits)?;
            if input as usize >= entry {
                return Err(RecipeError::MaskInput { entry, input });
            }
            Ok(input as u8)
        };
        out.push(match bits.take(3)? {
            0 => MaskTask::Mask(bits.take(MASK_INDEX_BITS)? as u8),
            1 => MaskTask::Generate(bits.normalized(8)?),
            2 => MaskTask::Blend(earlier(bits)?, earlier(bits)?, bits.normalized(8)?),
            3 => MaskTask::Scale(earlier(bits)?, bits.normalized(8)?),
            4 => MaskTask::Combine(earlier(bits)?, earlier(bits)?),
            kind => return Err(RecipeError::MaskKind(kind)),
        });
    }
    Ok(out)
}

/// A recipe decoded as far as its payloads are known.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The server tick it was made on.
    pub tick: u32,
    /// The topology's tasks.
    pub topology: Vec<TopologyTask>,
    /// The first tasks' payloads, in order: `topology[tasks.len()]` is the first task not decoded (its payload
    /// unknown), if any.
    pub tasks: Vec<Task>,
    /// The data bit where decoding stopped: the first undecoded task's payload starts there ([`read_bits`]).
    pub end_bit: usize,
}

impl Decoded {
    /// Every task decoded.
    pub fn complete(&self) -> bool {
        self.tasks.len() == self.topology.len()
    }
}

/// A recipe's tick, topology and as many tasks as can be decoded.
pub fn decode(recipe: &PoseRecipe) -> Result<Decoded, RecipeError> {
    if recipe.version != RECIPE_VERSION {
        return Err(RecipeError::Version(recipe.version));
    }
    let topology = parse_topology(&recipe.topology)?;
    let mut bits = Bits::new(&recipe.data, RecipeError::DataEnds);
    let tick = bits.take(32)?;
    let mut tasks = Vec::new();
    for node in &topology {
        tasks.push(match node.kind {
            1 => Task::Sample {
                resource: bits.take(RESOURCE_BITS)? as u16,
                time: bits.normalized(16)?,
            },
            7 | 8 => {
                let weight = bits.normalized(8)?;
                let mask = if bits.take(1)? == 1 {
                    mask_list(&mut bits)?
                } else {
                    Vec::new()
                };
                Task::Blend {
                    kind: node.kind,
                    weight,
                    mask,
                }
            }
            0 => Task::Type0(bits.take(6)? as u8),
            _ => break,
        });
    }
    Ok(Decoded {
        tick,
        topology,
        tasks,
        end_bit: bits.at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Least-significant-first bit writer.
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        at: usize,
    }

    impl Writer {
        fn put(&mut self, value: u32, width: usize) -> &mut Self {
            for k in 0..width {
                if self.at / 8 == self.bytes.len() {
                    self.bytes.push(0);
                }
                self.bytes[self.at / 8] |= (((value >> k) & 1) as u8) << (self.at % 8);
                self.at += 1;
            }
            self
        }
    }

    fn recipe(topology: &Writer, data: &Writer) -> PoseRecipe {
        PoseRecipe {
            version: RECIPE_VERSION,
            slot: 0,
            topology: topology.bytes.clone(),
            data: data.bytes.clone(),
        }
    }

    #[test]
    fn a_blend_space_triangle_decodes() {
        // Topology: sample, sample, blend(0, 1), sample, blend(2, 3), then an aim task (type 14) on 4.
        let mut t = Writer::default();
        t.put(3, 4).put(6, 3);
        t.put(1, 5).put(1, 5);
        t.put(7, 5).put(0, 3).put(1, 3);
        t.put(1, 5);
        t.put(7, 5).put(2, 3).put(3, 3);
        t.put(14, 5).put(4, 3);
        // Data: the tick, three samples at one time, two weights, the second blend with a one-entry mask list.
        let mut d = Writer::default();
        d.put(94045, 32);
        d.put(321, 10).put(16973, 16);
        d.put(141, 10).put(16973, 16);
        d.put(51, 8).put(0, 1);
        d.put(850, 10).put(16973, 16);
        d.put(255, 8).put(1, 1).put(1, 5).put(0, 3).put(0, 4);
        let aim_at = d.at;
        d.put(32768, 16).put(32640, 16);
        let decoded = decode(&recipe(&t, &d)).expect("decodes");
        assert_eq!(decoded.tick, 94045);
        assert_eq!(decoded.topology.len(), 6);
        assert_eq!(decoded.topology[4].inputs, vec![2, 3]);
        assert_eq!(
            decoded.tasks.len(),
            5,
            "the aim task's payload is not known"
        );
        assert!(!decoded.complete());
        assert_eq!(decoded.end_bit, aim_at);
        assert_eq!(read_bits(&d.bytes, decoded.end_bit, 16), Some(32768));
        let resources: Vec<u16> = decoded
            .tasks
            .iter()
            .filter_map(|t| match t {
                Task::Sample { resource, time } => {
                    assert!((time - 0.259).abs() < 1e-3);
                    Some(*resource)
                }
                _ => None,
            })
            .collect();
        assert_eq!(resources, vec![321, 141, 850]);
        assert_eq!(
            decoded.tasks[2],
            Task::Blend {
                kind: 7,
                weight: 0.2,
                mask: Vec::new(),
            }
        );
        assert_eq!(
            decoded.tasks[4],
            Task::Blend {
                kind: 7,
                weight: 1.0,
                mask: vec![MaskTask::Mask(0)],
            }
        );
        let fit = check_topology(&t.bytes).expect("parses");
        assert!(fit.all_used && fit.spare_bits < 8);
    }

    #[test]
    fn mask_lists_and_type_0_stay_aligned() {
        // Topology: sample, type 0 on it, sample, blend(1, 2) with a three-entry mask, sample (the sentinel).
        let mut t = Writer::default();
        t.put(3, 4).put(5, 3);
        t.put(1, 5);
        t.put(0, 5).put(0, 3);
        t.put(1, 5);
        t.put(8, 5).put(1, 3).put(2, 3);
        t.put(1, 5);
        let mut d = Writer::default();
        d.put(7, 32);
        d.put(5, 10).put(0, 16);
        d.put(34, 6);
        d.put(9, 10).put(65535, 16);
        // Three entries: 2-bit indices. Mask 9; Generate 0.5; Blend(0, 1) by 1.0.
        d.put(128, 8).put(1, 1).put(3, 5);
        d.put(0, 3).put(9, 4);
        d.put(1, 3).put(128, 8);
        d.put(2, 3).put(0, 2).put(1, 2).put(255, 8);
        d.put(1000, 10).put(32768, 16);
        let decoded = decode(&recipe(&t, &d)).expect("decodes");
        assert!(decoded.complete());
        assert_eq!(decoded.tasks[1], Task::Type0(34));
        match &decoded.tasks[3] {
            Task::Blend { kind: 8, mask, .. } => {
                assert_eq!(mask[0], MaskTask::Mask(9));
                assert!(matches!(mask[2], MaskTask::Blend(0, 1, w) if w == 1.0));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            decoded.tasks[4],
            Task::Sample { resource: 1000, .. }
        ));
        assert_eq!(decoded.end_bit, d.at);
    }

    #[test]
    fn bad_recipes_are_refused() {
        // An input that is not earlier.
        let mut t = Writer::default();
        t.put(2, 4)
            .put(2, 2)
            .put(1, 5)
            .put(7, 5)
            .put(0, 2)
            .put(1, 2);
        assert_eq!(
            parse_topology(&t.bytes),
            Err(RecipeError::BadInput { task: 1, input: 1 })
        );
        // A width over 8.
        let mut wide = Writer::default();
        wide.put(9, 4).put(0, 12);
        assert_eq!(parse_topology(&wide.bytes), Err(RecipeError::Width(9)));
        // Sample, sample, blend: with a mask entry of kind 6, with data that ends early, in another version.
        let mut t2 = Writer::default();
        t2.put(2, 4)
            .put(3, 2)
            .put(1, 5)
            .put(1, 5)
            .put(7, 5)
            .put(0, 2)
            .put(1, 2);
        let mut d2 = Writer::default();
        d2.put(0, 32).put(0, 26).put(0, 26);
        d2.put(0, 8).put(1, 1).put(1, 5).put(6, 3).put(0, 8);
        assert_eq!(decode(&recipe(&t2, &d2)), Err(RecipeError::MaskKind(6)));
        let mut short = Writer::default();
        short.put(0, 32).put(0, 20);
        assert_eq!(decode(&recipe(&t2, &short)), Err(RecipeError::DataEnds));
        let mut old = recipe(&t2, &d2);
        old.version = 1;
        assert_eq!(decode(&old), Err(RecipeError::Version(1)));
    }
}
