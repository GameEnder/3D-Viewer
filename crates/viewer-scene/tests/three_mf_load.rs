// crates/viewer-scene/tests/three_mf_load.rs
use std::io::Write;

const CONTENT_TYPES: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>
"##;
const RELS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"##;
/// Build a minimal but structurally valid 3MF package: [Content_Types].xml,
/// _rels/.rels, and one model part stored at `model_path`.
fn write_zip(path: &std::path::Path, model_path: &str, model: &str) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip.start_file("[Content_Types].xml", opts).unwrap();
    zip.write_all(CONTENT_TYPES.as_bytes()).unwrap();
    zip.start_file("_rels/.rels", opts).unwrap();
    zip.write_all(RELS.as_bytes()).unwrap();
    zip.start_file(model_path, opts).unwrap();
    zip.write_all(model.as_bytes()).unwrap();
    zip.finish().unwrap();
}

fn assert_parse_error_3mf(path: &std::path::Path, needle: &str) {
    let err = viewer_scene::load(path).map(|_| ()).unwrap_err();
    assert!(
        matches!(err, viewer_scene::LoadError::Parse { format: "3MF", .. }),
        "expected 3MF Parse error, got: {err}"
    );
    assert!(err.to_string().contains(needle), "unexpected error: {err}");
}

const MODEL_ONE_TRIANGLE_RED: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="2">
      <base name="Red" displaycolor="#FF0000"/>
    </basematerials>
    <object id="1" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="10" y="0" z="0"/>
          <vertex x="0" y="10" z="0"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2"/>
        </triangles>
      </mesh>
    </object>
  </resources>
  <build>
    <item objectid="1"/>
  </build>
</model>
"##;
#[test]
fn loads_minimal_3mf_one_triangle_red_base_material() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("t.3mf");
    write_zip(&p, "3D/3dmodel.model", MODEL_ONE_TRIANGLE_RED);
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 3);
    assert_eq!(s.tri_count, 1);
    // base material #FF0000 → linear red, fully opaque
    let bc = s.materials[0].base_color;
    assert!(
        (bc[0] - 0.5).abs() < 1e-6,
        "r = {} (brightness-capped)",
        bc[0]
    );
    assert_eq!(bc[1], 0.0);
    assert_eq!(bc[2], 0.0);
    assert_eq!(bc[3], 1.0);
    assert!(matches!(
        s.materials[0].alpha_mode,
        viewer_scene::AlphaMode::Opaque
    ));
    // triangle carries no pid/p1 → single default material assignment
    assert_eq!(s.materials.len(), 2);
    assert_eq!(s.meshes[0].material, 1);
    assert_eq!(s.meshes[0].indices, [0, 1, 2]);
    // normalized: unit bounding sphere, base on ground, centered on x/z
    assert!((s.bounds.bounding_sphere_radius() - 1.0).abs() < 1e-4);
    assert!(s.bounds.min.y.abs() < 1e-5);
    assert!(s.bounds.center().x.abs() < 1e-5);
    assert!(s.bounds.center().z.abs() < 1e-5);
}

const MODEL_TWO_OBJECTS_MAPPED: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="3">
      <base name="Red" displaycolor="#FF0000"/>
      <base name="Green" displaycolor="#00FF00"/>
    </basematerials>
    <object id="1" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="1" y="0" z="0"/>
          <vertex x="0" y="1" z="0"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2" pid="3" p1="0"/>
        </triangles>
      </mesh>
    </object>
    <object id="2" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="2" y="0" z="0"/>
          <vertex x="0" y="2" z="0"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2" pid="3" p1="1"/>
        </triangles>
      </mesh>
    </object>
  </resources>
  <build>
    <item objectid="1"/>
    <item objectid="2"/>
  </build>
</model>
"##;
#[test]
fn multi_object_build_with_pid_p1_material_mapping() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("multi.3mf");
    write_zip(&p, "3D/3dmodel.model", MODEL_TWO_OBJECTS_MAPPED);
    let s = viewer_scene::load(&p).unwrap();
    // two items → two instance nodes under the root
    assert_eq!(s.nodes[s.root].children.len(), 2);
    // one material per base, no extra default (every triangle is mapped)
    assert_eq!(s.materials.len(), 2);
    assert!((s.materials[0].base_color[0] - 0.5).abs() < 1e-6);
    assert!((s.materials[1].base_color[1] - 0.5).abs() < 1e-6);
    assert_eq!(s.meshes[0].material, 0); // p1=0 → red
    assert_eq!(s.meshes[1].material, 1); // p1=1 → green
    assert_eq!(s.tri_count, 2);
    assert_eq!(s.meshes[0].positions.len(), 3);
    assert_eq!(s.meshes[1].positions.len(), 3);
}

