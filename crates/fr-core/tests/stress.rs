//! The large-mesh stress test. It runs against real scans when
//! `FERRENDER_STRESS_DIR` names a folder of `.stl` files (the development
//! machine keeps three, of 1, 2 and 3.5 million triangles), and otherwise
//! against a synthetic million-triangle surface, so CI exercises the same
//! code with generous ceilings. Exact timings are printed for the record.
//! Run it in release: `cargo test -p fr-core --release --test stress -- --nocapture`.

use std::path::PathBuf;
use std::time::Instant;

use fr_core::api::execute;
use fr_core::{Session, io};
use glam::DVec3;
use serde_json::json;

/// A closed, bumpy surface of about `n` triangles: a sphere of latitude-longitude quads with noise.
fn synthetic(n: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ferrender-stress-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("synthetic.stl");
    let rows = ((n / 2) as f64).sqrt() as usize;
    let cols = rows;
    let at = |i: usize, j: usize| {
        if i == 0 { return DVec3::new(0.0, 0.0, 50.0); }
        if i == rows { return DVec3::new(0.0, 0.0, -50.0); }
        let (u, v) = (i as f64 / rows as f64 * std::f64::consts::PI, j as f64 / cols as f64 * std::f64::consts::TAU);
        let r = 50.0 + 2.0 * ((u * 40.0).sin() * (v * 30.0).cos());
        DVec3::new(r * u.sin() * v.cos(), r * u.sin() * v.sin(), r * u.cos())
    };
    let mut bytes = vec![b' '; 80];
    let mut count = 0u32;
    bytes.extend([0u8; 4]);
    for i in 0..rows {
        for j in 0..cols {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, (j + 1) % cols), at(i, (j + 1) % cols));
            for t in [[a, b, c], [a, c, d]] {
                bytes.extend([0u8; 12]);
                for v in t { for x in v.to_array() { bytes.extend((x as f32).to_le_bytes()); } }
                bytes.extend([0u8; 2]);
                count += 1;
            }
        }
    }
    bytes[80..84].copy_from_slice(&count.to_le_bytes());
    std::fs::write(&path, bytes).unwrap();
    path
}

fn files() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("FERRENDER_STRESS_DIR") {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("stl"))).collect();
        v.sort();
        v
    } else {
        vec![synthetic(1_000_000)]
    }
}

/// Ceilings per million triangles, generous enough for CI machines; the real targets are in docs/0.4-release.md.
const IMPORT_S_PER_M: f64 = 6.0;
const PICK_MS_PER_M: f64 = 60.0;
const SAVE_OPEN_S_PER_M: f64 = 8.0;

