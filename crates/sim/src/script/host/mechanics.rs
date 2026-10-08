use super::entities::Link;
use crate::script::runtime::raise;
use crate::script::{BTreeMap, Resource, Runtime, Value};
use bevy_ecs::prelude::World;

/// Script-driven entity mechanics: timed moves, launched physics bodies and
/// entity/player links. They run before the VM each tick and settle native
/// collision, so a thread woken by movedone or physics_finished already
/// queries the final pose.
#[derive(Resource, Clone, Default)]
pub(crate) struct Mechanics {
    motions: BTreeMap<u64, Vec<Motion>>,
    bodies: BTreeMap<u64, Body>,
    finished: Vec<(u64, &'static str)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Body {
    velocity: [f32; 3],
    ticks: u32,
}

const TICK_S: f32 = crate::MATCH_TICK_MS as f32 / 1000.0;
const SETTLE_TICKS: u32 = 200;
const BODY_MINS: [f32; 3] = [-12.0, -12.0, 0.0];
const BODY_MAXS: [f32; 3] = [12.0, 12.0, 24.0];

impl Mechanics {
    pub(crate) fn start(&mut self, object: u64, motion: Motion) {
        let motions = self.motions.entry(object).or_default();
        motions.retain(|m| m.field != motion.field);
        motions.push(motion);
    }

    pub(crate) fn stop(&mut self, object: u64, field: &str) {
        if let Some(motions) = self.motions.get_mut(&object) {
            motions.retain(|m| m.field != field);
        }
    }

    pub(crate) fn launch(&mut self, object: u64, velocity: [f32; 3]) {
        self.motions.remove(&object);
        self.bodies.insert(object, Body { velocity, ticks: 0 });
    }

