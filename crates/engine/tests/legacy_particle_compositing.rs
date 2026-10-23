//! Compatibility of particle effects in projects saved before v0.10.0 (#1898).

use effectcraft_keyframe::Value as KV;
use effectcraft_project::{Layer, LayerId, PropGroup};
use effectcraft_time::Tick;
use serde_json::json;

use effectcraft_engine::Session;

fn comp_with_two_solids() -> (Session, u64, u64) {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 640, "height": 360, "frameRate": 30, "duration": 10})).unwrap();
    let a = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 200, "height": 100})).unwrap()["layer"].as_u64().unwrap();
    let b = s.execute("layer.newSolid", json!({"color": "#00ff00", "width": 400, "height": 300})).unwrap()["layer"].as_u64().unwrap();
    (s, a, b)
}

fn layer(s: &Session, id: u64) -> Layer {
    s.active_comp().unwrap().layer(LayerId(id)).unwrap().clone()
}

fn apply(s: &mut Session, lid: u64, effect: &str) -> u64 {
    s.execute("effect.apply", json!({"layers": [lid], "effect": effect})).unwrap()["effects"][0].as_u64().unwrap()
}

fn effect_group(s: &Session, lid: u64, uid: u64) -> PropGroup {
    layer(s, lid).effects().unwrap().groups().find(|g| g.uid == uid).unwrap().clone()
}

fn animate_effect(s: &mut Session, lid: u64, uid: u64, path: &str, keys: &[(f64, f64)]) {
    let prop = effect_group(s, lid, uid).prop(path).unwrap().uid;
    for &(time, value) in keys {
        s.execute("prop.addKey", json!({"layer": lid, "prop": prop, "time": time, "timeBase": "comp", "value": value})).unwrap();
    }
}