#[test]
fn large_meshes_import_pick_and_round_trip_within_budget() {
    for path in files() {
        let t = Instant::now();
        let (mesh, report) = io::import_mesh(&path, fr_core::Unit::Mm).unwrap();
        let import = t.elapsed().as_secs_f64();
        let m = mesh.len() as f64 / 1e6;
        println!("{}: {} ({:.2} s import)", path.file_name().unwrap().to_string_lossy(), report.summary(), import);
        assert!(mesh.is_welded());
        assert!(mesh.vertex_count() < mesh.len(), "a scan's vertices are shared");
        assert!(import < IMPORT_S_PER_M * m.max(0.5), "import took {import:.2} s for {m:.1} M triangles");

        let (lo, hi) = mesh.bbox().unwrap();
        let centre = (lo + hi) / 2.0;
        let t = Instant::now();
        let _ = mesh.bvh();
        let bvh_build = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let hit = mesh.ray(centre + DVec3::new(0.0, 0.0, (hi.z - lo.z) * 2.0), -DVec3::Z);
        let ray = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let near = mesh.nearest_tri(centre + DVec3::new((hi.x - lo.x) * 0.3, 0.0, 0.0));
        let nearest = t.elapsed().as_secs_f64() * 1000.0;
        println!("  bvh {:.2} s ({} nodes), ray {ray:.2} ms, nearest {nearest:.2} ms", bvh_build, mesh.bvh().node_count());
        assert!(hit.is_some() && near.is_some());
        assert!(ray < PICK_MS_PER_M * m.max(0.5) && nearest < PICK_MS_PER_M * m.max(0.5) * 4.0, "pick took {ray:.1} / {nearest:.1} ms");

        let t = Instant::now();
        let adjacency = mesh.adjacency();
        let face = mesh.face(hit.unwrap().1);
        println!("  adjacency {:.2} s, face spread {} triangles in {:.2} s total", t.elapsed().as_secs_f64(), face.len(), t.elapsed().as_secs_f64());
        assert_eq!(adjacency.vertex_count(), mesh.vertex_count());

        let t = Instant::now();
        let coarse = mesh.clustered_to(200_000);
        println!("  clustered to {} triangles in {:.2} s", coarse.len(), t.elapsed().as_secs_f64());
        assert!(coarse.len() < 400_000 && coarse.len() > 50_000, "clustering aimed at 200 k triangles and made {}", coarse.len());

        // A boolean with a small tool: only the triangles near the tool are split.
        let tool = {
            let rows = 60;
            let r = (hi - lo).max_element() * 0.08;
            let at = |i: usize, j: usize| {
                if i == 0 { return centre + DVec3::new(0.0, 0.0, r); }
                if i == rows { return centre + DVec3::new(0.0, 0.0, -r); }
                let (u, v) = (i as f64 / rows as f64 * std::f64::consts::PI, j as f64 / rows as f64 * std::f64::consts::TAU);
                centre + DVec3::new(r * u.sin() * v.cos(), r * u.sin() * v.sin(), r * u.cos())
            };
            let mut tris = Vec::new();
            for i in 0..rows { for j in 0..rows {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, (j + 1) % rows), at(i, (j + 1) % rows));
                tris.push([a, b, c]); tris.push([a, c, d]);
            } }
            let mut m = fr_core::mesh::Mesh::welded_from_tris(&tris);
            m.repair();
            // Put it on the surface so it actually cuts: slide it along +Z until it meets the scan's top.
            let top = mesh.ray(centre + DVec3::new(0.0, 0.0, (hi.z - lo.z) * 2.0), -DVec3::Z).map(|(d, _)| centre.z + (hi.z - lo.z) * 2.0 - d).unwrap_or(hi.z);
            m.map(|p| p + DVec3::new(0.0, 0.0, top - centre.z));
            m
        };
        let t = Instant::now();
        let cut = fr_core::meshops::boolean(&mesh, &tool, fr_core::csg::Bool::Subtract).unwrap();
        let bool_s = t.elapsed().as_secs_f64();
        println!("  cut with a {}-triangle tool in {bool_s:.2} s: {} triangles, {} open edges", tool.len(), cut.len(), cut.open_edges());
        assert!(cut.len() > mesh.len() / 2);
        assert!(bool_s < 10.0 * m.max(0.5), "the culled boolean took {bool_s:.1} s");

        let mut s = Session::default();
        execute(&mut s, &json!({"op": "import_stl", "path": path.display().to_string(), "units": "mm"}), None).unwrap();
        let out = std::env::temp_dir().join(format!("ferrender-stress-{}-{}.ferr", std::process::id(), path.file_stem().unwrap().to_string_lossy()));
        let t = Instant::now();
        let saved = s.save(&out).unwrap();
        let save = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let again = Session::open(&out).unwrap();
        let open = t.elapsed().as_secs_f64();
        let size = std::fs::metadata(&out).unwrap().len();
        println!("  save {save:.2} s, open {open:.2} s, {} MB on disk, container {}", size / 1_000_000, saved.container);
        assert!(saved.container);
        assert_eq!(again.built.bodies[0].mesh.len(), mesh.len());
        assert!(save + open < SAVE_OPEN_S_PER_M * m.max(0.5), "save and open took {:.2} s", save + open);
        let _ = std::fs::remove_file(&out);
    }
}
