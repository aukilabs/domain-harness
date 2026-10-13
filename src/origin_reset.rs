//! Restart-bound origin reset. Never relabel a live Output's physical frame.
//! A durable journal makes interrupted multi-file rebinding resumable. Startup
//! must finish it before mounting Products or accepting requests.
use anyhow::{Result, ensure};
use auki_scenegraph::MapSnapshot;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    path: String,
    before: Value,
    after: Value,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    scene: String,
    old_frame: String,
    new_frame: String,
    changes: Option<Vec<Change>>,
}
fn read(path: &Path) -> Result<Value> {
    ensure!(
        std::fs::metadata(path)?.len() <= 16 * 1024 * 1024,
        "Reset input exceeds bounds"
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}
fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().unwrap();
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(temp, path)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn stage(directory: &Path, expected: &MapSnapshot) -> Result<Value> {
    expected.validate()?;
    let scene = &expected.scenegraph.map.map_id;
    let old = &expected.scenegraph.map.frame.id;
    ensure!(
        crate::local_scenes::frame_scene_id(old) == Some(scene.as_str()),
        "Invalid scene identity"
    );
    let path = directory
        .join(".origin-resets")
        .join(format!("{scene}.pending"));
    let pending = if path.exists() {
        let pending: Pending = serde_json::from_value(read(&path)?)?;
        ensure!(
            pending.old_frame == *old && pending.scene == *scene,
            "Different reset already pending"
        );
        pending
    } else {
        let fresh = crate::scene_origin::reset(expected)?;
        let pending = Pending {
            scene: scene.clone(),
            old_frame: old.clone(),
            new_frame: fresh.scenegraph.map.frame.id,
            changes: None,
        };
        write(&path, &pending)?;
        pending
    };
    Ok(
        json!({"status":"pending_restart","restart_required":true,"old_frame":old,"new_frame":pending.new_frame,
        "voxels_cleared":false,"notes_preserved":true,"volumes_preserved":true,"paths_preserved":true,
        "summary":"Origin reset queued. Restart Home to clear portals/voxels and activate a new frame. Authored coordinates are retained numerically, not physically realigned."}),
    )
}

fn targets(id: &str) -> Vec<String> {
    vec![
        format!("mobile-notes/{id}.json"),
        format!("semantic-volumes/{id}.json"),
        format!("paths/{id}.json"),
        format!("{id}.json"),
    ]
}
/// Called only during LocalScenes startup, before readers/writers are mounted.
pub fn apply_pending(directory: &Path) -> Result<()> {
    let queue = directory.join(".origin-resets");
    if !queue.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&queue)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("pending") {
            continue;
        }
        let mut p: Pending = serde_json::from_value(read(&path)?)?;
        ensure!(
            crate::local_scenes::frame_scene_id(&p.old_frame) == Some(p.scene.as_str())
                && crate::local_scenes::frame_scene_id(&p.new_frame) == Some(p.scene.as_str())
                && p.new_frame != p.old_frame
                && p.new_frame.contains(":epoch-"),
            "Invalid origin reset journal"
        );
        let allowed = targets(&p.scene);
        if p.changes.is_none() {
            let mut changes = Vec::new();
            for relative in &allowed {
                let file = directory.join(relative);
                if !file.exists() {
                    continue;
                }
                let before = read(&file)?;
                let after = if relative.starts_with("mobile-notes/") {
                    let mut notes: crate::notes::Notes = serde_json::from_value(before.clone())?;
                    notes.validate(&p.old_frame)?;
                    notes.frame = p.new_frame.clone();
                    for note in &mut notes.items {
                        if let Some(surface) = &mut note.anchor_surface {
                            if surface.frame == p.old_frame {
                                surface.frame = p.new_frame.clone();
                            }
                        }
                    }
                    notes.validate(&p.new_frame)?;
                    serde_json::to_value(notes)?
                } else if relative.starts_with("semantic-volumes/") {
                    let mut volumes: crate::semantic_volumes::Volumes =
                        serde_json::from_value(before.clone())?;
                    volumes.validate(&p.old_frame)?;
                    volumes.frame = p.new_frame.clone();
                    volumes.validate(&p.new_frame)?;
                    serde_json::to_value(volumes)?
                } else if relative.starts_with("paths/") {
                    let mut paths: crate::scene_paths::Paths =
                        serde_json::from_value(before.clone())?;
                    paths.validate(&p.old_frame)?;
                    paths.frame = p.new_frame.clone();
                    paths.validate(&p.new_frame)?;
                    serde_json::to_value(paths)?
                } else {
                    let snapshot: MapSnapshot = serde_json::from_value(before.clone())?;
                    ensure!(
                        snapshot.scenegraph.map.frame.id == p.old_frame,
                        "Saved scene changed before reset"
                    );
                    let mut fresh = crate::scene_origin::reset(&snapshot)?.scenegraph;
                    fresh.map.frame.id = p.new_frame.clone();
                    serde_json::to_value(MapSnapshot::new(fresh)?)?
                };
                changes.push(Change {
                    path: relative.clone(),
                    before,
                    after,
                });
            }
            ensure!(
                changes
                    .iter()
                    .any(|c| c.path == format!("{}.json", p.scene)),
                "Reset scene missing"
            );
            p.changes = Some(changes);
            write(&path, &p)?;
        }
        let changes = p.changes.as_ref().unwrap();
        // Validate the entire remaining write set before changing any file.
        let mut seen = std::collections::BTreeSet::new();
        for change in changes {
            ensure!(
                allowed.contains(&change.path) && seen.insert(&change.path),
                "Invalid reset file target"
            );
            let current = read(&directory.join(&change.path))?;
            ensure!(
                current == change.before || current == change.after,
                "Reset file was edited during recovery; originals retained in journal"
            );
        }
        for change in changes {
            write(&directory.join(&change.path), &change.after)?;
        }
        // Preserve original authored data in the completed journal for recovery.
        std::fs::rename(
            &path,
            queue.join(format!("{}-{}.complete", p.scene, uuid::Uuid::new_v4())),
        )?;
        std::fs::File::open(&queue)?.sync_all()?;
    }
    Ok(())
}
