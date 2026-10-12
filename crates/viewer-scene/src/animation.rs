// crates/viewer-scene/src/animation.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrsProperty {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    CubicSpline,
}

#[derive(Clone, Debug)]
pub struct Keyframes {
    pub times: Vec<f32>,
    /// T/S: xyz + 0 pad. R: quaternion xyzw (glTF order). One per key.
    pub values: Vec<[f32; 4]>,
    /// CubicSpline only (glTF output layout per key: [in-tangent, value, out-tangent]).
    pub in_tangents: Vec<[f32; 4]>,
    pub out_tangents: Vec<[f32; 4]>,
    pub interpolation: Interpolation,
}

#[derive(Clone, Debug)]
pub struct Channel {
    pub node: usize,
    pub property: TrsProperty,
    pub keys: Keyframes,
}

#[derive(Clone, Debug)]
pub struct Animation {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<Channel>,
}
