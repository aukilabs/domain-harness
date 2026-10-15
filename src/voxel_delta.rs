//! A Home scene's occupied voxels for phones: a keyframe now and then,
//! otherwise only the cells that changed. Binary on the wire, never JSON.
//!
//! Cells are grid indices in the scene's frame: a cell's centre is
//! `(index + 0.5) * voxel_size` metres on each axis. Colours are sRGB bytes.
use anyhow::{Result, bail, ensure};
use auki_components::ContractType;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

pub const SCHEMA: &str = "spatial-harness.voxel-delta/v1";
pub const ENCODING: &str = "spatial-harness.voxel-delta-packed/v1";
/// Metadata on the Product: where a new reader should start.
pub const METADATA: &str = "spatial-harness.voxel-delta-keyframe/v1";
/// Side of a chunk in cells. A phone redraws whole chunks.
pub const CHUNK: i32 = 16;
/// A keyframe after this many deltas, so a late reader never waits long.
pub const KEYFRAME_EVERY: u64 = 30;
const MAX_CELLS: usize = 2_000_000;

pub fn product_id(scene: &str) -> String {
    format!("phone-voxels:{scene}")
}

pub type Index = [i32; 3];

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    /// Changes when Home's map restarts; a reader then waits for a keyframe.
    pub epoch: u64,
    pub sequence: u64,
    pub keyframe: bool,
    pub frame: String,
    pub voxel_size: f32,
    /// Cells now occupied, with their colour. A keyframe lists all of them.
    pub set: Vec<(Index, [u8; 3])>,
    pub removed: Vec<Index>,
}
impl ContractType for Update {
    const DATATYPE: &'static str = SCHEMA;
}
// Serde only so the SDK's retained buffer can size and hold it; the wire is `pack`.
impl Serialize for Update {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&pack(self).map_err(serde::ser::Error::custom)?)
    }
}
impl<'de> Deserialize<'de> for Update {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let bytes = <Vec<u8>>::deserialize(d)?;
        unpack(&bytes).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header { epoch: u64, sequence: u64, keyframe: bool, frame: String, voxel_size: f32, set: u32, removed: u32 }

/// "VXD1", a u32-length JSON header, then each set cell as 3 × i32 + 3 colour
/// bytes, then each removed cell as 3 × i32. Little endian.
pub fn pack(u: &Update) -> Result<Vec<u8>> {
    ensure!(u.set.len() + u.removed.len() <= MAX_CELLS, "Too many voxel changes");
    ensure!(u.voxel_size.is_finite() && u.voxel_size > 0., "Invalid voxel size");
    let header = serde_json::to_vec(&Header { epoch: u.epoch, sequence: u.sequence, keyframe: u.keyframe,
        frame: u.frame.clone(), voxel_size: u.voxel_size, set: u.set.len() as u32, removed: u.removed.len() as u32 })?;
    let mut bytes = Vec::with_capacity(8 + header.len() + u.set.len() * 15 + u.removed.len() * 12);
    bytes.extend_from_slice(b"VXD1");
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    for (index, color) in &u.set {
        for v in index { bytes.extend_from_slice(&v.to_le_bytes()); }
        bytes.extend_from_slice(color);
    }
    for index in &u.removed {
        for v in index { bytes.extend_from_slice(&v.to_le_bytes()); }
    }
    Ok(bytes)
}
pub fn unpack(bytes: &[u8]) -> Result<Update> {
    ensure!(bytes.len() >= 8 && &bytes[..4] == b"VXD1", "Invalid voxel update packet");
    let n = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    ensure!(n <= 4096 && bytes.len() >= 8 + n, "Invalid voxel update header");
    let h: Header = serde_json::from_slice(&bytes[8..8 + n])?;
    let (set, removed) = (h.set as usize, h.removed as usize);
    ensure!(set + removed <= MAX_CELLS && bytes.len() == 8 + n + set * 15 + removed * 12, "Invalid voxel update length");
    ensure!(h.voxel_size.is_finite() && h.voxel_size > 0., "Invalid voxel size");
    let body = &bytes[8 + n..];
    let int = |at: usize| i32::from_le_bytes(body[at..at + 4].try_into().unwrap());
    Ok(Update {
        epoch: h.epoch, sequence: h.sequence, keyframe: h.keyframe, frame: h.frame, voxel_size: h.voxel_size,
        set: (0..set).map(|i| { let at = i * 15; ([int(at), int(at + 4), int(at + 8)], [body[at + 12], body[at + 13], body[at + 14]]) }).collect(),
        removed: (0..removed).map(|i| { let at = set * 15 + i * 12; [int(at), int(at + 4), int(at + 8)] }).collect(),
    })
}
pub fn encoder() -> auki_component_protocol::ObservationEncoder<Update> {
    auki_component_protocol::ObservationEncoder::new(ENCODING, |u| pack(u).map_err(|e| e.to_string()))
        .expect("static voxel delta codec")
}
pub fn decoder() -> Result<auki_component_protocol::ObservationDecoder<Update>> {
    Ok(auki_component_protocol::ObservationDecoder::new(ENCODING, |b| unpack(b).map_err(|e| e.to_string()))?)
}

pub fn index(center: [f64; 3], voxel_size: f64) -> Index {
    center.map(|v| (v / voxel_size).floor() as i32)
}
pub fn center(index: Index, voxel_size: f32) -> [f32; 3] {
    index.map(|v| (v as f32 + 0.5) * voxel_size)
}
pub fn chunk(index: Index) -> Index {
    index.map(|v| v.div_euclid(CHUNK))
}
fn to_srgb(linear: f64) -> u8 {
    let v = linear.clamp(0., 1.);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1. / 2.4) - 0.055 };
    (s * 255.).round() as u8
}
pub fn to_linear(srgb: u8) -> f32 {
    let s = srgb as f32 / 255.;
    if s <= 0.040_45 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}
