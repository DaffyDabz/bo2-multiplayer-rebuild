use super::args::kind;
use crate::script::{ArrayKey, Runtime, Value};
use bevy_ecs::prelude::World;

pub(crate) fn array_values(world: &World, value: &Value) -> Result<Vec<Value>, String> {
    let Value::Array(id) = value else {
        return Err(format!("{} is not an array", kind(value)));
    };
    Ok(world
        .resource::<Runtime>()
        .arrays
        .get(id)
        .ok_or("invalid array reference")?
        .values()
        .cloned()
        .collect())
}

pub(crate) fn new_array(world: &mut World, values: Vec<Value>) -> Result<Value, String> {
    let mut runtime = world.resource_mut::<Runtime>();
    let id = runtime.next_object;
    runtime.next_object = id.checked_add(1).ok_or("object identifier exhausted")?;
    runtime.arrays.insert(
        id,
        values
            .into_iter()
            .enumerate()
            .map(|(i, v)| (ArrayKey::Integer(i as i32), v))
            .collect(),
    );
    Ok(Value::Array(id))
}
