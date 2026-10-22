//! Field paths: which networked fields an entity update touches.
//!
//! An entity's networked state is a tree: the class serializer has fields, some fields are nested structs
//! (pointers, embedded vectors), and those have fields of their own. A **field path** addresses one leaf of that tree
//! as a list of up to 7 indices, e.g. `[12, 3]` = "field 3 of the struct in field 12".
//!
//! An entity update first lists every field path it changes, then the new values (see `entity.rs`). The path list
//! is delta-compressed: starting from `[-1]`, each step is one of 40 **operations** ("add one to the last index",
//! "go one level deeper", "pop back to the top level and add N", …), and each operation is written with a
//! **Huffman code** — frequent operations get short codes. The operation `FieldPathEncodeFinish` ends the list.
//!
//! The Huffman tree is not stored in the demo; both sides build it from fixed operation weights (below) with a fixed
//! tie-breaking rule. Knowledge source: demoinfocs-golang `sendtables/field_path.go` + `huffman.go` and demoparser2
//! `second_pass/path_ops.rs` (both MIT). See `docs/formats/demo.md`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::OnceLock;

use super::bits::{BitReader, BitsEof, BitsResult};

/// Maximum depth of a field path.
pub const MAX_DEPTH: usize = 7;

/// A position in an entity's field tree: `len` indices, the first `len` entries of `indices` are meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FieldPath {
    indices: [i32; MAX_DEPTH],
    len: u8,
}

impl FieldPath {
    /// Builds a path from its indices (1 to [`MAX_DEPTH`] of them).
    pub fn new(indices: &[i32]) -> Self {
        assert!(
            (1..=MAX_DEPTH).contains(&indices.len()),
            "a field path has 1 to {MAX_DEPTH} indices"
        );
        let mut path = Self {
            indices: [0; MAX_DEPTH],
            len: indices.len() as u8,
        };
        path.indices[..indices.len()].copy_from_slice(indices);
        path
    }

    /// The indices, outermost first.
    pub fn indices(&self) -> &[i32] {
        &self.indices[..self.len as usize]
    }

    /// The path with only its first `len` indices.
    pub fn prefix(&self, len: usize) -> Self {
        let mut p = *self;
        p.indices[len..].fill(0);
        p.len = len as u8;
        p
    }

    /// The path one level deeper: element `index` of the array at this path (`None` past [`MAX_DEPTH`]).
    pub fn child(&self, index: i32) -> Option<Self> {
        let len = self.len as usize;
        (len < MAX_DEPTH).then(|| {
            let mut p = *self;
            p.indices[len] = index;
            p.len += 1;
            p
        })
    }
}

/// Error while reading the list of field paths.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FieldPathError {
    /// Ran out of bits.
    #[error(transparent)]
    Eof(#[from] BitsEof),
    /// An operation pushed deeper than [`MAX_DEPTH`] or popped above the root: the bit stream is out of sync.
    #[error("field path operation {op} left the valid depth range (stream out of sync?)")]
    BadDepth {
        /// Name of the operation.
        op: &'static str,
    },
}

/// The 40 field path operations in Valve's order, with the weights the Huffman tree is built from.
///
/// The weights are how often each operation occurred in Valve's sample data; a weight of 0 is treated as 1.
pub const OPS: [(&str, u32); 40] = [
    ("PlusOne", 36271),
    ("PlusTwo", 10334),
    ("PlusThree", 1375),
    ("PlusFour", 646),
    ("PlusN", 4128),
    ("PushOneLeftDeltaZeroRightZero", 35),
    ("PushOneLeftDeltaZeroRightNonZero", 3),
    ("PushOneLeftDeltaOneRightZero", 521),
    ("PushOneLeftDeltaOneRightNonZero", 2942),
    ("PushOneLeftDeltaNRightZero", 560),
    ("PushOneLeftDeltaNRightNonZero", 471),
    ("PushOneLeftDeltaNRightNonZeroPack6Bits", 10530),
    ("PushOneLeftDeltaNRightNonZeroPack8Bits", 251),
    ("PushTwoLeftDeltaZero", 0),
    ("PushTwoPack5LeftDeltaZero", 0),
    ("PushThreeLeftDeltaZero", 0),
    ("PushThreePack5LeftDeltaZero", 0),
    ("PushTwoLeftDeltaOne", 0),
    ("PushTwoPack5LeftDeltaOne", 0),
    ("PushThreeLeftDeltaOne", 0),
    ("PushThreePack5LeftDeltaOne", 0),
    ("PushTwoLeftDeltaN", 0),
    ("PushTwoPack5LeftDeltaN", 0),
    ("PushThreeLeftDeltaN", 0),
    ("PushThreePack5LeftDeltaN", 0),
    ("PushN", 0),
    ("PushNAndNonTopological", 310),
    ("PopOnePlusOne", 2),
    ("PopOnePlusN", 0),
    ("PopAllButOnePlusOne", 1837),
    ("PopAllButOnePlusN", 149),
    ("PopAllButOnePlusNPack3Bits", 300),
    ("PopAllButOnePlusNPack6Bits", 634),
    ("PopNPlusOne", 0),
    ("PopNPlusN", 0),
    ("PopNAndNonTopographical", 1),
    ("NonTopoComplex", 76),
    ("NonTopoPenultimatePlusOne", 271),
    ("NonTopoComplexPack4Bits", 99),
    ("FieldPathEncodeFinish", 25474),
];