    pub(crate) fn velocity(&self, object: u64, now: i64) -> [f32; 3] {
        self.motions
            .get(&object)
            .and_then(|motions| motions.iter().find(|m| m.field == "origin"))
            .map_or([0.0; 3], |m| m.sample(now).1)
    }
}

pub(crate) fn advance_mechanics(world: &mut World) {
    let request = world.resource::<crate::step::StepRequest>();
    if !request.reason.advances_authority_world() {
        return;
    }
    let now = i64::from(request.tick.0) * i64::from(crate::MATCH_TICK_MS);
    let runtime = world.resource::<Runtime>();
    if runtime.fault.is_some() || runtime.program.is_none() {
        return;
    }
    advance_motions(world, now);
    advance_bodies(world);
    apply_entity_links(world);
    super::players::apply_player_links(world);
    super::presence::settle_collision(world);
}

pub(crate) fn deliver_finished(world: &mut World) {
    let finished = std::mem::take(&mut world.resource_mut::<Mechanics>().finished);
    for (object, name) in finished {
        raise(world, Value::Object(object), name, Vec::new());
    }
}

fn advance_motions(world: &mut World, now: i64) {
    world.resource_scope::<Mechanics, _>(|world, mut mechanics| {
        let mut runtime = world.resource_mut::<Runtime>();
        let Mechanics {
            motions, finished, ..
        } = &mut *mechanics;
        motions.retain(|object, _| runtime.entities.contains_key(object));
        for (object, list) in motions.iter_mut() {
            list.retain(|motion| {
                let (value, _) = motion.sample(now);
                runtime.set_object_field(*object, motion.field, Value::Vector(value));
                let done = now - motion.start_ms >= motion.duration_ms;
                if done {
                    finished.push((*object, motion.done));
                }
                !done
            });
        }
        motions.retain(|_, list| !list.is_empty());
    });
}

fn advance_bodies(world: &mut World) {
    world.resource_scope::<Mechanics, _>(|world, mut mechanics| {
        let Mechanics {
            bodies, finished, ..
        } = &mut *mechanics;
        bodies.retain(|object, body| {
            let (origin, angles) = {
                let mut runtime = world.resource_mut::<Runtime>();
                if !runtime.entities.contains_key(object) {
                    return false;
                }
                match (
                    runtime.object_field(*object, "origin"),
                    runtime.object_field(*object, "angles"),
                ) {
                    (Value::Vector(o), Value::Vector(a)) => (o, a),
                    (Value::Vector(o), _) => (o, [0.0; 3]),
                    _ => return true,
                }
            };
            body.velocity[2] -= GRAVITY * TICK_S;
            let end: [f32; 3] = std::array::from_fn(|i| origin[i] + body.velocity[i] * TICK_S);
            let trace = crate::frame::FrameWorld::from_world(world).trace_static_world(
                origin,
                end,
                BODY_MINS,
                BODY_MAXS,
                crate::bullet_collision::MASK_PLAYER_SOLID,
            );
            let fraction = if trace.startsolid != 0 {
                0.0
            } else {
                trace.fraction
            };
            let at: [f32; 3] = std::array::from_fn(|i| origin[i] + (end[i] - origin[i]) * fraction);
            body.ticks += 1;
            let rested = fraction < 1.0 || body.ticks >= SETTLE_TICKS;
            let mut runtime = world.resource_mut::<Runtime>();
            runtime.set_object_field(*object, "origin", Value::Vector(at));
            if rested {
                runtime.set_object_field(*object, "angles", Value::Vector([0.0, angles[1], 0.0]));
                finished.push((*object, "physics_finished"));
            }
            !rested
        });
    });
}

fn apply_entity_links(world: &mut World) {
    let mut runtime = world.resource_mut::<Runtime>();
    let linked: Vec<(u64, Link)> = runtime
        .entities
        .iter()
        .filter_map(|(id, e)| Some((*id, e.linked_to.clone()?)))
        .collect();
    for (id, link) in linked {
        if !runtime.live(&link.parent) {
            runtime.entities.get_mut(&id).unwrap().linked_to = None;
            continue;
        }
        let field = |runtime: &mut Runtime, name| match runtime.object_field(link.parent, name) {
            Value::Vector(v) => v,
            _ => [0.0; 3],
        };
        let (base, base_angles) = (field(&mut runtime, "origin"), field(&mut runtime, "angles"));
        let axis = math_iw4::angles_to_axis(base_angles);
        let offset = link.tag_offset.unwrap_or([0.0; 3]);
        let local = std::array::from_fn(|i| offset[i] + link.origin[i]);
        let (child_axis, origin) =
            math_iw4::matrix_multiply43(math_iw4::angles_to_axis(link.angles), local, axis, base);
        runtime.set_object_field(id, "origin", Value::Vector(origin));
        runtime.set_object_field(
            id,
            "angles",
            Value::Vector(math_iw4::axis_to_angles(child_axis)),
        );
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Motion {
    pub field: &'static str,
    pub path: MotionPath,
    pub start_ms: i64,
    pub duration_ms: i64,
    pub done: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MotionPath {
    Linear {
        from: [f32; 3],
        to: [f32; 3],
        accel: f32,
        decel: f32,
    },
    Ballistic {
        from: [f32; 3],
        velocity: [f32; 3],
    },
}

const GRAVITY: f32 = 800.0;

impl Motion {
    pub(crate) fn sample(&self, now: i64) -> ([f32; 3], [f32; 3]) {
        let elapsed = (now - self.start_ms).clamp(0, self.duration_ms);
        let t = elapsed as f32 / 1000.0;
        match self.path {
            MotionPath::Linear {
                from,
                to,
                accel,
                decel,
            } => {
                let total = self.duration_ms as f32 / 1000.0;
                let peak = 2.0 / (2.0 * total - accel - decel);
                let cruise_end = total - decel;
                let (fraction, rate) = if t < accel {
                    (0.5 * peak * t * t / accel, peak * t / accel)
                } else if t <= cruise_end {
                    (0.5 * peak * accel + peak * (t - accel), peak)
                } else {
                    let u = t - cruise_end;
                    (
                        0.5 * peak * accel + peak * (cruise_end - accel) + peak * u
                            - 0.5 * peak * u * u / decel,
                        peak * (1.0 - u / decel),
                    )
                };
                let fraction = if elapsed >= self.duration_ms {
                    1.0
                } else {
                    fraction
                };
                let rate = if elapsed >= self.duration_ms {
                    0.0
                } else {
                    rate
                };
                (
                    std::array::from_fn(|i| from[i] + (to[i] - from[i]) * fraction),
                    std::array::from_fn(|i| (to[i] - from[i]) * rate),
                )
            }
            MotionPath::Ballistic { from, velocity } => {
                let mut p: [f32; 3] = std::array::from_fn(|i| from[i] + velocity[i] * t);
                p[2] -= 0.5 * GRAVITY * t * t;
                let mut v = velocity;
                v[2] -= GRAVITY * t;
                (p, v)
            }
        }
    }
}
