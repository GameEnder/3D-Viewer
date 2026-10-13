// crates/viewer-scene/src/anim_eval.rs
//! glTF animation evaluation (Task 17): samples an [`Animation`]'s channels
//! at a time, producing per-node TRS overrides for [`crate::world_transforms`].
use glam::{Quat, Vec3};

use crate::animation::{Interpolation, Keyframes, TrsProperty};
use crate::scene::{Scene, Trs};

/// Evaluate animation `anim` at `time` (wraps at `duration` via
/// `rem_euclid`, so scrubbing past the end and negative times both loop).
/// Returns one entry per node: `Some` for nodes with channels (fields not
/// animated keep the node's static TRS), `None` for untouched nodes.
///
/// Interpolation (glTF 2.0 §"animation channels"):
/// - Step: value of the segment's first key.
/// - Linear: component lerp. For rotations this is NOT a true slerp, but
///   component-lerp + normalize (done on assignment below) is an accepted
///   viewer approximation — keyframes are <= 90deg apart in practice, and
///   the midpoint error is bounded (identity -> 45deg evaluates exactly
///   22.5deg).
/// - CubicSpline: Hermite `h00*p0 + h10*(dt*m0) + h01*p1 + h11*(dt*m1)` per
///   component — the tangents are velocity-per-second, so they scale by the
///   segment length `dt` (spec: CUBICSPLINE output = [in-tangent, value,
///   out-tangent] per key). Rotations are hermite'd on components and
///   normalized afterwards (never slerp'd).
pub fn evaluate(scene: &Scene, anim: usize, time: f32) -> Vec<Option<Trs>> {
    let mut out: Vec<Option<Trs>> = vec![None; scene.nodes.len()];
    let Some(a) = scene.animations.get(anim) else {
        return out;
    };
    let t = if a.duration > 0.0 {
        time.rem_euclid(a.duration)
    } else {
        time
    };
    for ch in &a.channels {
        let Some(node) = scene.nodes.get(ch.node) else {
            continue;
        };
        let base = &node.transform;
        let v = sample(&ch.keys, t);
        let trs = out[ch.node].get_or_insert(*base);
        match ch.property {
            TrsProperty::Translation => trs.translation = Vec3::new(v[0], v[1], v[2]),
            // Normalize after ANY interpolation (see module docs for the
            // linear-rotation note).
            TrsProperty::Rotation => {
                trs.rotation = Quat::from_xyzw(v[0], v[1], v[2], v[3]).normalize();
            }
            TrsProperty::Scale => trs.scale = Vec3::new(v[0], v[1], v[2]),
        }
    }
    out
}

/// Segment lookup: returns `(i, j)` with `times[i] <= t <= times[j]`
/// (clamped at both ends; a single key returns `(0, 0)`).
fn find_segment(times: &[f32], t: f32) -> (usize, usize) {
    if times.len() < 2 {
        return (0, times.len().saturating_sub(1));
    }
    for i in 0..times.len() - 1 {
        if t >= times[i] && t <= times[i + 1] {
            return (i, i + 1);
        }
    }
    if t < times[0] {
        (0, 1)
    } else {
        (times.len() - 2, times.len() - 1)
    }
}

/// Sample one keyframe track at `t`: `N`-dim values padded to `[f32; 4]`
/// (T/S carry xyz + 0 pad, R carries xyzw).
fn sample(k: &Keyframes, t: f32) -> [f32; 4] {
    let Some(&last_time) = k.times.last() else {
        return [0.0; 4];
    };
    if k.values.is_empty() {
        return [0.0; 4];
    }
    let last = k.values[k.values.len() - 1];
    if k.values.len() == 1 || t <= k.times[0] {
        return k.values[0];
    }
    if t >= last_time {
        return last;
    }
    let (i, j) = find_segment(&k.times, t);
    let (t0, t1) = (k.times[i], k.times[j]);
    let dt = (t1 - t0).max(1e-6);
    let u = (t - t0) / dt;
    match k.interpolation {
        Interpolation::Step => k.values[i],
        Interpolation::Linear => lerp4(k.values[i], k.values[j], u),
        Interpolation::CubicSpline => {
            // Hermite: h00*p0 + h10*(dt*m0) + h01*p1 + h11*(dt*m1).
            // Tangents scale by the segment dt (see evaluate docs).
            let m0 = k.out_tangents.get(i).copied().unwrap_or([0.0; 4]);
            let m1 = k.in_tangents.get(j).copied().unwrap_or([0.0; 4]);
            let (h00, h10, h01, h11) = (
                2.0 * u * u * u - 3.0 * u * u + 1.0,
                u * u * u - 2.0 * u * u + u,
                -2.0 * u * u * u + 3.0 * u * u,
                u * u * u - u * u,
            );
            let p0 = k.values[i];
            let p1 = k.values[j];
            let mut out = [0.0f32; 4];
            for c in 0..4 {
                out[c] = h00 * p0[c] + h10 * dt * m0[c] + h01 * p1[c] + h11 * dt * m1[c];
            }
            out
        }
    }
}

fn lerp4(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for c in 0..4 {
        o[c] = a[c] + (b[c] - a[c]) * u;
    }
    o
}

