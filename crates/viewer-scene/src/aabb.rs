// crates/viewer-scene/src/aabb.rs
use glam::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    pub fn from_points(points: impl Iterator<Item = [f32; 3]>) -> Self {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for p in points {
            let p = Vec3::from(p);
            min = min.min(p);
            max = max.max(p);
        }
        Self { min, max }
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn bounding_sphere_radius(&self) -> f32 {
        (self.max - self.center()).length().max(1e-6)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_box() {
        let a = Aabb::from_points([[-0.5, -0.5, -0.5], [0.5, 0.5, 0.5]].into_iter());
        assert_eq!(a.min, Vec3::splat(-0.5));
        assert_eq!(a.max, Vec3::splat(0.5));
        assert_eq!(a.center(), Vec3::ZERO);
        assert!((a.bounding_sphere_radius() - 0.5 * 3f32.sqrt()).abs() < 1e-5);
    }
}
