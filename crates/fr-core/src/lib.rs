//! Ferrender's engine: parametric sketches, a constraint solver, mesh
//! solids and the command API that the app, the MCP server and the AI
//! assistant all drive. No GUI dependencies.

pub mod api;
pub mod body_ops;
pub mod cache;
mod cache_auth;
pub mod csg;
pub mod components;
pub mod doc;
pub mod exact;
pub mod expr;
pub mod face;
pub mod io;
pub mod measure;
pub mod mesh;
pub mod meshfile;
pub mod meshops;
pub mod ops;
pub mod profile;
pub mod primitives;
pub mod planes;
pub mod reference;
pub mod render;
pub mod sandboxfs;
pub mod script;
pub mod sketch;
pub mod solver;
pub mod tag;
pub mod threads;
pub mod text;
pub mod units;
pub mod validation;

pub use doc::{Axis, Body, Built, Document, Feature, FeatureKind, Op, Session};
pub use expr::{Kind, Value};
pub use sketch::{CKind, Constraint, Entity, Geom, Id, ORIGIN, Plane, Sketch};
pub use units::Unit;

/// How the linked OpenCascade libraries were obtained, recorded by build.rs:
/// `verified <release> sha256:<archive digest>` or `unverified`.
pub const OCCT_PROVENANCE: &str = env!("FR_CORE_OCCT");
