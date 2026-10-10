//! bo2mp: Black Ops II's ragdoll definitions (`ragdoll.cfg`), read as the
//! game reads them. A definition is a set of bodies, each a capsule from
//! one joint of the skeleton to the next (`j_shoulder_le` to `j_elbow_le`)
//! with its radius, mass and friction and the body it hangs from; the pairs
//! of bodies that may not pass through each other; and the joint between
//! each body and its parent: a swivel (neck, shoulders, hips) or a hinge
//! (elbows, knees), with its angle limits per axis.
//!
//! Every machine reads the file from the zones it loaded; a dead player's
//! body goes limp by definition 0 once the server says it does (see
//! `render_anim::occupancy::ragdoll`).

use std::sync::Arc;

/// One body: a capsule between two joints.
#[derive(Clone, Debug, Default)]
pub struct RagdollBody {
    /// The joint it starts at (`p0`) and the one it reaches (`p1`).
    pub joint: String,
    pub child: String,
    pub radius: f32,
    pub cog_lerp: f32,
    pub mass: f32,
    pub friction: f32,
    /// The body it hangs from (earlier in the list), none for the root.
    pub parent: Option<usize>,
    /// The right side's mirror of a left body.
    pub mirror: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RagdollJointKind {
    Swivel,
    Hinge,
}

/// The joint between a body and its parent.
#[derive(Clone, Debug)]
pub struct RagdollJoint {
    pub body: usize,
    pub kind: RagdollJointKind,
    /// Per axis (x twist, y, z): the (least, most) angle in degrees, none
    /// where the file sets no limit.
    pub limits: [Option<(f32, f32)>; 3],
}

#[derive(Clone, Debug, Default)]
pub struct RagdollDef {
    pub bodies: Vec<RagdollBody>,
    /// Bodies that collide with each other.
    pub self_pairs: Vec<(usize, usize)>,
    pub joints: Vec<RagdollJoint>,
}

/// The players' ragdoll (definition 0) as a resource; none when the zones
/// hold no `ragdoll.cfg`.
#[derive(bevy_ecs::prelude::Resource, Clone, Default)]
pub struct T6RagdollRes(pub Option<Arc<RagdollDef>>);

impl T6RagdollRes {
    pub fn from_text(text: Option<&str>) -> Self {
        let Some(text) = text else {
            return Self(None);
        };
        let def = parse(text).into_iter().next().filter(|d| !d.bodies.is_empty());
        match def {
            Some(def) => Self(Some(Arc::new(def))),
            None => {
                diag::warn!(Sim, "bo2mp: ragdoll.cfg has no definition 0");
                Self(None)
            }
        }
    }
}

/// Every definition in the file, by number (`ragdoll_clear N` starts one
/// again; `ragdoll_bone`, `ragdoll_selfpair`, `ragdoll_joint` and
/// `ragdoll_limit` add to one). Lines it does not know are skipped.
pub fn parse(text: &str) -> Vec<RagdollDef> {
    let mut defs: Vec<RagdollDef> = Vec::new();
    fn def(defs: &mut Vec<RagdollDef>, n: usize) -> &mut RagdollDef {
        if defs.len() <= n {
            defs.resize_with(n + 1, RagdollDef::default);
        }
        &mut defs[n]
    }
    for raw in text.lines() {
        let line = raw.split("//").next().unwrap_or("");
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some((&cmd, args)) = t.split_first() else { continue };
        let Some(n) = args.first().and_then(|a| a.parse::<usize>().ok()).filter(|n| *n < 16) else {
            continue;
        };
        let f = |i: usize| args.get(i).and_then(|a| a.parse::<f32>().ok());
        let u = |i: usize| args.get(i).and_then(|a| a.parse::<i32>().ok());
        match cmd.to_ascii_lowercase().as_str() {
            "ragdoll_clear" => *def(&mut defs, n) = RagdollDef::default(),
            "ragdoll_bone" => {
                let (Some(joint), Some(child)) = (args.get(1), args.get(2)) else { continue };
                let parent = u(7).and_then(|p| usize::try_from(p).ok());
                let d = def(&mut defs, n);
                d.bodies.push(RagdollBody {
                    joint: joint.to_ascii_lowercase(),
                    child: child.to_ascii_lowercase(),
                    radius: f(3).unwrap_or(2.0),
                    cog_lerp: f(4).unwrap_or(0.5),
                    mass: f(5).unwrap_or(1.0),
                    friction: f(6).unwrap_or(0.3),
                    parent: parent.filter(|p| *p < d.bodies.len()),
                    mirror: u(8).is_some_and(|m| m != 0),
                });
            }
            "ragdoll_selfpair" => {
                if let (Some(a), Some(b)) = (u(1), u(2))
                    && let (Ok(a), Ok(b)) = (usize::try_from(a), usize::try_from(b))
                {
                    def(&mut defs, n).self_pairs.push((a, b));
                }
            }
            "ragdoll_joint" => {
                let (Some(body), Some(kind)) = (u(1).and_then(|b| usize::try_from(b).ok()), args.get(2)) else {
                    continue;
                };
                let kind = if kind.eq_ignore_ascii_case("hinge") {
                    RagdollJointKind::Hinge
                } else {
                    RagdollJointKind::Swivel
                };
                def(&mut defs, n).joints.push(RagdollJoint { body, kind, limits: [None; 3] });
            }
            "ragdoll_limit" => {
                let (Some(joint), Some(axis), Some(lo), Some(hi)) =
                    (u(1).and_then(|j| usize::try_from(j).ok()), args.get(2), f(4), f(5))
                else {
                    continue;
                };
                let axis = match axis.trim_start_matches('-').to_ascii_lowercase().as_str() {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    _ => continue,
                };
                if let Some(j) = def(&mut defs, n).joints.get_mut(joint) {
                    j.limits[axis] = Some((lo.min(hi), lo.max(hi)));
                }
            }
            _ => {}
        }
    }
    // Pairs and joints that name bodies the definition does not have.
    for d in &mut defs {
        let count = d.bodies.len();
        d.self_pairs.retain(|(a, b)| *a < count && *b < count && a != b);
        d.joints.retain(|j| j.body < count && d.bodies[j.body].parent.is_some());
    }
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "ragdoll_clear 0\n\
        ragdoll_bone 0 j_mainroot j_neck 5.0 0.5 3.0 0.3 -1 0 capsule 1.0\n\
        ragdoll_bone 0 j_neck j_head 3.5 0.5 0.3 0.3 0 0 capsule 1.0\n\
        ragdoll_bone 0 j_shoulder_ri j_elbow_ri 2.5 0.5 0.6 0.3 0 1 capsule 1.0 // right\n\
        ragdoll_selfpair 0 1 2\n\
        ragdoll_joint 0 1 swivel // neck\n\
        ragdoll_joint 0 2 hinge\n\
        ragdoll_limit 0 0 z 60.0 30.0 -20.0\n\
        ragdoll_limit 0 1 z 60.0 -120.0 -5.0\n\
        ragdoll_clear 1\n\
        ragdoll_bone 1 j_mainroot j_spinelower 4.5 0.5 20.0 0.7 -1 0 capsule 1.0\n";

    #[test]
    fn reads_bodies_joints_and_limits() {
        let defs = parse(SAMPLE);
        assert_eq!(defs.len(), 2);
        let d = &defs[0];
        assert_eq!(d.bodies.len(), 3);
        assert_eq!(d.bodies[0].parent, None);
        assert_eq!(d.bodies[2].parent, Some(0));
        assert!(d.bodies[2].mirror);
        assert_eq!(d.bodies[1].child, "j_head");
        assert_eq!(d.self_pairs, vec![(1, 2)]);
        assert_eq!(d.joints.len(), 2);
        assert_eq!(d.joints[1].kind, RagdollJointKind::Hinge);
        assert_eq!(d.joints[0].limits[2], Some((-20.0, 30.0)));
        assert_eq!(d.joints[1].limits[2], Some((-120.0, -5.0)));
        assert_eq!(defs[1].bodies.len(), 1);
    }
}