/// #1898: v0.6.0 composited CC Particle Systems II over its input, including earlier
/// particle effects. Opening that project must keep both emitters and the precomp artwork.
#[test]
fn legacy_particle_compositing_survives_open_and_save() {
    let mut s = Session::default();
    s.execute("comp.new", json!({"name": "Main", "width": 96, "height": 64, "frameRate": 30, "duration": 3})).unwrap();
    let cid = s.active_comp_id().unwrap();
    let lid = s.execute("layer.newSolid", json!({"color": "#ff0000", "width": 96, "height": 64})).unwrap()["layer"].as_u64().unwrap();
    s.execute("layer.precompose", json!({"layers": [lid], "name": "Button", "mode": "leave"})).unwrap();
    for (x, color) in [(18.0, "#00ff00"), (78.0, "#0000ff")] {
        let uid = apply(&mut s, lid, "ec.sim.ccparticlesystems2");
        for (path, value) in [
            ("producer/position", json!([x, 20.0])),
            ("producer/radiusX", json!(0)),
            ("producer/radiusY", json!(0)),
            ("physics/velocity", json!(0)),
            ("physics/gravity", json!(0)),
            ("particle/particleType", json!(7)),
            ("particle/birthSize", json!(3)),
            ("particle/deathSize", json!(3)),
            ("particle/birthColor", json!(color)),
            ("particle/deathColor", json!(color)),
            ("particle/maxOpacity", json!(100)),
        ] {
            let prop = effect_group(&s, lid, uid).prop(path).unwrap().uid;
            s.execute("prop.set", json!({"layer": lid, "prop": prop, "value": value})).unwrap();
        }
        animate_effect(&mut s, lid, uid, "birthRate", &[(0.0, 1.5), (1.0, 2.5)]);
    }
    let original: Vec<_> = layer(&s, lid).effects().unwrap().groups().cloned().collect();
    let file = legacy_particle_file(&s, Some("0.6.0"));
    let dir = std::env::temp_dir().join(format!("ec-1898-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("old.ecproj").to_string_lossy().to_string();
    std::fs::write(&path, file.to_string()).unwrap();
    let mut opened = Session::default();
    opened.execute("file.open", json!({"path": path})).unwrap();
    let render = |s: &Session| s.render(cid, Tick::from_seconds_f64(0.5), Default::default());
    let frame = render(&opened);
    let visible = |img: &effectcraft_raster::Image| {
        assert_eq!(img.get(48, 48), [1.0, 0.0, 0.0, 1.0], "the precomp artwork survives");
        assert!(img.get(18, 20)[1] > 0.9, "the first particle effect survives the second");
        assert!(img.get(78, 20)[2] > 0.9, "the second particle effect is visible");
    };
    visible(&frame);
    for old in &original {
        let now = effect_group(&opened, lid, old.uid);
        assert_eq!(now.prop("birthRate"), old.prop("birthRate"), "animated values, times and ids are kept");
        assert_eq!(now.prop("producer/position"), old.prop("producer/position"));
    }
    // Importing uses the same version-aware migration before remapping ids.
    let mut imported = Session::default();
    imported.execute("file.importProject", json!({"path": path})).unwrap();
    let imported_cid = imported.project.items.values().find(|i| i.name == "Main").unwrap().id;
    // Import remaps effect uids, hence particle seeds and sub-step birth offsets. The artwork
    // and both emitters must survive, but antialiased edges need not be pixel-identical.
    visible(&imported.render(imported_cid, Tick::from_seconds_f64(0.5), Default::default()));
    let imported_fx: Vec<_> = imported.project.comp(imported_cid).unwrap().layers[0].effects().unwrap().groups().collect();
    assert_eq!(imported_fx.len(), original.len());
    for (now, old) in imported_fx.iter().zip(&original) {
        assert_eq!(now.get("compositeWithOriginal").unwrap().value, KV::Bool(true));
        assert_eq!(now.get("birthRate").unwrap().keys, old.get("birthRate").unwrap().keys);
    }
    opened.execute("file.save", json!({})).unwrap();
    let mut reopened = Session::default();
    reopened.execute("file.open", json!({"path": path})).unwrap();
    assert!(render(&reopened) == frame, "saving as a current project keeps the migrated behavior");
    assert_eq!(opened.project, reopened.project, "migration is idempotent");
    std::fs::remove_dir_all(dir).unwrap();
}

fn legacy_particle_file(s: &Session, version: Option<&str>) -> serde_json::Value {
    let mut file: serde_json::Value = serde_json::from_str(&s.project.to_file_json().unwrap()).unwrap();
    match version {
        Some(v) => file["savedBy"] = json!(v),
        None => {
            file.as_object_mut().unwrap().remove("savedBy");
        }
    }
    // The compatibility control did not exist in v0.6.0.
    fn remove_control(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Array(a) => {
                a.retain(|n| n.get("match").and_then(serde_json::Value::as_str) != Some("compositeWithOriginal"));
                for n in a {
                    remove_control(n);
                }
            }
            serde_json::Value::Object(o) => {
                for n in o.values_mut() {
                    remove_control(n);
                }
            }
            _ => {}
        }
    }
    remove_control(&mut file);
    file
}

#[test]
fn legacy_particle_compositing_uses_writer_version_and_preserves_explicit_choices() {
    let (mut s, lid, _) = comp_with_two_solids();
    for id in ["ec.sim.ccparticlesystems2", "ec.sim.ccparticleworld"] {
        apply(&mut s, lid, id);
    }
    for (version, legacy) in [
        (Some("0.6.0"), true),
        (Some("0.9.2"), true),
        (Some("0.9.2+build"), true),
        (Some("0.10.0"), false),
        (Some("0.16.0"), false),
        (Some("1.0.0"), false),
        (None, false),
        (Some("unknown"), false),
        (Some("0.6"), false),
    ] {
        let file = legacy_particle_file(&s, version);
        let mut p = effectcraft_project::Project::from_json(&file.to_string()).unwrap();
        effectcraft_engine::upgrade_effects(&mut p);
        let fx = p.comp_mut(s.active_comp_id().unwrap()).unwrap().layer_mut(LayerId(lid)).unwrap().props.sub_mut("effects").unwrap();
        for n in &mut fx.children {
            let effectcraft_project::Node::Group(g) = n else { continue };
            assert_eq!(g.get("compositeWithOriginal").unwrap().value, KV::Bool(legacy), "{version:?}: {}", g.match_id);
            g.get_mut("compositeWithOriginal").unwrap().value = KV::Bool(!legacy);
        }
        // An explicit value wins even when the file still names the old writer.
        let mut explicit: serde_json::Value = serde_json::from_str(&p.to_file_json().unwrap()).unwrap();
        explicit["savedBy"] = json!("0.6.0");
        let mut again = effectcraft_project::Project::from_json(&explicit.to_string()).unwrap();
        effectcraft_engine::upgrade_effects(&mut again);
        assert_eq!(again, p);
    }
    for writer in [json!(null), json!(6), json!([]), json!({"version": "0.6.0"})] {
        let mut file = legacy_particle_file(&s, None);
        file["savedBy"] = writer;
        let mut p = effectcraft_project::Project::from_json(&file.to_string()).unwrap();
        effectcraft_engine::upgrade_effects(&mut p);
        let fx = p.comp(s.active_comp_id().unwrap()).unwrap().layer(LayerId(lid)).unwrap().effects().unwrap();
        assert!(fx.groups().all(|g| g.get("compositeWithOriginal").unwrap().value == KV::Bool(false)));
    }
}
