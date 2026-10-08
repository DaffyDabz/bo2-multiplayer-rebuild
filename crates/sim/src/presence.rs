use crate::bullet_collision::{EntityCollisionCapabilities, LinkedBrushCollisionBrush};
use crate::gentity::{ScriptMoverGentity, apos_from_entity_state, pos_from_entity_state};
use crate::identities::ScriptModelId;
use entity_iw4::{CG_SCRIPT_MOVER_NODRAW, evaluate_trajectory};
use std::collections::BTreeMap;

/// bo2zm M3: brush movers the prediction world has no row for (Black Ops
/// II's doors and debris piles: its level starts after prediction arms)
/// get one from the snapshot, the brush model the mover names (`index`) at
/// its pose, as IW4's client clips against a snapshot's brush entities;
/// such a row goes when its mover does. `follow_movers` keeps its pose and
/// solidity after.
pub(crate) fn adopt_brush_movers(
    capabilities: &mut Vec<EntityCollisionCapabilities>,
    movers: &[ScriptMoverGentity],
    time_ms: i32,
) {
    let brushes: BTreeMap<ScriptModelId, &ScriptMoverGentity> = movers
        .iter()
        .filter(|mover| {
            mover.state.solid == entity_iw4::SCRIPT_MOVER_BMODEL_SOLID && mover.state.index >= 0
        })
        .map(|mover| (mover.id, mover))
        .collect();
    let before = capabilities.len();
    capabilities.retain(|row| {
        !row.from_snapshot
            || row
                .owner
                .script_model()
                .is_some_and(|id| brushes.contains_key(&id))
    });
    let removed = before - capabilities.len();
    let mut added = 0;
    for (id, mover) in brushes {
        let owner = crate::AuthorityModelOwner::ScriptModel(id);
        if capabilities
            .binary_search_by_key(&owner, |row| row.owner)
            .is_ok()
        {
            continue;
        }
        let origin = evaluate_trajectory(&pos_from_entity_state(&mover.state), time_ms);
        let angles = evaluate_trajectory(&apos_from_entity_state(&mover.state), time_ms);
        let mut row = EntityCollisionCapabilities::current_tick(
            owner,
            None,
            vec![LinkedBrushCollisionBrush {
                cmodel_handle: mover.state.index as u32,
                origin,
                angles,
            }],
        );
        row.from_snapshot = true;
        row.hidden = mover.state.e_flags & CG_SCRIPT_MOVER_NODRAW != 0;
        row.solid = !mover.nonsolid;
        row.followed_pose = Some((origin, angles));
        capabilities.push(row);
        added += 1;
    }
    if added > 0 {
        capabilities.sort_by_key(|row| row.owner);
    }
    static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if (added > 0 || removed > 0)
        && *LOG.get_or_init(|| std::env::var_os("IW4L_PRED_LOG").is_some())
    {
        diag::info!(
            Sim,
            "prediction brush rows: {added} added, {removed} gone, {} in all",
            capabilities.iter().filter(|row| row.from_snapshot).count()
        );
    }
}

pub(crate) fn follow_movers(
    capabilities: &mut [EntityCollisionCapabilities],
    movers: &[ScriptMoverGentity],
    time_ms: i32,
) {
    let by_id: BTreeMap<ScriptModelId, &ScriptMoverGentity> =
        movers.iter().map(|mover| (mover.id, mover)).collect();
    for row in capabilities {
        let Some(mover) = row.owner.script_model().and_then(|id| by_id.get(&id)) else {
            continue;
        };
        row.hidden = mover.state.e_flags & CG_SCRIPT_MOVER_NODRAW != 0;
        row.solid = !mover.nonsolid;
        let pose = (
            evaluate_trajectory(&pos_from_entity_state(&mover.state), time_ms),
            evaluate_trajectory(&apos_from_entity_state(&mover.state), time_ms),
        );
        match row.followed_pose {
            None => row.followed_pose = Some(pose),
            Some(followed) if followed == pose => {}
            Some(_) => {
                row.followed_pose = Some(pose);
                if let Some(dobj) = row.dobj.as_mut() {
                    dobj.set_world_pose(pose.0, pose.1);
                }
                for brush in &mut row.linked_brushes {
                    brush.origin = pose.0;
                    brush.angles = pose.1;
                }
            }
        }
    }
}
