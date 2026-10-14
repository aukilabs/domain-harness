//! Shared declared point-cloud wire contract for every producer.
use crate::compact_pose::FrameRef;
use anyhow::{Result, ensure};
use auki_components::*;
use serde::{Deserialize, Serialize};

pub const POINTS_SCHEMA: &str = "spatial-harness.depth-points/v1";
pub const POINTS_METADATA: &str = "spatial-harness.depth-points-contract/v1";
pub const MAX_POINTS: usize = 2048;
pub const PACKED_POINTS_SCHEMA: &str = "spatial-harness.depth-points/v2";
pub const PACKED_POINTS_ENCODING: &str = "spatial-harness.depth-points-packed/v2";
pub const PACKED_HEADER_BYTES: usize = 16;
pub const MAX_PACKED_CONTRACT_BYTES: usize = 4096;
pub const MAX_PACKED_BYTES: usize = PACKED_HEADER_BYTES + MAX_PACKED_CONTRACT_BYTES + MAX_POINTS * 15;

/// Which frame the points are in. A reader places them by the transforms out of that frame.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointsContract {
    pub frame: FrameRef,
    pub clock: ClockReference,
    pub session_id: String,
}
impl PointsContract {
    pub fn output_id(&self) -> String {
        format!("points-{}", manifest_hash(self))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Points {
    /// Metres in the camera frame the contract names.
    pub points: Vec<[f64; 3]>,
    /// sRGB per point, when the camera gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<[u8; 3]>>,
}
impl ContractType for Points {
    const DATATYPE: &'static str = POINTS_SCHEMA;
}
impl Points {
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.points.is_empty() && self.points.len() <= MAX_POINTS, "Depth capture needs 1–{MAX_POINTS} points");
        ensure!(self.points.iter().flatten().all(|v| v.is_finite() && v.abs() <= 100.), "Invalid depth point");
        ensure!(self.colors.as_ref().is_none_or(|c| c.len() == self.points.len()), "Depth color/point count differs");
        Ok(())
    }
}

/// An immutable, validated binary depth capture. Capture time and source sequence
/// remain in the SDK Observation envelope; this packet only describes measured
/// coordinates and their exact frame/clock/session contract.
///
/// Deliberately not serde-serializable: export and consume this type with the
/// SDK binary observation codec, never a JSON byte array or base64 payload.
#[derive(Clone, Debug)]
pub struct PackedPoints {
    contract: PointsContract,
    bytes: Vec<u8>,
    points_offset: usize,
    count: usize,
    colors: bool,
}

impl ContractType for PackedPoints {
    const DATATYPE: &'static str = PACKED_POINTS_SCHEMA;
}

impl PackedPoints {
    pub fn from_points(contract: PointsContract, points: &Points) -> Result<Self> {
        points.validate()?;
        validate_packed_contract(&contract)?;
        let descriptor = serde_json::to_vec(&contract)?;
        ensure!(descriptor.len() <= MAX_PACKED_CONTRACT_BYTES, "Depth contract is too large");
        let colors = points.colors.is_some();
        let stride = if colors { 15 } else { 12 };
        let points_offset = PACKED_HEADER_BYTES + descriptor.len();
        let mut bytes = Vec::with_capacity(points_offset + points.points.len() * stride);
        bytes.extend_from_slice(b"APC2");
        bytes.extend_from_slice(&[u8::from(colors), 0, 0, 0]);
        bytes.extend_from_slice(&(points.points.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(descriptor.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&descriptor);
        for (i, point) in points.points.iter().enumerate() {
            for coordinate in point {
                bytes.extend_from_slice(&(*coordinate as f32).to_le_bytes());
            }
            if let Some(colors) = &points.colors {
                bytes.extend_from_slice(&colors[i]);
            }
        }
        Ok(Self { contract, bytes, points_offset, count: points.points.len(), colors })
    }

    /// Bounds and validates before copying network bytes or allocating points.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!((PACKED_HEADER_BYTES..=MAX_PACKED_BYTES).contains(&bytes.len()), "Invalid packed depth size");
        ensure!(&bytes[..4] == b"APC2", "Unknown packed depth version");
        ensure!(bytes[4] <= 1 && bytes[5..8] == [0, 0, 0], "Unknown packed depth flags");
        let count = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let descriptor_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        ensure!((1..=MAX_POINTS).contains(&count), "Invalid packed depth point count");
        ensure!((1..=MAX_PACKED_CONTRACT_BYTES).contains(&descriptor_len), "Invalid packed depth contract size");
        let colors = bytes[4] == 1;
        let stride = if colors { 15 } else { 12 };
        let points_offset = PACKED_HEADER_BYTES + descriptor_len;
        ensure!(bytes.len() == points_offset + count * stride, "Packed depth length/count mismatch");
        let contract: PointsContract = serde_json::from_slice(&bytes[PACKED_HEADER_BYTES..points_offset])?;
        validate_packed_contract(&contract)?;
        for point in bytes[points_offset..].chunks_exact(stride) {
            for axis in 0..3 {
                let coordinate = f32::from_le_bytes(point[axis * 4..axis * 4 + 4].try_into().unwrap());
                ensure!(coordinate.is_finite() && coordinate.abs() <= 100., "Invalid packed depth point");
            }
        }
        Ok(Self { contract, bytes: bytes.to_vec(), points_offset, count, colors })
    }

    pub fn contract(&self) -> &PointsContract { &self.contract }
    pub fn as_bytes(&self) -> &[u8] { &self.bytes }
    pub fn point_count(&self) -> usize { self.count }
    pub fn has_colors(&self) -> bool { self.colors }

    pub fn validate(&self) -> Result<()> {
        Self::from_bytes(&self.bytes).map(|_| ())
    }

    pub fn into_points(self) -> Points {
        let stride = if self.colors { 15 } else { 12 };
        let mut points = Vec::with_capacity(self.count);
        let mut colors = self.colors.then(|| Vec::with_capacity(self.count));
        for point in self.bytes[self.points_offset..].chunks_exact(stride) {
            points.push(std::array::from_fn(|axis| {
                f32::from_le_bytes(point[axis * 4..axis * 4 + 4].try_into().unwrap()) as f64
            }));
            if let Some(colors) = &mut colors {
                colors.push([point[12], point[13], point[14]]);
            }
        }
        Points { points, colors }
    }

    pub fn encoder() -> std::result::Result<auki_component_protocol::ObservationEncoder<Self>, auki_component_protocol::ComponentProtocolError> {
        auki_component_protocol::ObservationEncoder::new(PACKED_POINTS_ENCODING, |points: &Self| Ok(points.as_bytes().to_vec()))
    }

    pub fn decoder() -> std::result::Result<auki_component_protocol::ObservationDecoder<Self>, auki_component_protocol::ComponentProtocolError> {
        auki_component_protocol::ObservationDecoder::new(PACKED_POINTS_ENCODING, |bytes| Self::from_bytes(bytes).map_err(|e| e.to_string()))
    }
}

fn validate_packed_contract(contract: &PointsContract) -> Result<()> {
    ensure!(!contract.session_id.trim().is_empty() && contract.session_id.len() <= 256, "Invalid depth session");
    for value in [&contract.frame.peer_id, &contract.frame.id, &contract.frame.hash,
        &contract.clock.peer_id, &contract.clock.id, &contract.clock.hash] {
        ensure!(!value.trim().is_empty(), "Missing depth registry reference");
    }
    // Registry identity, frame definition and Output binding are checked by the
    // consumer against the authenticated producer, not inferred by this codec.
    Ok(())
}
