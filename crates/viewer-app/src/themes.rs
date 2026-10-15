// crates/viewer-app/src/themes.rs
//! The original Microsoft 3D Viewer's 9 lighting themes, ported verbatim
//! from its `Themes.json` asset (Task 11). The RGB triples, intensities,
//! rig rotations and names are an exact-value contract — do not "fix",
//! round or reorder them.
//!
//! Intensities use the original app's percent scale (100 = 1.0 multiplier);
//! colors are sRGB u8 and are converted to linear on the CPU before they
//! reach `LightInput` / the environment ambient.

use viewer_render::LightInput;

pub struct ThemeLight {
    pub rgb: [u8; 3],
    pub intensity: f32,
}
pub struct Theme {
    pub name: &'static str,
    pub lights: [ThemeLight; 3],
    pub env: ThemeLight,
    pub rig_rot_deg: f32,
}

const fn t(rgb: [u8; 3], i: f32) -> ThemeLight {
    ThemeLight { rgb, intensity: i }
}

pub const THEMES: [Theme; 9] = [
    Theme {
        name: "Default",
        lights: [
            t([255, 255, 255], 100.0),
            t([255, 255, 255], 100.0),
            t([255, 255, 255], 100.0),
        ],
        env: t([255, 255, 255], 40.0),
        rig_rot_deg: 0.0,
    },
    Theme {
        name: "Campfire",
        lights: [
            t([53, 34, 122], 48.0),
            t([255, 255, 188], 15.0),
            t([255, 68, 0], 200.0),
        ],
        env: t([33, 37, 104], 31.0),
        rig_rot_deg: 145.5,
    },
    Theme {
        name: "Cotton Candy",
        lights: [
            t([255, 40, 201], 160.0),
            t([110, 234, 226], 164.0),
            t([211, 46, 46], 66.0),
        ],
        env: t([161, 198, 197], 30.0),
        rig_rot_deg: 189.4,
    },
    Theme {
        name: "Glow in the Dark",
        lights: [
            t([45, 255, 52], 180.0),
            t([232, 232, 232], 19.0),
            t([0, 0, 0], 0.0),
        ],
        env: t([0, 2, 158], 82.0),
        rig_rot_deg: 345.0,
    },
    Theme {
        name: "Moonlight",
        lights: [
            t([255, 250, 173], 97.0),
            t([52, 46, 119], 74.0),
            t([255, 255, 255], 13.0),
        ],
        env: t([0, 35, 150], 34.0),
        rig_rot_deg: 348.0,
    },
    Theme {
        name: "Paint3D",
        lights: [
            t([255, 194, 168], 88.0),
            t([73, 76, 255], 117.0),
            t([0, 0, 0], 0.0),
        ],
        env: t([255, 255, 255], 58.0),
        rig_rot_deg: 225.0,
    },
    Theme {
        name: "Silhouette",
        lights: [
            t([0, 0, 0], 0.0),
            t([255, 255, 255], 200.0),
            t([255, 255, 255], 0.0),
        ],
        env: t([255, 255, 255], 3.0),
        rig_rot_deg: 240.8,
    },
    Theme {
        name: "Studio",
        lights: [
            t([151, 199, 201], 111.0),
            t([255, 251, 232], 92.0),
            t([255, 248, 239], 43.0),
        ],
        env: t([211, 211, 211], 103.0),
        rig_rot_deg: 155.5,
    },
    Theme {
        name: "Sunset",
        lights: [
            t([255, 144, 0], 200.0),
            t([84, 106, 255], 170.0),
            t([0, 0, 0], 0.0),
        ],
        env: t([81, 68, 147], 64.0),
        rig_rot_deg: 46.0,
    },
];

