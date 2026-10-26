//! A map's radar overview: where its radar image sits in the world, for drawing players on it (the Studio's 2D
//! replay).
//!
//! CS2 keeps it in `resource/overviews/<map>.txt` (KeyValues 1, [`crate::kv1`]), the file HLTV used:
//!
//! ```text
//! "de_nuke"
//! {
//!     "material"  "overviews/de_nuke"   // texture file
//!     "pos_x"     "-3453"               // upper left world coordinate
//!     "pos_y"     "2887"
//!     "scale"     "7"
//!     "verticalsections"
//!     {
//!         "default" { "AltitudeMax" "10000" "AltitudeMin" "-495" }
//!         "lower"   { "AltitudeMax" "-495"  "AltitudeMin" "-10000" }
//!     }
//! }
//! ```
//!
//! The radar image is `panorama/images/overheadmaps/<map>_radar_psd.vtex_c` (1024 × 1024): world units per pixel =
//! `scale`, its top-left corner at (`pos_x`, `pos_y`), x to the right and y down the image (world y up the image).
//! Maps on two levels (Nuke, Train, Vertigo) list vertical sections: each has its own image,
//! `<map>_<section>_radar_psd` (`default` is the plain one), for the heights between its altitudes.

use serde::Serialize;

use crate::kv1;

/// Where a map's radar image sits in the world.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Overview {
    /// The world x of the image's left edge.
    pub pos_x: f32,
    /// The world y of the image's top edge.
    pub pos_y: f32,
    /// World units per image pixel.
    pub scale: f32,
    /// The levels of a map on two floors, each with its own image; empty for a map on one level.
    pub sections: Vec<Section>,
}

/// One level of a map on two floors.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Section {
    /// Its name in the file: `default` (the main image) or another (`lower`).
    pub name: String,
    /// The heights it covers, world units: from `altitude_min` up to `altitude_max`.
    pub altitude_min: f32,
    /// See [`altitude_min`](Self::altitude_min).
    pub altitude_max: f32,
}

/// The size of a radar image, pixels per side.
pub const RADAR_SIZE: f32 = 1024.0;

impl Overview {
    /// Reads `resource/overviews/<map>.txt`'s text; `None` without the position and scale.
    pub fn parse(text: &str) -> Option<Self> {
        let root = kv1::parse(text);
        // The file's one top-level block, named after the map.
        let (_, map) = root.pairs().first()?;
        let number = |key: &str| map.text(key)?.trim().parse::<f32>().ok();
        let sections = map
            .get("verticalsections")
            .map(|sections| {
                sections
                    .pairs()
                    .iter()
                    .filter_map(|(name, section)| {
                        let altitude = |key: &str| section.text(key)?.trim().parse::<f32>().ok();
                        Some(Section {
                            name: name.clone(),
                            altitude_min: altitude("AltitudeMin")?,
                            altitude_max: altitude("AltitudeMax")?,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            pos_x: number("pos_x")?,
            pos_y: number("pos_y")?,
            scale: number("scale").filter(|s| *s > 0.0)?,
            sections,
        })
    }

    /// Where a world position (x, y) falls on the radar image, in pixels from its top-left corner.
    pub fn to_image(&self, [x, y]: [f32; 2]) -> [f32; 2] {
        [(x - self.pos_x) / self.scale, (self.pos_y - y) / self.scale]
    }

    /// The section a height falls in (`None` for a map on one level, or a height outside every section).
    pub fn section_at(&self, z: f32) -> Option<&Section> {
        self.sections
            .iter()
            .find(|s| z >= s.altitude_min && z < s.altitude_max)
    }
}

/// The overview file of a map.
pub fn overview_path(map: &str) -> String {
    format!("resource/overviews/{map}.txt")
}

/// The radar image of a map's section (`None` or `default`: the main image).
pub fn radar_image_path(map: &str, section: Option<&str>) -> String {
    match section {
        None | Some("default") => format!("panorama/images/overheadmaps/{map}_radar_psd.vtex_c"),
        Some(section) => format!("panorama/images/overheadmaps/{map}_{section}_radar_psd.vtex_c"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUKE: &str = r#"
// HLTV overview description file for de_nuke.bsp
"de_nuke"
{
	"material"	"overviews/de_nuke"	// texture file
	"pos_x"		"-3453"	// upper left world coordinate
	"pos_y"		"2887"
	"scale"		"7"
	"verticalsections"
	{
		"default" // i.e. de_nuke_radar.dds
		{
			"AltitudeMax" "10000"
			"AltitudeMin" "-495"
		}
		"lower" // i.e. de_nuke_lower_radar.dds
		{
			"AltitudeMax" "-495"
			"AltitudeMin" "-10000"
		}
	}
	"CTSpawn_x"	"0.82"
}
"#;

    #[test]
    fn reads_position_scale_and_sections() {
        let overview = Overview::parse(NUKE).expect("an overview");
        assert_eq!(
            (overview.pos_x, overview.pos_y, overview.scale),
            (-3453.0, 2887.0, 7.0)
        );
        let names: Vec<&str> = overview.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["default", "lower"]);
        assert_eq!(
            overview.section_at(-600.0).map(|s| s.name.as_str()),
            Some("lower")
        );
        assert_eq!(
            overview.section_at(0.0).map(|s| s.name.as_str()),
            Some("default")
        );
    }

    #[test]
    fn world_positions_land_on_the_image() {
        let overview = Overview::parse(NUKE).expect("an overview");
        assert_eq!(overview.to_image([-3453.0, 2887.0]), [0.0, 0.0]);
        // 7 units per pixel: the image's far corner is 7168 units right and down.
        assert_eq!(
            overview.to_image([-3453.0 + 7168.0, 2887.0 - 7168.0]),
            [1024.0, 1024.0]
        );
    }

    #[test]
    fn radar_images_by_section() {
        assert_eq!(
            radar_image_path("de_nuke", Some("lower")),
            "panorama/images/overheadmaps/de_nuke_lower_radar_psd.vtex_c"
        );
        assert_eq!(
            radar_image_path("de_dust2", None),
            "panorama/images/overheadmaps/de_dust2_radar_psd.vtex_c"
        );
    }

    #[test]
    fn a_file_without_a_scale_is_no_overview() {
        assert_eq!(
            Overview::parse("\"de_x\" { \"pos_x\" \"1\" \"pos_y\" \"2\" }"),
            None
        );
    }
}
