// crates/viewer-app/src/camera.rs
use glam::{Mat4, Vec3};

#[allow(dead_code)]
pub const DEFAULT_ELEVATION_DEG: f32 = 15.0;
#[allow(dead_code)]
pub const DEFAULT_ROTATION_DEG: f32 = -90.0;
#[allow(dead_code)]
pub const DEFAULT_FOV_DEG: f32 = 45.0;

/// Spherical orbit camera matching the original's (distance, elevation, rotation) model.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    pub elevation_deg: f32,
    pub rotation_deg: f32,
    pub fov_y_deg: f32,
}

#[allow(dead_code)]
impl OrbitCamera {
    pub fn fit(&mut self, center: Vec3, radius: f32) {
        self.target = center;
        self.elevation_deg = DEFAULT_ELEVATION_DEG;
        self.rotation_deg = DEFAULT_ROTATION_DEG;
        self.fov_y_deg = DEFAULT_FOV_DEG;
        self.distance = (radius / (self.fov_y_deg * 0.5).to_radians().sin()) * 1.25;
    }

    pub fn default_for(center: Vec3, radius: f32) -> Self {
        let mut c = Self {
            target: center,
            distance: 3.0,
            elevation_deg: DEFAULT_ELEVATION_DEG,
            rotation_deg: DEFAULT_ROTATION_DEG,
            fov_y_deg: DEFAULT_FOV_DEG,
        };
        c.fit(center, radius);
        c
    }

    pub fn eye(&self) -> Vec3 {
        let el = self.elevation_deg.to_radians();
        let ro = self.rotation_deg.to_radians();
        let dir = Vec3::new(el.cos() * ro.sin(), el.sin(), el.cos() * ro.cos());
        self.target + dir * self.distance
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.target, Vec3::Y)
    }

    pub fn proj_matrix(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y_deg.to_radians(), aspect.max(0.01), 0.05, 100.0)
    }

    pub fn view_proj(&self, aspect: f32) -> [[f32; 4]; 4] {
        (self.proj_matrix(aspect) * self.view_matrix()).to_cols_array_2d()
    }

    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.rotation_deg -= dx * 0.5;
        self.elevation_deg = (self.elevation_deg + dy * 0.5).clamp(-89.0, 89.0);
    }

    pub fn dolly(&mut self, mult: f32) {
        self.distance = (self.distance / mult.max(0.01)).clamp(0.4, 30.0);
    }

    pub fn pan(&mut self, dx: f32, dy: f32, aspect: f32) {
        let scale = self.distance * (self.fov_y_deg.to_radians() * 0.5).tan() / 500.0;
        let view = self.view_matrix();
        let right = Vec3::new(view.x_axis.x, view.y_axis.x, view.z_axis.x); // view row 0
        let up = Vec3::new(view.x_axis.y, view.y_axis.y, view.z_axis.y); // view row 1
        self.target += right * (-dx * scale * aspect.clamp(0.01, 3.0)) + up * (dy * scale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec3, Vec4};

    #[test]
    fn spherical_eye_matches_rotation_elevation() {
        let c = OrbitCamera::default_for(Vec3::ZERO, 1.0);
        // rotation −90° → eye on −X side (mod axis flip), elevation 15° above horizon
        assert!((c.rotation_deg - (-90.0)).abs() < 1e-4);
        assert!((c.elevation_deg - 15.0).abs() < 1e-4);
        let eye = c.eye();
        assert!(eye.x < 0.0 && eye.y > 0.0 && eye.z.abs() < 1e-4);
    }

    #[test]
    fn target_maps_to_clip_center() {
        let c = OrbitCamera::default_for(Vec3::new(0.0, 0.5, 0.0), 1.0);
        let vp = Mat4::from_cols_array_2d(&c.view_proj(16.0 / 9.0));
        let clip = vp * Vec4::new(0.0, 0.5, 0.0, 1.0);
        assert!(clip.w > 0.0);
        assert!((clip.x / clip.w).abs() < 1e-5);
        assert!((clip.y / clip.w).abs() < 1e-5);
        assert!(clip.z / clip.w > 0.0 && clip.z / clip.w < 1.0); // inside clip volume; depth range itself is pinned by depth_range_is_0_to_1
    }

    #[test]
    fn dolly_clamps() {
        let mut c = OrbitCamera::default_for(Vec3::ZERO, 1.0);
        let before = c.distance;
        c.dolly(0.001);
        assert!(c.distance > before * 0.5); // clamped, not collapsed
        c.dolly(100.0);
        assert!(c.distance < 50.0);
    }

    #[test]
    fn depth_range_is_0_to_1() {
        let c = OrbitCamera::default_for(Vec3::ZERO, 1.0);
        let vp = Mat4::from_cols_array_2d(&c.view_proj(16.0 / 9.0));
        let fwd = (c.target - c.eye()).normalize();
        // Just inside the frustum (0.06 from eye, near = 0.05): perspective_rh (0..1)
        // gives z_ndc ≈ +0.167; a GL −1..1 projection (perspective_rh_gl) would give
        // ≈ −0.667, so the sign here discriminates the two conventions.
        let inside = vp * (c.eye() + fwd * 0.06).extend(1.0);
        assert!(inside.z / inside.w > 0.0);
        assert!(inside.z / inside.w < 1.0);
        // Just in front of the near plane (0.04 from eye, clipped): z_ndc ≈ −0.250 under
        // 0..1 (GL would give ≈ −1.500); the frustum boundary sits at z_ndc = 0, not −1.
        let outside = vp * (c.eye() + fwd * 0.04).extend(1.0);
        assert!(outside.z / outside.w < 0.0);
    }
}
