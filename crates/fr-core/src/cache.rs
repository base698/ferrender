//! The geometry cache: the built bodies saved beside the design, so a file
//! opens without a rebuild, `ferrender check` can read many files quickly,
//! and an older application can still show a design that uses a newer feature.
//!
//! Editable sessions adopt a cache only after the entire native container is
//! authenticated with this installation's private key. CRC, algorithm revision,
//! volume and bounds are additional consistency checks, not authentication.
//! Foreign/unsigned compatible files rebuild. Unsupported newer timelines can
//! only show an explicitly unverified read-only preview of their saved geometry.

use std::collections::{BTreeMap, HashMap};

use glam::{DAffine3, DVec3};
use serde::{Deserialize, Serialize};

use crate::doc::{Body, Built, Document};
use crate::exact::{self, Tags};
use crate::mesh::Mesh;
use crate::planes::ResolvedPlane;
use crate::sketch::{Id, Plane};
use crate::tag::Level;

/// What produced the cached shapes. A different kernel may triangulate or
/// even model differently, so its caches are not used.
pub const KERNEL: &str = "cadrum 0.8.20";
/// Bump whenever Ferrender changes feature evaluation or mesh geometry. Kernel
/// version alone cannot invalidate results from an older application algorithm.
/// Revision 0 (a missing field) denotes the original 0.4 development caches.
pub const GEOMETRY_REVISION: u32 = 2;
/// A cache larger than this is not written; the design rebuilds instead.
pub const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
/// Designs that rebuild faster than this are not worth caching unless the file is a container anyway.
pub const WORTH_CACHING_MS: u32 = 250;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BodyEntry {
    pub id: Id,
    pub name: String,
    pub component: Id,
    pub placement: DAffine3,
    pub local_bounds: Option<[DVec3; 2]>,
    /// The exact volume (or the mesh's), checked after the shape is read back.
    pub volume: f64,
    pub bounds: [DVec3; 2],
    pub triangles: usize,
    #[serde(default)]
    pub tags: Tags,
    /// `cache/<id>.brep` for an exact body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brep: Option<String>,
    /// `cache/<id>.mesh` for a mesh body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaneEntry {
    pub id: Id,
    pub component: Id,
    pub plane: Plane,
    pub corners: [DVec3; 4],
}

/// `cache/index.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Index {
    /// CRC-32 of the plain JSON of the design the cache was made from.
    pub design_crc: u32,
    pub kernel: String,
    #[serde(default)]
    pub geometry_revision: u32,
    pub bodies: Vec<BodyEntry>,
    #[serde(default)]
    pub planes: Vec<PlaneEntry>,
    #[serde(default)]
    pub errors: BTreeMap<Id, String>,
    #[serde(default)]
    pub resolutions: BTreeMap<Id, Level>,
}

/// A cache as read from or written to a container.
#[derive(Clone, Debug)]
pub struct Cache {
    pub index: Index,
    /// Entry name to bytes: BRep text for exact bodies, the mesh blob for mesh bodies.
    pub blobs: HashMap<String, Vec<u8>>,
    /// Set only for freshly evaluated geometry or a locally authenticated file.
    /// Untrusted previews can be restored for display, never adopted or signed.
    pub(crate) trusted: bool,
}

/// What a cache restores, ready to stand in for a rebuild's results.
pub struct Restored {
    pub bodies: Vec<Body>,
    pub planes: BTreeMap<Id, ResolvedPlane>,
    pub errors: BTreeMap<Id, String>,
    pub resolutions: BTreeMap<Id, Level>,
}

/// The design's identity for the cache: the CRC of its plain JSON form.
pub fn design_crc(doc: &Document) -> u32 {
    crc32fast::hash(crate::io::to_json(doc).as_bytes())
}

fn body_volume(b: &Body) -> f64 {
    if b.is_exact() { b.solids.iter().map(|s| s.volume()).sum() } else { b.mesh.volume() }
}

impl Cache {
    /// Captures the built geometry. `None` when something cannot be cached:
    /// a body with modeled threads (their meshes are not stored) or a cache over the size limit.
    pub fn capture(doc: &Document, built: &Built) -> Result<Option<Cache>, String> {
        if built.bodies.iter().any(|b| !b.threads.is_empty()) {
            return Ok(None);
        }
        let mut blobs = HashMap::new();
        let mut total = 0usize;
        let mut bodies = Vec::new();
        for b in &built.bodies {
            let Some((lo, hi)) = b.mesh.bbox() else { continue };
            let (mut brep, mut mesh) = (None, None);
            if b.is_exact() {
                let mut out = Vec::new();
                cadrum::Solid::write_brep(b.solids.iter(), &mut out).map_err(|e| format!("the kernel could not write the cache: {e}"))?;
                total += out.len();
                let name = format!("cache/{}.brep", b.id);
                blobs.insert(name.clone(), out);
                brep = Some(name);
            } else {
                let out = crate::meshfile::encode_blob(&b.mesh);
                total += out.len();
                let name = format!("cache/{}.mesh", b.id);
                blobs.insert(name.clone(), out);
                mesh = Some(name);
            }
            if total > MAX_CACHE_BYTES {
                return Ok(None);
            }
            bodies.push(BodyEntry { id: b.id, name: b.name.clone(), component: b.component, placement: b.placement, local_bounds: b.local_bounds, volume: body_volume(b), bounds: [lo, hi], triangles: b.mesh.len(), tags: b.tags.clone(), brep, mesh });
        }
        let planes = built.planes.iter().map(|(id, p)| PlaneEntry { id: *id, component: p.component, plane: p.plane, corners: p.corners }).collect();
        let index = Index { design_crc: design_crc(doc), kernel: KERNEL.into(), geometry_revision: GEOMETRY_REVISION, bodies, planes, errors: built.errors.clone(), resolutions: built.resolutions.clone() };
        Ok(Some(Cache { index, blobs, trusted: true }))
    }