/// Index of `FieldPathEncodeFinish` in [`OPS`].
const FINISH: u8 = 39;

/// A Huffman tree stored as an array: internal node `n` has children `nodes[n]` = `[left, right]`. A child value
/// below 40 is a leaf (an operation index); 40 and above refers to internal node `value - 40`.
struct HuffmanTree {
    nodes: Vec<[u16; 2]>,
    root: u16,
}

/// Builds the operation Huffman tree exactly like Valve's encoder.
///
/// Classic Huffman: repeatedly take the two lightest trees and join them. Ties need a rule or the codes differ;
/// Valve's rule is "among equal weights, the tree with the **higher value** comes out first", where leaves have value
/// = operation index and joined trees get values 40, 41, 42… in creation order. The first tree taken becomes the
/// left child (bit 0), the second the right child (bit 1).
fn build_tree() -> HuffmanTree {
    // Min-heap on (weight, then highest value first).
    let mut heap: BinaryHeap<Reverse<(u32, Reverse<u16>)>> = OPS
        .iter()
        .enumerate()
        .map(|(i, &(_, w))| Reverse((w.max(1), Reverse(i as u16))))
        .collect();
    let mut nodes = Vec::new();
    while heap.len() > 1 {
        let Reverse((wa, Reverse(a))) = heap.pop().expect("len > 1");
        let Reverse((wb, Reverse(b))) = heap.pop().expect("len > 1");
        let value = 40 + nodes.len() as u16;
        nodes.push([a, b]);
        heap.push(Reverse((wa + wb, Reverse(value))));
    }
    let Reverse((_, Reverse(root))) = heap.pop().expect("one tree left");
    HuffmanTree { nodes, root }
}

fn tree() -> &'static HuffmanTree {
    static TREE: OnceLock<HuffmanTree> = OnceLock::new();
    TREE.get_or_init(build_tree)
}

/// Reads one Huffman-coded operation index (0..40).
fn read_op(r: &mut BitReader<'_>) -> BitsResult<u8> {
    let tree = tree();
    let mut node = tree.root;
    while node >= 40 {
        let bit = r.bits(1, "field path op")? as usize;
        node = tree.nodes[(node - 40) as usize][bit];
    }
    Ok(node as u8)
}

/// The path being edited while reading: Valve's operations work on a fixed array plus the index of the last level.
struct Cursor {
    path: [i32; MAX_DEPTH],
    last: usize,
}

impl Cursor {
    fn push(&mut self, op: &'static str) -> Result<(), FieldPathError> {
        if self.last + 1 >= MAX_DEPTH {
            return Err(FieldPathError::BadDepth { op });
        }
        self.last += 1;
        Ok(())
    }

    /// Goes up `n` levels, zeroing the indices that are left behind (so a later push starts at 0).
    fn pop(&mut self, n: usize, op: &'static str) -> Result<(), FieldPathError> {
        if n > self.last {
            return Err(FieldPathError::BadDepth { op });
        }
        for _ in 0..n {
            self.path[self.last] = 0;
            self.last -= 1;
        }
        Ok(())
    }