const MODEL_ONE_UNMAPPED_OBJECT: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <object id="7" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="1" y="0" z="0"/>
          <vertex x="0" y="1" z="0"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2"/>
        </triangles>
      </mesh>
    </object>
  </resources>
  <build>
    <item objectid="7"/>
    <item objectid="7"/>
  </build>
</model>
"##;
#[test]
fn two_items_instancing_same_object_share_one_mesh() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("instance.3mf");
    write_zip(&p, "3D/3dmodel.model", MODEL_ONE_UNMAPPED_OBJECT);
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.nodes[s.root].children.len(), 2);
    // the object's geometry is uploaded once; both nodes reference it
    assert_eq!(s.meshes.len(), 1);
    assert_eq!(s.nodes[s.nodes[s.root].children[0]].meshes, [0]);
    assert_eq!(s.nodes[s.nodes[s.root].children[1]].meshes, [0]);
    // no basematerials → single default material
    assert_eq!(s.materials.len(), 1);
    assert_eq!(s.materials[0].base_color, [0.5, 0.5, 0.5, 1.0]);
    assert_eq!(s.tri_count, 1);
}

#[test]
fn out_of_range_triangle_index_is_parse_error() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let bad = MODEL_ONE_TRIANGLE_RED.replace(
        "<triangle v1=\"0\" v2=\"1\" v3=\"2\"/>",
        "<triangle v1=\"5\" v2=\"1\" v3=\"2\"/>",
    );
    let p = dir.join("bad-index.3mf");
    write_zip(&p, "3D/3dmodel.model", &bad);
    assert_parse_error_3mf(&p, "out of range");
}

const MODEL_COLORS: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
  <resources>
    <basematerials id="3">
      <base name="Short" displaycolor="#F00"/>
      <base name="Translucent" displaycolor="#00FF0080"/>
    </basematerials>
    <object id="1" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="1" y="0" z="0"/>
          <vertex x="0" y="1" z="0"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2" pid="3" p1="0"/>
        </triangles>
      </mesh>
    </object>
    <object id="2" type="model">
      <mesh>
        <vertices>
          <vertex x="0" y="0" z="0"/>
          <vertex x="1" y="0" z="0"/>
          <vertex x="0" y="1" z="0"/>
        </vertices>
        <triangles>
          <triangle v1="0" v2="1" v3="2" pid="3" p1="1"/>
        </triangles>
      </mesh>
    </object>
  </resources>
  <build>
    <item objectid="1"/>
    <item objectid="2"/>
  </build>
</model>
"##;
#[test]
fn colors_rgb3_rrggbbaa_and_alpha_blend() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("colors.3mf");
    write_zip(&p, "3D/3dmodel.model", MODEL_COLORS);
    let s = viewer_scene::load(&p).unwrap();
    // #F00 expands to #FF0000 → linear red, opaque (brightness-capped 0.5)
    assert_eq!(s.materials[0].base_color[0], 0.5);
    assert_eq!(s.materials[0].base_color[3], 1.0);
    assert!(matches!(
        s.materials[0].alpha_mode,
        viewer_scene::AlphaMode::Opaque
    ));
    // #00FF0080: alpha 0x80 = 128/255 → opacity in base_color[3], AlphaMode::Blend
    assert!((s.materials[1].base_color[1] - 0.5).abs() < 1e-6);
    assert!((s.materials[1].base_color[3] - 128.0 / 255.0).abs() < 1e-6);
    assert!(matches!(
        s.materials[1].alpha_mode,
        viewer_scene::AlphaMode::Blend
    ));
    assert_eq!(s.meshes[0].material, 0);
    assert_eq!(s.meshes[1].material, 1);
}

const MODEL_PREFIXED: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<c:model xmlns:c="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" unit="millimeter">
  <c:resources>
    <c:object id="1" type="model">
      <c:mesh>
        <c:vertices>
          <c:vertex x="0" y="0" z="0"/>
          <c:vertex x="1" y="0" z="0"/>
          <c:vertex x="0" y="1" z="0"/>
        </c:vertices>
        <c:triangles>
          <c:triangle v1="0" v2="1" v3="2"/>
        </c:triangles>
      </c:mesh>
    </c:object>
  </c:resources>
  <c:build>
    <c:item objectid="1"/>
  </c:build>
</c:model>
"##;
#[test]
fn prefixed_core_namespace_matches_local_names() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("prefixed.3mf");
    write_zip(&p, "3D/3dmodel.model", MODEL_PREFIXED);
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 3);
    assert_eq!(s.tri_count, 1);
}