    /// Whether this cache was made from exactly this design by this kernel and
    /// this revision of Ferrender's geometry algorithms.
    pub fn matches(&self, doc: &Document) -> bool {
        self.trusted && self.index.kernel == KERNEL && self.index.geometry_revision == GEOMETRY_REVISION && self.index.design_crc == design_crc(doc)
    }

    /// Reads the shapes back and checks them against the index. An error means
    /// the cache is not to be used; the caller rebuilds instead.
    pub fn restore(&self) -> Result<Restored, String> {
        if self.index.kernel != KERNEL {
            return Err(format!("the cache was made by {}, not {KERNEL}", self.index.kernel));
        }
        let mut bodies = Vec::with_capacity(self.index.bodies.len());
        for e in &self.index.bodies {
            let mut body = Body { id: e.id, name: e.name.clone(), component: e.component, placement: e.placement, local_bounds: e.local_bounds, mesh: Mesh::default(), solids: Vec::new(), tags: Vec::new(), edges: Vec::new(), threads: Vec::new(), plain: 0 };
            match (&e.brep, &e.mesh) {
                (Some(name), _) => {
                    let bytes = self.blobs.get(name).ok_or_else(|| format!("the cache has no {name}"))?;
                    let solids = cadrum::Solid::read_brep(&mut &bytes[..]).map_err(|err| format!("the cached shape {name} could not be read: {err}"))?;
                    if solids.is_empty() { return Err(format!("the cached shape {name} is empty")); }
                    let (mesh, edges) = exact::tessellate(&solids)?;
                    let tags = if e.tags.len() == solids.len() && e.tags.iter().zip(&solids).all(|(t, s)| t.len() == s.iter_face().count()) { e.tags.clone() } else { exact::no_tags(&solids) };
                    body.mesh = mesh;
                    body.edges = edges;
                    body.solids = solids;
                    body.tags = tags;
                }
                (None, Some(name)) => {
                    let bytes = self.blobs.get(name).ok_or_else(|| format!("the cache has no {name}"))?;
                    body.mesh = crate::meshfile::decode_blob(bytes).map_err(|err| format!("the cached mesh {name} could not be read: {err}"))?;
                }
                (None, None) => return Err(format!("the cache names no shape for body {}", e.id)),
            }
            body.plain = body.mesh.len();
            // The shape that came back must be the shape that was saved.
            let (lo, hi) = body.mesh.bbox().ok_or("a cached body has no triangles")?;
            let volume = body_volume(&body);
            let scale = (e.bounds[1] - e.bounds[0]).max_element().max(1.0);
            if (volume - e.volume).abs() > 1e-6 * e.volume.abs().max(1.0) || lo.distance(e.bounds[0]) > 1e-4 * scale || hi.distance(e.bounds[1]) > 1e-4 * scale {
                return Err(format!("cached body {} does not match its index (volume {volume} vs {}, bounds {lo} {hi} vs {:?})", e.id, e.volume, e.bounds));
            }
            if body.mesh.len() != e.triangles && body.is_exact() {
                // Tessellation counts can drift with the kernel; volume and bounds already agreed. Keep it.
            }
            bodies.push(body);
        }
        let planes = self.index.planes.iter().map(|p| (p.id, ResolvedPlane { component: p.component, plane: p.plane, corners: p.corners })).collect();
        Ok(Restored { bodies, planes, errors: self.index.errors.clone(), resolutions: self.index.resolutions.clone() })
    }

    /// The bytes of every entry, index first, as the container writes them.
    pub fn entries(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = vec![("cache/index.json".to_owned(), serde_json::to_vec_pretty(&self.index).expect("the index serializes"))];
        let mut names: Vec<&String> = self.blobs.keys().collect();
        names.sort();
        out.extend(names.into_iter().map(|n| (n.clone(), self.blobs[n].clone())));
        out
    }

    pub fn body_count(&self) -> usize {
        self.index.bodies.len()
    }
}