fn srgb_to_linear(c: u8) -> f32 {
    let v = c as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn lin3(rgb: [u8; 3]) -> [f32; 3] {
    [
        srgb_to_linear(rgb[0]),
        srgb_to_linear(rgb[1]),
        srgb_to_linear(rgb[2]),
    ]
}

/// Light rig: 3 lights, 120° apart, elevation 40°, radius 2.2 around the model.
pub fn light_pos(i: usize, rig_rot_deg: f32) -> [f32; 3] {
    const RADIUS: f32 = 2.2;
    let elev = 40.0f32.to_radians();
    let az = (rig_rot_deg + 120.0 * i as f32).to_radians();
    [
        RADIUS * elev.cos() * az.sin(),
        RADIUS * elev.sin(),
        RADIUS * elev.cos() * az.cos(),
    ]
}

/// Theme intensities: 100 = 1.0 multiplier (original's percent scale).
pub fn frame_lights(
    theme: &Theme,
    user_mult: [f32; 3],
    env_mult: f32,
) -> ([LightInput; 3], ([f32; 3], f32)) {
    let lights = std::array::from_fn(|i| LightInput {
        pos: light_pos(i, theme.rig_rot_deg),
        color: lin3(theme.lights[i].rgb),
        intensity: theme.lights[i].intensity / 100.0 * user_mult[i],
        shadows: theme.lights[i].intensity > 0.0 && user_mult[i] > 0.0,
    });
    let env = (lin3(theme.env.rgb), theme.env.intensity / 100.0 * env_mult);
    (lights, env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_themes_and_rig_geometry() {
        assert_eq!(THEMES.len(), 9);
        assert_eq!(THEMES[0].name, "Default");
        assert_eq!(THEMES[8].name, "Sunset");
        let p = light_pos(0, 0.0);
        assert!((p[1] - 2.2 * 40.0f32.to_radians().sin()).abs() < 1e-4); // y ≈ 1.414
        assert!(p[2] > 1.5); // in front (+Z)
        let q = light_pos(1, 0.0);
        let angle = (q[0].atan2(q[2]) - p[0].atan2(p[2])).to_degrees();
        assert!((angle - 120.0).abs() < 0.01);
        let (lights, env) = frame_lights(&THEMES[7], [1.0; 3], 1.0);
        assert!(lights[0].shadows);
        let (_, e) = env;
        assert!((e - 1.03).abs() < 1e-4); // Studio env 103/100
    }

    /// Exact-value contract: every rgb triple, intensity, env and rig
    /// rotation transcribed independently from the original app's
    /// `Themes.json` (task brief) and diffed against `THEMES`.
    #[test]
    fn theme_data_matches_original_themes_json() {
        type Entry = (&'static str, [([u8; 3], f32); 3], ([u8; 3], f32), f32);
        let expected: [Entry; 9] = [
            (
                "Default",
                [
                    ([255, 255, 255], 100.0),
                    ([255, 255, 255], 100.0),
                    ([255, 255, 255], 100.0),
                ],
                ([255, 255, 255], 40.0),
                0.0,
            ),
            (
                "Campfire",
                [
                    ([53, 34, 122], 48.0),
                    ([255, 255, 188], 15.0),
                    ([255, 68, 0], 200.0),
                ],
                ([33, 37, 104], 31.0),
                145.5,
            ),
            (
                "Cotton Candy",
                [
                    ([255, 40, 201], 160.0),
                    ([110, 234, 226], 164.0),
                    ([211, 46, 46], 66.0),
                ],
                ([161, 198, 197], 30.0),
                189.4,
            ),
            (
                "Glow in the Dark",
                [
                    ([45, 255, 52], 180.0),
                    ([232, 232, 232], 19.0),
                    ([0, 0, 0], 0.0),
                ],
                ([0, 2, 158], 82.0),
                345.0,
            ),
            (
                "Moonlight",
                [
                    ([255, 250, 173], 97.0),
                    ([52, 46, 119], 74.0),
                    ([255, 255, 255], 13.0),
                ],
                ([0, 35, 150], 34.0),
                348.0,
            ),
            (
                "Paint3D",
                [
                    ([255, 194, 168], 88.0),
                    ([73, 76, 255], 117.0),
                    ([0, 0, 0], 0.0),
                ],
                ([255, 255, 255], 58.0),
                225.0,
            ),
            (
                "Silhouette",
                [
                    ([0, 0, 0], 0.0),
                    ([255, 255, 255], 200.0),
                    ([255, 255, 255], 0.0),
                ],
                ([255, 255, 255], 3.0),
                240.8,
            ),
            (
                "Studio",
                [
                    ([151, 199, 201], 111.0),
                    ([255, 251, 232], 92.0),
                    ([255, 248, 239], 43.0),
                ],
                ([211, 211, 211], 103.0),
                155.5,
            ),
            (
                "Sunset",
                [
                    ([255, 144, 0], 200.0),
                    ([84, 106, 255], 170.0),
                    ([0, 0, 0], 0.0),
                ],
                ([81, 68, 147], 64.0),
                46.0,
            ),
        ];
        for (theme, (name, lights, env, rot)) in THEMES.iter().zip(expected) {
            assert_eq!(theme.name, name, "theme order/name");
            for (slot, (rgb, intensity)) in theme.lights.iter().zip(lights) {
                assert_eq!(slot.rgb, rgb, "{name} light rgb");
                assert_eq!(slot.intensity, intensity, "{name} light intensity");
            }
            assert_eq!(theme.env.rgb, env.0, "{name} env rgb");
            assert_eq!(theme.env.intensity, env.1, "{name} env intensity");
            assert_eq!(theme.rig_rot_deg, rot, "{name} rig rotation");
        }
    }

    /// Percent scale + sRGB conversion endpoints.
    #[test]
    fn percent_scale_and_srgb_conversion() {
        assert_eq!(lin3([0, 0, 0]), [0.0, 0.0, 0.0]);
        assert_eq!(lin3([255, 255, 255]), [1.0, 1.0, 1.0]);
        assert!((srgb_to_linear(128) - 0.2158).abs() < 1e-3);
        // Default theme: all intensities normalize to exactly 1.0, env 0.4.
        let (lights, env) = frame_lights(&THEMES[0], [1.0; 3], 1.0);
        for l in &lights {
            assert_eq!(l.intensity, 1.0);
            assert_eq!(l.color, [1.0, 1.0, 1.0]);
        }
        assert_eq!(env, ([1.0, 1.0, 1.0], 0.4));
    }

    /// Shadows flag: off for zero-intensity lights and zero user multipliers.
    #[test]
    fn shadows_follow_intensity_and_user_mult() {
        // Silhouette: light 0 has intensity 0, light 1 has 200.
        let (lights, _) = frame_lights(&THEMES[6], [1.0; 3], 1.0);
        assert!(!lights[0].shadows);
        assert!(lights[1].shadows);
        // Zero user multiplier: light off AND shadow off (Studio light 0 has
        // intensity 111 > 0, so the flag is killed by the multiplier alone).
        let (lights, _) = frame_lights(&THEMES[7], [0.0, 1.0, 1.0], 1.0);
        assert!(!lights[0].shadows);
        assert_eq!(lights[0].intensity, 0.0);
        // Environment multiplier scales the percent-scaled env intensity.
        let (_, env) = frame_lights(&THEMES[6], [1.0; 3], 2.0);
        assert!((env.1 - 0.06).abs() < 1e-4);
    }

    /// Rig rotation shifts azimuths; light 2 sits 240° around; elevation and
    /// radius hold for every slot at an arbitrary rotation.
    #[test]
    fn rig_rotation_and_full_geometry() {
        let base = light_pos(0, 0.0);
        let rotated = light_pos(0, 30.0);
        let delta = (rotated[0].atan2(rotated[2]) - base[0].atan2(base[2])).to_degrees();
        assert!((delta - 30.0).abs() < 1e-3);
        // Azimuths (normalized to [0, 360)) sit at rig_rot + 120°·i.
        let az = |p: [f32; 3]| p[0].atan2(p[2]).to_degrees().rem_euclid(360.0);
        assert!((az(base)).abs() < 1e-3);
        assert!((az(light_pos(1, 0.0)) - 120.0).abs() < 1e-3);
        assert!((az(light_pos(2, 0.0)) - 240.0).abs() < 1e-3);
        for i in 0..3 {
            let p = light_pos(i, 123.4);
            let radius = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((radius - 2.2).abs() < 1e-4, "light {i} radius");
            let elev = p[1].atan2((p[0] * p[0] + p[2] * p[2]).sqrt()).to_degrees();
            assert!((elev - 40.0).abs() < 1e-3, "light {i} elevation");
        }
    }
}

#[cfg(test)]
mod thumbnail_dump {
    use super::*;
    use egui_wgpu::wgpu;

    #[test]
    #[ignore = "dump theme thumbnails to %TEMP%\\opencode"]
    fn dump() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            ..Default::default()
        }))
        .unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        for theme in THEMES.iter() {
            let (lights, (env, env_i)) = frame_lights(theme, [1.0; 3], 1.0);
            let img = viewer_render::preview::render(
                &device,
                &queue,
                96,
                viewer_render::FrameParams {
                    view_proj: viewer_render::preview::camera_view_proj(),
                    camera_pos: viewer_render::preview::camera_pos(),
                    turntable_deg: 0.0,
                    model_scale: 1.0,
                    lights,
                    env_color: env,
                    env_intensity: env_i,
                    anim: None,
                    settings: viewer_render::RenderSettings {
                        clear_color: [0.03, 0.03, 0.035],
                        wireframe: false,
                        ground: true,
                        grid: false,
                        bloom: false,
                        exposure: 0.5,
                        contrast: 1.66,
                        ground_opacity: 0.4,
                        shadows: true,
                    },
                },
            );
            img.save(format!(
                r"C:\Users\Steven\AppData\Local\Temp\opencode\thumb-{}.png",
                theme.name
            ))
            .unwrap();
        }
    }
}
