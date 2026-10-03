//! Manually supplied dimensions only. Codes are kept exactly as supplied; this
//! registry does not invent UUIDs, placements, or contact portal infrastructure.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementBasis {
    Unconfirmed,
    EncodedSquare,
    FullPrintedMarker,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalSize {
    pub code: String,
    pub side_length_m: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortalSizes {
    pub schema: u32,
    pub measurement_basis: MeasurementBasis,
    pub portals: Vec<PortalSize>,
}

impl PortalSizes {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let result: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.schema == 1, "unsupported portal-size schema");
        let mut codes = BTreeSet::new();
        for p in &self.portals {
            ensure!(
                p.code.len() == 11
                    && p.code
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                "invalid portal code"
            );
            ensure!(codes.insert(&p.code), "duplicate portal code: {}", p.code);
            ensure!(
                p.side_length_m.is_finite() && p.side_length_m > 0.,
                "portal size must be finite and positive"
            );
        }
        Ok(())
    }

    /// Safe injection boundary for the SDK's encoded-square size field.
    /// Unknown codes have no guessed size and never cause a backend lookup.
    pub fn encoded_size_m(&self, code: &str) -> Result<Option<f64>> {
        self.validate()?;
        let value = self.portals.iter().find(|p| p.code == code);
        if value.is_none() {
            return Ok(None);
        }
        ensure!(
            self.measurement_basis == MeasurementBasis::EncodedSquare,
            "confirm encoded-square dimensions before SDK injection"
        );
        Ok(value.map(|p| p.side_length_m))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manual_sizes_require_confirmed_basis_and_never_guess_unknowns() {
        let mut sizes: PortalSizes =
            serde_json::from_str(include_str!("../config/portals.json")).unwrap();
        sizes.validate().unwrap();
        assert_eq!(sizes.portals.len(), 13);
        assert_eq!(
            sizes
                .portals
                .iter()
                .filter(|p| p.side_length_m == 0.05)
                .count(),
            1
        );
        assert_eq!(
            sizes
                .portals
                .iter()
                .filter(|p| p.side_length_m == 0.10)
                .count(),
            4
        );
        assert_eq!(
            sizes
                .portals
                .iter()
                .filter(|p| p.side_length_m == 0.15)
                .count(),
            8
        );
        sizes.measurement_basis = MeasurementBasis::Unconfirmed;
        assert!(sizes.encoded_size_m("2ZWHV5L6I04").is_err());
        assert_eq!(sizes.encoded_size_m("UNKNOWN").unwrap(), None);
        sizes.measurement_basis = MeasurementBasis::EncodedSquare;
        assert_eq!(sizes.encoded_size_m("2ZWHV5L6I04").unwrap(), Some(0.05));
        sizes.portals[0].side_length_m = -1.;
        assert!(sizes.validate().is_err());
    }
}
