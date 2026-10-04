//! Bounded latest-value mailboxes. Preserve dense odometry for capture-time
//! interpolation; coalesce replaceable video/depth/status samples per source.
use anyhow::{Result, ensure};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[derive(Default)]
pub struct FreshIngress {
    registrations: BTreeMap<String, Value>,
    samples: BTreeMap<String, Value>,
    odometry: VecDeque<Value>,
    pose_sources: BTreeSet<String>,
    sensor_poses: VecDeque<Value>,
    pub replaced: u64,
}
impl FreshIngress {
    pub fn push(&mut self, v: Value) -> Result<()> {
        let kind = v["kind"].as_str().unwrap_or("");
        if kind == "register_sensor" && v["spec"]["payload"]["datatype"] == "auki.odometry-pose" {
            self.pose_sources
                .insert(v["spec"]["id"].as_str().unwrap_or("").to_owned());
        }
        if kind == "sensor_sample"
            && self
                .pose_sources
                .contains(v["sensor_id"].as_str().unwrap_or(""))
        {
            if self.sensor_poses.len() == 2048 {
                self.sensor_poses.pop_front();
                self.replaced += 1;
            }
            self.sensor_poses.push_back(v);
            return Ok(());
        }
        if kind == "odometry" {
            if self.odometry.len() == 600 {
                self.odometry.pop_front();
                self.replaced += 1;
            }
            self.odometry.push_back(v);
            return Ok(());
        }
        let (map, key) = if kind == "register_sensor" {
            (
                &mut self.registrations,
                v["spec"]["id"].as_str().unwrap_or("").to_owned(),
            )
        } else {
            (
                &mut self.samples,
                if kind == "sensor_sample" {
                    v["sensor_id"].as_str().unwrap_or("").to_owned()
                } else {
                    kind.to_owned()
                },
            )
        };
        ensure!(!key.is_empty(), "ingress source missing");
        ensure!(
            map.contains_key(&key) || map.len() < 128,
            "too many ingress sources"
        );
        if map.insert(key, v).is_some() {
            self.replaced += 1;
        }
        Ok(())
    }
    pub fn drain(&mut self) -> Vec<Value> {
        let mut values: Vec<_> = std::mem::take(&mut self.registrations)
            .into_values()
            .collect();
        values.extend(self.odometry.drain(..));
        values.extend(self.sensor_poses.drain(..));
        let mut samples: Vec<_> = std::mem::take(&mut self.samples).into_values().collect();
        samples.sort_by_key(|v| {
            v["sample"]["timestamp_ns"]
                .as_u64()
                .or_else(|| v["timestamp_ns"].as_u64())
                .unwrap_or(0)
        });
        values.extend(samples);
        values
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn sensor_poses_survive_image_coalescing() -> Result<()> {
        let mut q = FreshIngress::default();
        q.push(json!({"kind":"register_sensor","spec":{"id":"optical-pose","payload":{"datatype":"auki.odometry-pose"}}}))?;
        for t in 1..3000 {
            q.push(json!({"kind":"sensor_sample","sensor_id":"optical-pose","timestamp_ns":t}))?;
            q.push(json!({"kind":"sensor_sample","sensor_id":"camera","timestamp_ns":t}))?;
        }
        let batch = q.drain();
        assert_eq!(batch.len(), 2050);
        assert_eq!(batch[1]["timestamp_ns"], 952);
        assert_eq!(batch[2048]["timestamp_ns"], 2999);
        assert_eq!(batch[2049]["sensor_id"], "camera");
        Ok(())
    }
    #[test]
    fn overload_keeps_newest_per_sensor_but_preserves_pose_brackets() -> Result<()> {
        let mut q = FreshIngress::default();
        q.push(json!({"kind":"register_sensor","spec":{"id":"camera"}}))?;
        for t in 1..1000 {
            q.push(json!({"kind":"sensor_sample","sensor_id":"camera","timestamp_ns":t}))?;
            q.push(json!({"kind":"odometry","timestamp_ns":t}))?;
        }
        let batch = q.drain();
        assert_eq!(batch.len(), 602);
        assert_eq!(batch[0]["kind"], "register_sensor");
        assert_eq!(batch[1]["timestamp_ns"], 400);
        assert_eq!(batch[600]["timestamp_ns"], 999);
        assert_eq!(batch[601]["timestamp_ns"], 999);
        assert!(q.drain().is_empty());
        Ok(())
    }
}