    /// Adds `delta` to the index at `level`. Wrapping arithmetic: a corrupt stream must not panic; the nonsense
    /// index it produces is rejected later when the path is resolved against the serializer.
    fn add(&mut self, level: usize, delta: i64) {
        self.path[level] = self.path[level].wrapping_add(delta as i32);
    }

    /// Sets the index at `level` (truncating like [`Cursor::add`]).
    fn set(&mut self, level: usize, value: i64) {
        self.path[level] = value as i32;
    }

    /// For every level from the root to the current one, a flag bit says whether that index changes; if so, `delta`
    /// reads the change. Used by the "non-topological" operations that jump around the tree.
    fn non_topo(
        &mut self,
        r: &mut BitReader<'_>,
        mut delta: impl FnMut(&mut BitReader<'_>) -> BitsResult<i64>,
    ) -> BitsResult<()> {
        for i in 0..=self.last {
            if r.bool("non-topological flag")? {
                let d = delta(r)?;
                self.add(i, d);
            }
        }
        Ok(())
    }

    fn snapshot(&self) -> FieldPath {
        FieldPath {
            indices: self.path,
            len: (self.last + 1) as u8,
        }
    }
}

/// Reads the list of field paths that starts an entity update, up to and including `FieldPathEncodeFinish`.
pub fn read_field_paths(
    r: &mut BitReader<'_>,
    out: &mut Vec<FieldPath>,
) -> Result<(), FieldPathError> {
    out.clear();
    let mut c = Cursor {
        path: [-1, 0, 0, 0, 0, 0, 0],
        last: 0,
    };
    // Shorthands for the value encodings the operations use.
    // (They return i64 so that `value + 5` and friends cannot overflow; see `Cursor::add`.)
    fn fp(r: &mut BitReader<'_>) -> BitsResult<i64> {
        Ok(i64::from(r.ubitvar_field_path("field path index")?))
    }
    fn ubv(r: &mut BitReader<'_>) -> BitsResult<i64> {
        Ok(i64::from(r.ubitvar("field path index")?))
    }
    fn zz(r: &mut BitReader<'_>) -> BitsResult<i64> {
        Ok(i64::from(r.varint_i32("field path delta")?))
    }
    fn bits(r: &mut BitReader<'_>, n: u32) -> BitsResult<i64> {
        Ok(i64::from(r.bits(n, "field path index")?))
    }

    loop {
        let op = read_op(r)?;
        if op == FINISH {
            return Ok(());
        }
        let name = OPS[op as usize].0;
        let l = c.last;
        match op {
            // PlusOne … PlusFour, PlusN: move to a later sibling.
            0..=3 => c.add(l, i64::from(op) + 1),
            4 => c.add(l, fp(r)? + 5),
            // PushOne…: optionally move along, then go one level deeper.
            5 => {
                c.push(name)?;
                c.set(c.last, 0);
            }
            6 => {
                c.push(name)?;
                c.set(c.last, fp(r)?);
            }
            7 => {
                c.add(l, 1);
                c.push(name)?;
                c.set(c.last, 0);
            }
            8 => {
                c.add(l, 1);
                c.push(name)?;
                c.set(c.last, fp(r)?);
            }
            9 => {
                c.add(l, fp(r)?);
                c.push(name)?;
                c.set(c.last, 0);
            }
            10 => {
                c.add(l, fp(r)? + 2);
                c.push(name)?;
                c.set(c.last, fp(r)? + 1);
            }
            11 => {
                c.add(l, bits(r, 3)? + 2);
                c.push(name)?;
                c.set(c.last, bits(r, 3)? + 1);
            }
            12 => {
                c.add(l, bits(r, 4)? + 2);
                c.push(name)?;
                c.set(c.last, bits(r, 4)? + 1);
            }
            // PushTwo / PushThree variants: optional move (none / +1 / +ubitvar+2), then 2 or 3 levels deeper.
            13..=24 => {
                let k = op - 13;
                let levels = if k % 4 < 2 { 2 } else { 3 };
                let pack5 = k % 2 == 1;
                match k / 4 {
                    0 => {}
                    1 => c.add(l, 1),
                    _ => c.add(l, ubv(r)? + 2),
                }
                for _ in 0..levels {
                    c.push(name)?;
                    let v = if pack5 { bits(r, 5)? } else { fp(r)? };
                    c.add(c.last, v);
                }
            }
            25 => {
                // PushN
                let n = ubv(r)?;
                c.add(l, ubv(r)?);
                for _ in 0..n {
                    c.push(name)?;
                    c.add(c.last, fp(r)?);
                }
            }
            26 => {
                // PushNAndNonTopological
                c.non_topo(r, |r| Ok(zz(r)? + 1))?;
                let n = ubv(r)?;
                for _ in 0..n {
                    c.push(name)?;
                    c.set(c.last, fp(r)?);
                }
            }
            27 => {
                c.pop(1, name)?;
                c.add(c.last, 1);
            }
            28 => {
                c.pop(1, name)?;
                c.add(c.last, fp(r)? + 1);
            }
            29 => {
                c.pop(l, name)?;
                c.add(0, 1);
            }
            30 => {
                c.pop(l, name)?;
                c.add(0, fp(r)? + 1);
            }
            31 => {
                c.pop(l, name)?;
                c.add(0, bits(r, 3)? + 1);
            }
            32 => {
                c.pop(l, name)?;
                c.add(0, bits(r, 6)? + 1);
            }
            33 => {
                let n = usize::try_from(fp(r)?).unwrap_or(usize::MAX);
                c.pop(n, name)?;
                c.add(c.last, 1);
            }
            34 => {
                let n = usize::try_from(fp(r)?).unwrap_or(usize::MAX);
                c.pop(n, name)?;
                c.add(c.last, zz(r)?);
            }
            35 => {
                let n = usize::try_from(fp(r)?).unwrap_or(usize::MAX);
                c.pop(n, name)?;
                c.non_topo(r, zz)?;
            }
            36 => c.non_topo(r, zz)?,
            37 => {
                if l == 0 {
                    return Err(FieldPathError::BadDepth { op: name });
                }
                c.add(l - 1, 1);
            }
            38 => c.non_topo(r, |r| Ok(bits(r, 4)? - 7))?,
            _ => unreachable!("read_op returns 0..40 and 39 is handled above"),
        }
        out.push(c.snapshot());
    }
}

/// The Huffman code of every operation, as a string of `0`/`1` in reading order. For tests and documentation.
#[cfg(test)]
pub(crate) fn codes() -> Vec<String> {
    fn walk(tree: &HuffmanTree, node: u16, prefix: String, out: &mut Vec<String>) {
        if node < 40 {
            out[node as usize] = prefix;
            return;
        }
        let [left, right] = tree.nodes[(node - 40) as usize];
        walk(tree, left, format!("{prefix}0"), out);
        walk(tree, right, format!("{prefix}1"), out);
    }
    let mut out = vec![String::new(); 40];
    walk(tree(), tree().root, String::new(), &mut out);
    out
}

/// Writes operations with their Huffman codes, for tests.
#[cfg(test)]
pub(crate) fn write_op(w: &mut super::bits::BitWriter, op: u8) {
    for c in codes()[op as usize].chars() {
        w.bool(c == '1');
    }
}

/// Encodes a list of field paths followed by `FieldPathEncodeFinish`, for building test fixtures.
///
/// Every operation emits exactly one path, so each path is reached in one operation (Valve's encoder picks
/// shorter ones, but any valid choice decodes the same):
/// - same depth or deeper: `PushNAndNonTopological` — per current level a flag + (delta − 1), then the number of
///   levels to push and their indices;
/// - shallower: `PopNAndNonTopographical` — levels to pop, then per remaining level a flag + delta.
#[cfg(test)]
pub(crate) fn write_paths(w: &mut super::bits::BitWriter, paths: &[&[i32]]) {
    const PUSH_N_AND_NON_TOPOLOGICAL: u8 = 26;
    const POP_N_AND_NON_TOPOGRAPHICAL: u8 = 35;
    let mut cur: Vec<i32> = vec![-1];
    for &target in paths {
        if target.len() >= cur.len() {
            write_op(w, PUSH_N_AND_NON_TOPOLOGICAL);
            for (&now, &want) in cur.iter().zip(target) {
                w.bool(now != want);
                if now != want {
                    w.varint_signed(i64::from(want - now - 1));
                }
            }
            w.ubitvar((target.len() - cur.len()) as u32);
            for &index in &target[cur.len()..] {
                w.ubitvar_field_path(index as u32);
            }
        } else {
            write_op(w, POP_N_AND_NON_TOPOGRAPHICAL);
            w.ubitvar_field_path((cur.len() - target.len()) as u32);
            for (&now, &want) in cur.iter().zip(target) {
                w.bool(now != want);
                if now != want {
                    w.varint_signed(i64::from(want - now));
                }
            }
        }
        cur = target.to_vec();
    }
    write_op(w, FINISH);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::bits::BitWriter;

    #[test]
    fn huffman_codes_match_the_reference_table() {
        // The full code table from docs/formats/demo.md, which was cross-checked against demoparser2's precomputed
        // lookup table. Building the tree ourselves must reproduce it exactly.
        let codes = codes();
        let expect = [
            "0",
            "1110",
            "110010",
            "11011111",
            "11010",
            "110110001101",
            "110110001100101",
            "11011010",
            "11000",
            "11011100",
            "11011001",
            "1111",
            "110110110",
            "1101100011001000",
            "11011000110010011",
            "11011000110010010",
            "11011000110011101",
            "11011000110011100",
            "11011000110011111",
            "11011000110011110",
            "11011000110011001",
            "11011000110011000",
            "11011000110011011",
            "11011000110011010",
            "1101100011000101",
            "1101100011000100",
            "110111011",
            "110110001100001",
            "1101100011000111",
            "110011",
            "110110000",
            "110111010",
            "11011110",
            "1101100011000110",
            "1101100011000001",
            "1101100011000000",
            "11011000111",
            "110110111",
            "1101100010",
            "10",
        ];
        for (op, code) in expect.iter().enumerate() {
            assert_eq!(codes[op], *code, "code of {}", OPS[op].0);
        }
        // A valid prefix code: no code is a prefix of another.
        for (i, a) in codes.iter().enumerate() {
            for (j, b) in codes.iter().enumerate() {
                assert!(i == j || !b.starts_with(a.as_str()), "{a} prefixes {b}");
            }
        }
    }

    #[test]
    fn reads_a_sequence_of_operations() {
        let mut w = BitWriter::default();
        write_op(&mut w, 0); // PlusOne → [0]
        write_op(&mut w, 1); // PlusTwo → [2]
        write_op(&mut w, 4); // PlusN with 3 → [10]
        w.ubitvar_field_path(3);
        write_op(&mut w, 8); // PushOneLeftDeltaOneRightNonZero → [11, 6]
        w.ubitvar_field_path(6);
        write_op(&mut w, 0); // PlusOne → [11, 7]
        write_op(&mut w, 11); // Pack6Bits: +(1+2), push (4+1) → [11, 10, 5]
        w.bits(1, 3).bits(4, 3);
        write_op(&mut w, 29); // PopAllButOnePlusOne → [12]
        write_op(&mut w, 36); // NonTopoComplex: index 0 += -2 → [10]
        w.bool(true).varint_signed(-2);
        write_op(&mut w, 39); // finish
        let data = w.finish();

        let mut out = Vec::new();
        read_field_paths(&mut BitReader::new(&data), &mut out).unwrap();
        let got: Vec<Vec<i32>> = out.iter().map(|p| p.indices().to_vec()).collect();
        assert_eq!(
            got,
            vec![
                vec![0],
                vec![2],
                vec![10],
                vec![11, 6],
                vec![11, 7],
                vec![11, 10, 5],
                vec![12],
                vec![10],
            ]
        );
    }

    #[test]
    fn test_encoder_round_trips() {
        let paths: &[&[i32]] = &[
            &[0],
            &[0, 0],
            &[0, 5],
            &[0, 5, 2, 1],
            &[1],
            &[3, 1],
            &[3, 2, 0],
            &[40],
        ];
        let mut w = BitWriter::default();
        write_paths(&mut w, paths);
        let data = w.finish();
        let mut out = Vec::new();
        read_field_paths(&mut BitReader::new(&data), &mut out).unwrap();
        let got: Vec<&[i32]> = out.iter().map(FieldPath::indices).collect();
        assert_eq!(got, paths);
    }

    #[test]
    fn popping_above_the_root_is_an_error() {
        let mut w = BitWriter::default();
        write_op(&mut w, 27); // PopOnePlusOne at depth 1
        let data = w.finish();
        let err = read_field_paths(&mut BitReader::new(&data), &mut Vec::new()).unwrap_err();
        assert!(matches!(err, FieldPathError::BadDepth { .. }));
    }
}