#[test]
fn model_part_found_by_fallback_scan_of_3d_directory() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("fallback.3mf");
    // not the canonical name — the loader must scan 3D/*.model
    write_zip(&p, "3D/backup.model", MODEL_ONE_TRIANGLE_RED);
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 3);
    assert_eq!(s.tri_count, 1);
}

fn model_with_resources(resources: &str, build: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <model unit=\"millimeter\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n\
         <resources>{resources}</resources>\n<build>{build}</build>\n</model>\n"
    )
}

const SOUP_OBJECT: &str = "<object id=\"1\" type=\"model\"><mesh><vertices>\
<vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/>\
<vertex x=\"0\" y=\"1\" z=\"0\"/></vertices><triangles/></mesh></object>";

#[test]
fn object_without_triangles_loads_as_point_soup() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let model = model_with_resources(SOUP_OBJECT, "<item objectid=\"1\"/>");
    let p = dir.join("soup.3mf");
    write_zip(&p, "3D/3dmodel.model", &model);
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.meshes[0].positions.len(), 3);
    assert!(s.meshes[0].indices.is_empty());
    assert_eq!(s.tri_count, 1); // soup: positions/3
}

#[test]
fn empty_build_falls_back_to_instancing_all_objects() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let model = model_with_resources(SOUP_OBJECT, "");
    let p = dir.join("empty-build.3mf");
    write_zip(&p, "3D/3dmodel.model", &model);
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.nodes[s.root].children.len(), 1);
    assert_eq!(s.tri_count, 1);
    assert!(
        s.warnings.iter().any(|w| w.contains("no <build> items")),
        "warnings: {:?}",
        s.warnings
    );
}

const UNKNOWN_PID_OBJECT: &str = "<object id=\"1\" type=\"model\"><mesh><vertices>\
<vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/>\
<vertex x=\"0\" y=\"1\" z=\"0\"/></vertices>\
<triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\" pid=\"9\" p1=\"0\"/></triangles>\
</mesh></object>";

#[test]
fn unknown_material_group_falls_back_to_default_with_warning() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let model = model_with_resources(UNKNOWN_PID_OBJECT, "<item objectid=\"1\"/>");
    let p = dir.join("unknown-pid.3mf");
    write_zip(&p, "3D/3dmodel.model", &model);
    let s = viewer_scene::load(&p).unwrap();
    // pid=9 references no basematerials resource → default material, warning
    assert_eq!(s.materials.len(), 1);
    assert_eq!(s.meshes[0].material, 0);
    assert!(
        s.warnings
            .iter()
            .any(|w| w.contains("material group 9 not found")),
        "warnings: {:?}",
        s.warnings
    );
}

const BAD_P1_RESOURCES: &str = "<basematerials id=\"2\">\
<base name=\"Red\" displaycolor=\"#FF0000\"/></basematerials>\
<object id=\"1\" type=\"model\"><mesh><vertices>\
<vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/>\
<vertex x=\"0\" y=\"1\" z=\"0\"/></vertices>\
<triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\" pid=\"2\" p1=\"5\"/></triangles>\
</mesh></object>";

#[test]
fn material_index_out_of_range_is_parse_error() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let model = model_with_resources(BAD_P1_RESOURCES, "<item objectid=\"1\"/>");
    let p = dir.join("bad-p1.3mf");
    write_zip(&p, "3D/3dmodel.model", &model);
    assert_parse_error_3mf(&p, "p1=5 out of range");
}

const BAD_COLOR_OBJECT: &str = "<basematerials id=\"2\">\
<base name=\"Junk\" displaycolor=\"not-a-color\"/></basematerials>\
<object id=\"1\" type=\"model\"><mesh><vertices>\
<vertex x=\"0\" y=\"0\" z=\"0\"/><vertex x=\"1\" y=\"0\" z=\"0\"/>\
<vertex x=\"0\" y=\"1\" z=\"0\"/></vertices>\
<triangles><triangle v1=\"0\" v2=\"1\" v3=\"2\" pid=\"2\" p1=\"0\"/></triangles>\
</mesh></object>";

#[test]
fn malformed_displaycolor_warns_and_uses_default_color() {
    let dir = std::env::temp_dir().join("viewer-3mf-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let model = model_with_resources(BAD_COLOR_OBJECT, "<item objectid=\"1\"/>");
    let p = dir.join("bad-color.3mf");
    write_zip(&p, "3D/3dmodel.model", &model);
    let s = viewer_scene::load(&p).unwrap();
    assert_eq!(s.materials[0].base_color, [0.5, 0.5, 0.5, 1.0]);
    assert!(
        s.warnings
            .iter()
            .any(|w| w.contains("malformed displaycolor")),
        "warnings: {:?}",
        s.warnings
    );
}