/// Default colour for a cell with none, as the phone drew it before.
pub const UNCOLORED: [u8; 3] = [79, 194, 237];

/// Home's side: the occupied cells it last sent, and the next update.
pub struct Tracker {
    epoch: u64,
    sequence: u64,
    since_keyframe: u64,
    frame: String,
    voxel_size: f32,
    cells: HashMap<Index, [u8; 3]>,
}
impl Default for Tracker {
    fn default() -> Self {
        Self { epoch: rand::random::<u64>() >> 11, sequence: 0, since_keyframe: 0, frame: String::new(), voxel_size: 0., cells: HashMap::new() }
    }
}
impl Tracker {
    /// The map as Home renders it now (centres in `frame`, linear RGB).
    /// None when nothing changed and no keyframe is due.
    pub fn update(&mut self, frame: &str, voxel_size: f64, voxels: &[([f64; 3], Option<[f64; 3]>)]) -> Option<Update> {
        let cells: HashMap<Index, [u8; 3]> = voxels.iter()
            .map(|(c, rgb)| (index(*c, voxel_size), rgb.map_or(UNCOLORED, |rgb| rgb.map(to_srgb))))
            .collect();
        let size = voxel_size as f32;
        let keyframe = self.sequence == 0 || self.since_keyframe + 1 >= KEYFRAME_EVERY || frame != self.frame || size != self.voxel_size;
        let (set, removed): (Vec<_>, Vec<_>) = if keyframe {
            (cells.iter().map(|(i, c)| (*i, *c)).collect(), Vec::new())
        } else {
            (cells.iter().filter(|(i, c)| self.cells.get(*i) != Some(*c)).map(|(i, c)| (*i, *c)).collect(),
             self.cells.keys().filter(|i| !cells.contains_key(*i)).copied().collect())
        };
        if !keyframe && set.is_empty() && removed.is_empty() {
            return None;
        }
        self.sequence += 1;
        self.since_keyframe = if keyframe { 0 } else { self.since_keyframe + 1 };
        self.frame = frame.into();
        self.voxel_size = size;
        self.cells = cells;
        Some(Update { epoch: self.epoch, sequence: self.sequence, keyframe, frame: frame.into(), voxel_size: size, set, removed })
    }
}

