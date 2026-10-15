//! Coordinate conventions between Source 2, glTF and Unreal Engine — one place, used by every exporter.
//!
//! | Space | Handedness | Axes | Unit |
//! |---|---|---|---|
//! | Source 2 / CS2 | right-handed | X forward, Y left, Z up | inch |
//! | glTF 2.0 | right-handed | Y up, +Z "front" | metre |
//! | Unreal Engine | left-handed | X forward, Y right, Z up | centimetre |
//!
//! **Goal:** a point at Source `(x, y, z)` ends up at Unreal `(x, −y, z) × 2.54`, so maps, models and demo
//! positions all line up in Unreal exactly as in CS2.
//!
//! **Measured, not assumed** (`tools/unreal/probe_axes.py`, UE 5.8.3, 2026-10-15): Unreal's Interchange glTF
//! importer maps glTF `(x, y, z)` metres to Unreal `(x, z, y) × 100`. To reach the goal, OFAST writes glTF as
//! `0.0254 × (x, z, −y)`. That Source → glTF mapping is a pure rotation (determinant +1), so triangle winding and
//! tangent handedness stay valid in the glTF file. See `docs/decisions/003-axes.md`.

/// Inches to metres (Source → glTF).
pub const INCH_TO_METRE: f32 = 0.0254;

/// Inches to centimetres (Source → Unreal).
pub const INCH_TO_CM: f32 = 2.54;

/// Rotates a Source direction (normal, tangent, axis) into glTF axes. No scaling.
pub fn source_dir_to_gltf(v: [f32; 3]) -> [f32; 3] {
    [v[0], v[2], -v[1]]
}

/// Converts a Source position (inches) to a glTF position (metres).
pub fn source_point_to_gltf(p: [f32; 3]) -> [f32; 3] {
    source_dir_to_gltf(p).map(|c| c * INCH_TO_METRE)
}

/// Converts a Source position (inches) straight to an Unreal position (centimetres).
pub fn source_point_to_unreal(p: [f32; 3]) -> [f32; 3] {
    [p[0] * INCH_TO_CM, -p[1] * INCH_TO_CM, p[2] * INCH_TO_CM]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What Unreal's glTF importer does, as measured by `tools/unreal/probe_axes.py`.
    fn unreal_import_of_gltf(g: [f32; 3]) -> [f32; 3] {
        [g[0] * 100.0, g[2] * 100.0, g[1] * 100.0]
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn gltf_path_and_direct_path_agree() {
        for p in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [-1234.5, 678.25, 64.0],
        ] {
            let via_gltf = unreal_import_of_gltf(source_point_to_gltf(p));
            assert!(
                close(via_gltf, source_point_to_unreal(p)),
                "{p:?}: {via_gltf:?}"
            );
        }
    }

    #[test]
    fn source_axes_land_where_expected_in_unreal() {
        // Forward stays forward, left becomes −Y (Unreal's Y points right), up stays up.
        assert!(close(
            source_point_to_unreal([100.0, 0.0, 0.0]),
            [254.0, 0.0, 0.0]
        ));
        assert!(close(
            source_point_to_unreal([0.0, 100.0, 0.0]),
            [0.0, -254.0, 0.0]
        ));
        assert!(close(
            source_point_to_unreal([0.0, 0.0, 100.0]),
            [0.0, 0.0, 254.0]
        ));
    }

    #[test]
    fn source_to_gltf_is_a_rotation_not_a_mirror() {
        // Columns of the mapping matrix: images of the unit axes.
        let [x, y, z] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(source_dir_to_gltf);
        let det = x[0] * (y[1] * z[2] - y[2] * z[1]) - y[0] * (x[1] * z[2] - x[2] * z[1])
            + z[0] * (x[1] * y[2] - x[2] * y[1]);
        assert_eq!(det, 1.0);
    }
}
