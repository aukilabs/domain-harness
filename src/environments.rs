//! Environment names and input-file ownership, not a runtime calibration switch.
//! Each configured origin seeds an independent saved SDK scenegraph.
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub status: String,
    pub portal_poses: Option<String>,
    pub portal_sizes: String,
    pub origin: Option<Origin>,
}
#[derive(Deserialize)]
pub struct Origin {
    pub portal_code: String,
    /// Supplied Portal pose and the map's explicit coordinate reference.
    pub translation_m: [f64; 3],
    pub rotation_wxyz: [f64; 4],
    pub frame_description: String,
}
#[derive(Deserialize)]
pub struct Catalog {
    pub schema: u32,
    pub active_reference: String,
    pub preferred_scene: Option<String>,
    pub environments: Vec<Environment>,
}
pub static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../config/environments.json")).expect("Environment catalog")
});
pub fn reference_name() -> &'static str {
    // Naming a pending environment must not relabel Xmart's hard-wired
    // calibration inputs as if they belonged to that environment.
    assert_eq!(
        CATALOG.active_reference, "xmart",
        "Environment switching requires its own reference-scene integration"
    );
    &CATALOG
        .environments
        .iter()
        .find(|e| e.id == CATALOG.active_reference)
        .expect("Active reference environment")
        .name
}

#[cfg(test)]
mod tests {
    #[test]
    fn environments_have_separate_portal_inputs_and_no_guessed_nacs_geometry() {
        let c = &super::CATALOG;
        assert_eq!(c.schema, 1);
        assert_eq!(super::reference_name(), "Xmart");
        let old = c.environments.iter().find(|e| e.id == "xmart").unwrap();
        let new = c.environments.iter().find(|e| e.id == "nacs").unwrap();
        assert_eq!(new.name, "NACS");
        assert_eq!(new.status, "portal_reference");
        assert!(new.portal_poses.is_none());
        assert_ne!(old.portal_sizes, new.portal_sizes);
        let sizes = crate::portal_sizes::PortalSizes::load(&new.portal_sizes).unwrap();
        let origin = new.origin.as_ref().unwrap();
        assert_eq!(origin.portal_code, "RG3E3LUITXC");
        assert_eq!(origin.translation_m, [0.; 3]);
        assert_eq!(sizes.portals.len(), 1);
        assert_eq!(sizes.portals[0].code, origin.portal_code);
        assert_eq!(sizes.portals[0].side_length_m, 0.15);
        assert_eq!(
            sizes.encoded_size_m(&origin.portal_code).unwrap(),
            Some(0.15)
        );
        assert_eq!(
            sizes.measurement_basis,
            crate::portal_sizes::MeasurementBasis::EncodedSquare
        );
        assert_eq!(
            crate::domain_portals::load()
                .unwrap()
                .scenegraph
                .anchors
                .len(),
            13
        );
    }
}