#[cfg(test)]
mod tests {
    use crate::anim_eval::evaluate;
    use crate::animation::{Animation, Channel, Interpolation, Keyframes, TrsProperty};
    use crate::scene::{Scene, Trs};

    fn scene_with_translation_anim() -> (Scene, usize) {
        let mut s = Scene::new("t".into());
        let node = s.add_node(s.root, Trs::identity(), None);
        s.nodes[node].transform = Trs::identity();
        s.animations.push(Animation {
            name: "move".into(),
            duration: 2.0,
            channels: vec![Channel {
                node,
                property: TrsProperty::Translation,
                keys: Keyframes {
                    times: vec![0.0, 2.0],
                    values: vec![[0.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]],
                    in_tangents: vec![],
                    out_tangents: vec![],
                    interpolation: Interpolation::Linear,
                },
            }],
        });
        (s, node)
    }

    #[test]
    fn linear_midpoint() {
        let (s, node) = scene_with_translation_anim();
        let pose = evaluate(&s, 0, 1.0);
        assert_eq!(pose[node].as_ref().unwrap().translation.x, 1.0);
    }

    #[test]
    fn wraps_at_duration() {
        let (s, node) = scene_with_translation_anim();
        let pose = evaluate(&s, 0, 2.5); // wraps to 0.5
        assert!((pose[node].as_ref().unwrap().translation.x - 0.5).abs() < 1e-5);
    }

    #[test]
    fn wraps_negative_time() {
        let (s, node) = scene_with_translation_anim();
        let pose = evaluate(&s, 0, -0.5); // rem_euclid -> 1.5
        assert!((pose[node].as_ref().unwrap().translation.x - 1.5).abs() < 1e-5);
    }

    #[test]
    fn rotation_lerp_normalized() {
        let mut s = Scene::new("t".into());
        let node = s.add_node(s.root, Trs::identity(), None);
        s.animations.push(Animation {
            name: "spin".into(),
            duration: 1.0,
            channels: vec![Channel {
                node,
                property: TrsProperty::Rotation,
                keys: Keyframes {
                    times: vec![0.0, 1.0],
                    values: vec![[0.0, 0.0, 0.0, 1.0], [0.0, 0.3826834, 0.0, 0.9238795]],
                    in_tangents: vec![],
                    out_tangents: vec![],
                    interpolation: Interpolation::Linear,
                },
            }],
        });
        let pose = evaluate(&s, 0, 0.5);
        let q = pose[node].as_ref().unwrap().rotation;
        assert!((q.length() - 1.0).abs() < 1e-5); // normalized after interpolation
        let angle = 2.0 * q.w.acos();
        // Controller correction over the brief: Linear rotation is
        // component-lerp + normalize (accepted approximation). At the
        // midpoint of identity -> 45deg that gives exactly the HALF angle,
        // FRAC_PI_8 = 22.5deg, not FRAC_PI_4.
        assert!((angle - std::f32::consts::FRAC_PI_8).abs() < 0.05);
    }

    #[test]
    fn step_holds_value() {
        let (mut s, node) = scene_with_translation_anim();
        s.animations[0].channels[0].keys.interpolation = Interpolation::Step;
        let pose = evaluate(&s, 0, 1.9);
        assert_eq!(pose[node].as_ref().unwrap().translation.x, 0.0); // holds first key
    }

    #[test]
    fn single_key_holds() {
        let mut s = Scene::new("t".into());
        let node = s.add_node(s.root, Trs::identity(), None);
        s.animations.push(Animation {
            name: "hold".into(),
            duration: 1.0,
            channels: vec![Channel {
                node,
                property: TrsProperty::Translation,
                keys: Keyframes {
                    times: vec![0.5],
                    values: vec![[1.0, 2.0, 3.0, 0.0]],
                    in_tangents: vec![],
                    out_tangents: vec![],
                    interpolation: Interpolation::Linear,
                },
            }],
        });
        let pose = evaluate(&s, 0, 0.0);
        assert_eq!(
            pose[node].as_ref().unwrap().translation,
            glam::Vec3::new(1.0, 2.0, 3.0)
        );
    }

    #[test]
    fn cubic_spline_zero_tangents_reduces_to_linear() {
        // Hermite with zero tangents: h00*p0 + h01*p1 == component lerp.
        let mut s = Scene::new("t".into());
        let node = s.add_node(s.root, Trs::identity(), None);
        s.animations.push(Animation {
            name: "spline".into(),
            duration: 2.0,
            channels: vec![Channel {
                node,
                property: TrsProperty::Translation,
                keys: Keyframes {
                    times: vec![0.0, 2.0],
                    values: vec![[0.0, 0.0, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0]],
                    in_tangents: vec![[0.0; 4], [0.0; 4]],
                    out_tangents: vec![[0.0; 4], [0.0; 4]],
                    interpolation: Interpolation::CubicSpline,
                },
            }],
        });
        let pose = evaluate(&s, 0, 1.0);
        assert!((pose[node].as_ref().unwrap().translation.x - 1.0).abs() < 1e-5);
    }
}
