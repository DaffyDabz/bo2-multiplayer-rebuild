use bevy::prelude::{Entity, Message, Resource};

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivePad(pub Option<Entity>);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PromptStyle {
    Xbox,
    PlayStation,
    #[default]
    Generic,
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputDevices {
    pub pad_prompts: bool,
    pub style: PromptStyle,
    pub focused: bool,
    pub aiming_with_pad: bool,
}

#[derive(Message)]
pub struct TestControllerRumble;
