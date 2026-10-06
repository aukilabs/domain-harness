//! Locally managed, printable fiducials. The measured size is always the
//! outer black marker square; the white quiet zone is deliberately excluded.
use anyhow::{Context, Result, ensure};
use calib_targets_aruco::{Dictionary, builtins::builtin_dictionary};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FiducialFamily {
    #[serde(rename = "aruco_4x4_50")]
    Aruco4x4_50,
    #[serde(rename = "apriltag_36h11")]
    Apriltag36h11,
}

impl FiducialFamily {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Aruco4x4_50 => "aruco_4x4_50",
            Self::Apriltag36h11 => "apriltag_36h11",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Aruco4x4_50 => "ArUco 4x4 (50)",
            Self::Apriltag36h11 => "AprilTag 36h11",
        }
    }

    pub fn dictionary(self) -> Dictionary {
        builtin_dictionary(match self {
            Self::Aruco4x4_50 => "DICT_4X4_50",
            Self::Apriltag36h11 => "DICT_APRILTAG_36h11",
        })
        .expect("bundled fiducial dictionary")
    }

    pub fn parse_slug(value: &str) -> Option<Self> {
        [Self::Aruco4x4_50, Self::Apriltag36h11]
            .into_iter()
            .find(|family| family.slug() == value)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fiducial {
    pub family: FiducialFamily,
    pub id: u32,
    pub side_length_m: f64,
    pub created_unix_ms: u64,
}

impl Fiducial {
    pub fn key(&self) -> String {
        format!("{}:{}", self.family.slug(), self.id)
    }

    pub fn download_url(&self) -> String {
        format!(
            "/api/fiducials/{}/{}/marker.svg",
            self.family.slug(),
            self.id
        )
    }

    pub fn print_url(&self) -> String {
        format!("/api/fiducials/{}/{}/print", self.family.slug(), self.id)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.side_length_m.is_finite() && (0.01..=2.0).contains(&self.side_length_m),
            "marker side length must be between 10 mm and 2000 mm"
        );
        ensure!(
            (self.id as usize) < self.family.dictionary().codes().len(),
            "marker ID is outside this family"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FiducialDocument {
    schema: u32,
    measurement_basis: String,
    fiducials: Vec<Fiducial>,
}

impl Default for FiducialDocument {
    fn default() -> Self {
        Self {
            schema: 1,
            measurement_basis: "outer_black_square".into(),
            fiducials: Vec::new(),
        }
    }
}

impl FiducialDocument {
    fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported fiducial registry schema");
        ensure!(
            self.measurement_basis == "outer_black_square",
            "unsupported fiducial measurement basis"
        );
        let mut keys = std::collections::BTreeSet::new();
        for marker in &self.fiducials {
            marker.validate()?;
            ensure!(keys.insert(marker.key()), "duplicate fiducial");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateFiducial {
    pub family: FiducialFamily,
    pub id: Option<u32>,
    pub side_length_m: f64,
}

pub struct FiducialStore {
    path: PathBuf,
    document: FiducialDocument,
}

impl FiducialStore {
    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let document = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("invalid fiducial registry")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(error) => return Err(error.into()),
        };
        let result = Self { path, document };
        result.document.validate()?;
        Ok(result)
    }

    pub fn list(&self) -> &[Fiducial] {
        &self.document.fiducials
    }

    pub fn create(&mut self, request: CreateFiducial) -> Result<Fiducial> {
        let dictionary = request.family.dictionary();
        let id = match request.id {
            Some(id) => id,
            None => (0..dictionary.codes().len() as u32)
                .find(|id| {
                    !self
                        .document
                        .fiducials
                        .iter()
                        .any(|marker| marker.family == request.family && marker.id == *id)
                })
                .context("this marker family has no unused IDs")?,
        };
        ensure!(
            !self
                .document
                .fiducials
                .iter()
                .any(|marker| marker.family == request.family && marker.id == id),
            "that marker ID is already registered"
        );
        let marker = Fiducial {
            family: request.family,
            id,
            side_length_m: request.side_length_m,
            created_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_millis() as u64,
        };
        marker.validate()?;
        self.document.fiducials.push(marker.clone());
        self.document
            .fiducials
            .sort_by_key(|marker| (marker.family.slug(), marker.id));
        if let Err(error) = self.persist() {
            self.document
                .fiducials
                .retain(|candidate| candidate.key() != marker.key());
            return Err(error);
        }
        Ok(marker)
    }

    pub fn find(&self, family: FiducialFamily, id: u32) -> Option<&Fiducial> {
        self.document
            .fiducials
            .iter()
            .find(|marker| marker.family == family && marker.id == id)
    }

    pub fn resize(
        &mut self,
        family: FiducialFamily,
        id: u32,
        side_length_m: f64,
    ) -> Result<Fiducial> {
        let index = self
            .document
            .fiducials
            .iter()
            .position(|marker| marker.family == family && marker.id == id)
            .context("fiducial is not registered")?;
        let previous = self.document.fiducials[index].clone();
        let updated = Fiducial {
            side_length_m,
            ..previous.clone()
        };
        updated.validate()?;
        self.document.fiducials[index] = updated.clone();
        if let Err(error) = self.persist() {
            self.document.fiducials[index] = previous;
            return Err(error);
        }
        Ok(updated)
    }

    fn persist(&self) -> Result<()> {
        if let Some(parent) = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self
            .path
            .with_extension(format!("fiducials.{}.tmp", uuid::Uuid::new_v4().simple()));
        std::fs::write(&temporary, serde_json::to_vec_pretty(&self.document)?)?;
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }
}

pub fn marker_view(marker: &Fiducial) -> serde_json::Value {
    serde_json::json!({
        "family": marker.family,
        "family_label": marker.family.label(),
        "id": marker.id,
        "key": marker.key(),
        "side_length_m": marker.side_length_m,
        "side_length_mm": marker.side_length_m * 1000.,
        "measurement_basis": "outer_black_square",
        "created_unix_ms": marker.created_unix_ms,
        "download_url": marker.download_url(),
        "print_url": marker.print_url(),
    })
}

pub fn catalog_view(store: &FiducialStore) -> serde_json::Value {
    let families = [FiducialFamily::Aruco4x4_50, FiducialFamily::Apriltag36h11].map(|family| {
        serde_json::json!({
            "id": family,
            "label": family.label(),
            "capacity": family.dictionary().codes().len(),
        })
    });
    serde_json::json!({
        "schema": 1,
        "measurement_basis": "outer_black_square",
        "families": families,
        "fiducials": store.list().iter().map(marker_view).collect::<Vec<_>>(),
    })
}

pub fn registry_view(store: &FiducialStore) -> serde_json::Value {
    serde_json::json!({
        "schema": "spatial.fiducials/v1",
        "measurement_basis": "outer_black_square",
        "items": store.list(),
    })
}

/// An SVG whose outer black square has the declared physical side length. A
/// one-cell white quiet zone surrounds it and is additional to that dimension.
pub fn printable_svg(marker: &Fiducial) -> Result<String> {
    marker.validate()?;
    let dictionary = marker.family.dictionary();
    let inner = dictionary.marker_size();
    let marker_cells = inner + 2;
    let page_cells = marker_cells + 2;
    let side_mm = marker.side_length_m * 1000.;
    let page_mm = side_mm * page_cells as f64 / marker_cells as f64;
    let code = dictionary.codes()[marker.id as usize];
    let mut cells = String::new();
    for y in 0..marker_cells {
        for x in 0..marker_cells {
            let border = x == 0 || y == 0 || x + 1 == marker_cells || y + 1 == marker_cells;
            let data = !border && code & (1_u64 << ((y - 1) * inner + (x - 1))) != 0;
            if border || data {
                cells.push_str(&format!(
                    r#"<rect x="{}" y="{}" width="1" height="1"/>"#,
                    x + 1,
                    y + 1
                ));
            }
        }
    }
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{page_mm:.4}mm" height="{page_mm:.4}mm" viewBox="0 0 {page_cells} {page_cells}" shape-rendering="crispEdges"><title>{} ID {}</title><metadata>family={}; id={}; outer_black_square_mm={side_mm:.4}</metadata><rect width="{page_cells}" height="{page_cells}" fill="white"/><g fill="black">{cells}</g></svg>"#,
        marker.family.label(),
        marker.id,
        marker.family.slug(),
        marker.id,
    ))
}

pub fn registry_path(journal: &Path) -> PathBuf {
    let stem = journal
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("home");
    journal.with_file_name(format!("{stem}.fiducials.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_size_correction_survives_reload_and_failed_writes_roll_back() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("markers.json");
        let mut store = FiducialStore::load(&path)?;
        let family = FiducialFamily::Aruco4x4_50;
        let original = store.create(CreateFiducial {
            family,
            id: Some(4),
            side_length_m: 0.15,
        })?;
        let resized = store.resize(family, 4, 0.148)?;
        assert_eq!(resized.created_unix_ms, original.created_unix_ms);
        assert_eq!(
            FiducialStore::load(&path)?
                .find(family, 4)
                .unwrap()
                .side_length_m,
            0.148
        );
        for invalid in [0.0, f64::NAN, f64::INFINITY, 2.1] {
            assert!(store.resize(family, 4, invalid).is_err());
        }
        assert!(store.resize(family, 5, 0.1).is_err());
        assert_eq!(store.find(family, 4).unwrap().side_length_m, 0.148);
        store.path = directory.path().join("directory-instead-of-file");
        std::fs::create_dir(&store.path)?;
        assert!(store.resize(family, 4, 0.2).is_err());
        assert_eq!(store.find(family, 4).unwrap().side_length_m, 0.148);
        Ok(())
    }

    #[test]
    fn creates_persists_and_reloads_standard_markers() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("markers.json");
        let mut store = FiducialStore::load(&path)?;
        let marker = store.create(CreateFiducial {
            family: FiducialFamily::Aruco4x4_50,
            id: Some(7),
            side_length_m: 0.12,
        })?;
        assert_eq!(marker.key(), "aruco_4x4_50:7");
        assert!(
            store
                .create(CreateFiducial {
                    family: FiducialFamily::Aruco4x4_50,
                    id: Some(7),
                    side_length_m: 0.1,
                })
                .is_err()
        );
        let reloaded = FiducialStore::load(path)?;
        assert_eq!(reloaded.list().len(), 1);
        assert_eq!(reloaded.list()[0].side_length_m, 0.12);
        Ok(())
    }

    #[test]
    fn svg_uses_standard_bits_and_keeps_quiet_zone_outside_declared_size() -> Result<()> {
        let marker = Fiducial {
            family: FiducialFamily::Apriltag36h11,
            id: 0,
            side_length_m: 0.16,
            created_unix_ms: 0,
        };
        let svg = printable_svg(&marker)?;
        // AprilTag 36h11 is 6 data cells + a one-cell black border. One more
        // white cell on each side makes a 200 mm page around a 160 mm marker.
        assert!(svg.contains("width=\"200.0000mm\""));
        assert!(svg.contains("viewBox=\"0 0 10 10\""));
        assert!(svg.contains("outer_black_square_mm=160.0000"));
        assert!(svg.contains("<rect x=\"1\" y=\"1\""));
        Ok(())
    }

    #[test]
    fn automatic_ids_are_family_local_and_sizes_are_bounded() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let mut store = FiducialStore::load(directory.path().join("markers.json"))?;
        for family in [FiducialFamily::Aruco4x4_50, FiducialFamily::Apriltag36h11] {
            let marker = store.create(CreateFiducial {
                family,
                id: None,
                side_length_m: 0.1,
            })?;
            assert_eq!(marker.id, 0);
        }
        assert!(
            store
                .create(CreateFiducial {
                    family: FiducialFamily::Aruco4x4_50,
                    id: None,
                    side_length_m: 0.001,
                })
                .is_err()
        );
        Ok(())
    }
}