/// A phone's side: the cells it holds and the chunks to redraw.
#[derive(Default)]
pub struct Mirror {
    pub epoch: Option<u64>,
    pub sequence: u64,
    pub frame: String,
    pub voxel_size: f32,
    pub cells: HashMap<Index, [u8; 3]>,
    /// Chunks changed since the app last took them.
    pub dirty: BTreeSet<Index>,
    /// True when the app must drop every chunk it drew (new keyframe).
    pub reset: bool,
}
impl Mirror {
    /// Apply an update in order. A delta that does not follow on is refused:
    /// the reader waits for the next keyframe rather than draw a wrong map.
    pub fn apply(&mut self, u: &Update) -> Result<()> {
        if u.keyframe {
            self.dirty.extend(self.cells.keys().map(|i| chunk(*i)));
            self.cells = u.set.iter().copied().collect();
            self.dirty.extend(self.cells.keys().map(|i| chunk(*i)));
            self.reset = self.reset || self.frame != u.frame || self.voxel_size != u.voxel_size || self.epoch != Some(u.epoch);
        } else {
            if self.epoch != Some(u.epoch) || u.sequence != self.sequence + 1 || u.frame != self.frame || u.voxel_size != self.voxel_size {
                bail!("Voxel update {} does not follow {}; waiting for a keyframe", u.sequence, self.sequence);
            }
            for (i, c) in &u.set { self.cells.insert(*i, *c); self.dirty.insert(chunk(*i)); }
            for i in &u.removed {
                if self.cells.remove(i).is_some() { self.dirty.insert(chunk(*i)); }
            }
            // A face on a chunk's edge depends on the neighbour across it.
            let edges: Vec<Index> = u.set.iter().map(|(i, _)| *i).chain(u.removed.iter().copied())
                .flat_map(|i| [[1, 0, 0], [-1, 0, 0], [0, 1, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]]
                    .map(|d| chunk([i[0] + d[0], i[1] + d[1], i[2] + d[2]]))).collect();
            self.dirty.extend(edges);
        }
        self.epoch = Some(u.epoch);
        self.sequence = u.sequence;
        self.frame = u.frame.clone();
        self.voxel_size = u.voxel_size;
        Ok(())
    }
    /// Take the changed chunks, each with its cells, for the app to redraw.
    pub fn take(&mut self) -> Option<serde_json::Value> {
        if self.dirty.is_empty() && !self.reset {
            return None;
        }
        let dirty = std::mem::take(&mut self.dirty);
        let mut by_chunk: HashMap<Index, Vec<i32>> = dirty.iter().map(|c| (*c, Vec::new())).collect();
        for (i, c) in &self.cells {
            if let Some(cells) = by_chunk.get_mut(&chunk(*i)) {
                cells.extend([i[0], i[1], i[2], c[0] as i32, c[1] as i32, c[2] as i32]);
            }
        }
        let reset = std::mem::take(&mut self.reset);
        Some(serde_json::json!({
            "frame": self.frame, "voxel_size": self.voxel_size, "reset": reset, "chunk": CHUNK,
            "chunks": by_chunk.into_iter().map(|(key, cells)| serde_json::json!({"key": key, "cells": cells})).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(x: f64) -> ([f64; 3], Option<[f64; 3]>) { ([x, 0.05, 0.05], Some([1., 0., 0.])) }

    #[test]
    fn deltas_carry_only_changes_and_round_trip_in_binary() {
        let mut home = Tracker::default();
        let mut phone = Mirror::default();
        let first = home.update("f", 0.1, &[at(0.05), at(0.15)]).unwrap();
        assert!(first.keyframe && first.set.len() == 2);
        let second = home.update("f", 0.1, &[at(0.05), at(0.25)]).unwrap();
        assert!(!second.keyframe);
        assert_eq!(second.set, vec![([2, 0, 0], [255, 0, 0])]);
        assert_eq!(second.removed, vec![[1, 0, 0]]);
        assert!(home.update("f", 0.1, &[at(0.05), at(0.25)]).is_none(), "nothing changed, nothing sent");
        for u in [first, second] {
            let wire = unpack(&pack(&u).unwrap()).unwrap();
            assert_eq!(wire, u);
            phone.apply(&wire).unwrap();
        }
        assert_eq!(phone.cells.len(), 2);
        assert!(phone.cells.contains_key(&[2, 0, 0]) && !phone.cells.contains_key(&[1, 0, 0]));
        let taken = phone.take().unwrap();
        assert!(taken["chunks"].as_array().unwrap().iter().any(|c| c["key"] == serde_json::json!([0, 0, 0])));
        assert!(phone.take().is_none());
    }

    #[test]
    fn a_reader_refuses_a_gap_until_the_next_keyframe() {
        let mut home = Tracker::default();
        let mut phone = Mirror::default();
        let first = home.update("f", 0.1, &[at(0.05)]).unwrap();
        let _lost = home.update("f", 0.1, &[at(0.05), at(0.15)]).unwrap();
        let third = home.update("f", 0.1, &[at(0.15)]).unwrap();
        phone.apply(&first).unwrap();
        assert!(phone.apply(&third).is_err());
        // Keep the map changing until Home's next keyframe comes round.
        let mut keyframe = None;
        for step in 0..2 * KEYFRAME_EVERY {
            let voxels = if step % 2 == 0 { vec![at(0.15), at(0.35)] } else { vec![at(0.15)] };
            if let Some(u) = home.update("f", 0.1, &voxels).filter(|u| u.keyframe) { keyframe = Some(u); break; }
        }
        let keyframe = keyframe.expect("a keyframe within two cycles");
        phone.apply(&keyframe).unwrap();
        assert_eq!(phone.cells.len(), keyframe.set.len(), "the keyframe replaces everything the reader held");
    }

    #[test]
    fn a_change_on_a_chunk_edge_redraws_the_neighbour() {
        let mut phone = Mirror::default();
        phone.apply(&Update { epoch: 1, sequence: 1, keyframe: true, frame: "f".into(), voxel_size: 0.1, set: vec![], removed: vec![] }).unwrap();
        phone.take();
        phone.apply(&Update { epoch: 1, sequence: 2, keyframe: false, frame: "f".into(), voxel_size: 0.1,
            set: vec![([CHUNK - 1, 0, 0], [1, 2, 3])], removed: vec![] }).unwrap();
        assert!(phone.dirty.contains(&[0, 0, 0]) && phone.dirty.contains(&[1, 0, 0]));
    }

    #[test]
    fn grid_and_colours_round_trip() {
        assert_eq!(index([-3.75, -0.35, 0.05], 0.1), [-38, -4, 0]);
        let c = center([-38, -4, 0], 0.1);
        assert!((c[0] + 3.75).abs() < 1e-5 && (c[1] + 0.35).abs() < 1e-5);
        for v in [0u8, 1, 60, 128, 255] { assert_eq!(to_srgb(to_linear(v) as f64), v); }
    }
}
