use std::sync::Arc;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Undefined,
    Int(i32),
    Float(f32),
    String(Arc<str>),
    Vector([f32; 3]),
    Object(u64),
    Array(u64),
    Function(u32),
    Builtin(u32),
    LocalizedString(Arc<str>),
    Animation { tree: Arc<str>, name: Arc<str> },
    AnimationTree(Arc<str>),
}

impl Value {
    pub fn string(text: &str) -> Self {
        Self::String(text.into())
    }

    pub fn level() -> Self {
        Self::Object(0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ArrayKey {
    Integer(i32),
    String(Arc<str>),
}
