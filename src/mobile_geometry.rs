//! Combine Home's stable calibration reference with its independent live geometry.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
pub fn render(scene: &crate::network::CanonicalScene) -> Result<Value> {
    ensure!(
        scene.frame.starts_with("home-live:")
            && scene.registries["local_mapping"]["origin"]["kind"] == "canonical_portals"
            && scene.registries["local_mapping"]["origin"]["portal"] == "UCOF1J40N7O",
        "Live geometry is not aligned to Home's canonical origin"
    );
    let mut view = crate::scene_mirror::render_scene(scene)?;
    // render_scene converts Z-up to OpenGL; Home's declared canonical-portals
    // origin establishes the shared physical origin, not merely equal axes.
    view["frame"] = json!(crate::model::FRAME);
    if let Some(robot) = view["local_mapping"]
        .get_mut("robot_pose")
        .filter(|v| !v.is_null())
    {
        robot["frame"] = json!(crate::model::FRAME);
    }
    if let Some(runners) = view["local_mapping"]["runners"].as_object_mut() {
        for runner in runners.values_mut() {
            if let Some(robot) = runner.get_mut("robot_pose").filter(|v| !v.is_null()) {
                robot["frame"] = json!(crate::model::FRAME);
            }
        }
    }
    Ok(view)
}
pub fn merge(reference: &Value, live: Option<&Value>) -> Value {
    let mut result = reference.clone();
    if let Some(live) = live {
        for key in ["voxels", "voxel_size", "objects", "local_mapping"] {
            result["render"][key] = live[key].clone();
        }
        if let (Some(notes), Some(target)) = (
            live["notes"].as_array(),
            result["render"]["notes"].as_array_mut(),
        ) {
            // Authored notes come from the canonical scene, so a stale live map
            // cannot resurrect a deleted note. Keep live robot labels as well.
            target.extend(
                notes
                    .iter()
                    .filter(|n| !n["anchor_robot"].is_null())
                    .cloned(),
            );
        }
    }
    result
}
