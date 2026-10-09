//! Names for the faces and edges of exact bodies, derived from how they were
//! made rather than from where they are. A fillet remembers "the edge between
//! the side swept from sketch line 7 and the end cap of extrude 3", so when an
//! upstream edit moves that edge the fillet follows it instead of landing on
//! whatever is nearest its old position.
//!
//! Tags are assigned when a feature creates faces and carried through later
//! operations by the kernel's face history (OpenCascade's `Modified` and
//! `Generated` relations, which cadrum exposes for booleans, fillets,
//! chamfers, shells and face unification). Semantic primitive roles and boundary
//! provenance distinguish generated/split faces without kernel ordinals. Modern
//! references never fall back to an unrelated nearest surface; legacy references
//! migrate only at a unique saved location. Ambiguous changes request a repick.

use serde::{Deserialize, Serialize};

use crate::Id;

/// How a face came to be.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A side face swept from one sketch entity by an extrude or revolve.
    Swept { feature: Id, entity: Id },
    /// An extrude's start or end cap, or a revolve's start or end face.
    Cap { feature: Id, end: bool },
    /// A cap belongs to its sketch boundary, even if profile order changes.
    ProfileCap { feature: Id, end: bool, entities: Vec<Id> },
    /// A named role in a primitive, independent of the kernel's face order.
    Semantic { feature: Id, role: String },
    /// A generated face bounded by known source faces (for example a fillet).
    Derived { feature: Id, sources: Vec<Tag> },
    /// A merged face retains all source identities, never an arbitrary first one.
    Merged { sources: Vec<Tag> },
    /// A split face distinguished by its boundary provenance, not a piece ordinal.
    Patch { of: Box<Tag>, boundary: Vec<Tag>,
        /// Oriented boundary cycles, only needed when unordered provenance ties.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cycles: Vec<Vec<Tag>>,
    },
    /// Conservative identity for topology without semantic/kernel provenance.
    /// Geometric changes invalidate it instead of reusing a kernel ordinal.
    Surface { feature: Id, signature: Vec<i64> },
    /// A side of a split plane, qualified by the source material boundary.
    Section { feature: Id, positive: bool, boundary: Vec<Tag> },
    /// Legacy 0.4 ordinal identity, read only for tightly located migration.
    Made { feature: Id, n: u32 },
    /// A piece of an earlier face that a later operation cut up, numbered among the pieces.
    Split { of: Box<Tag>, n: u32 },
    /// The same face on copy `n` of a pattern.
    Copy { of: Box<Tag>, n: u32 },
}

/// The surface a face lies on, so a tag is never matched to the wrong kind of face.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Plane,
    Cylinder,
    Cone,
    Sphere,
    Torus,
    Freeform,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Tag {
    pub origin: Origin,
    pub kind: Kind,
    /// Absent in early 0.4 files. Legacy picks must be migrated at their saved
    /// location before they can follow semantic identities through later edits.
    #[serde(default, skip_serializing_if = "is_legacy_schema")]
    pub schema: u8,
}
fn is_legacy_schema(schema: &u8) -> bool { *schema == 0 }


impl Tag {
    pub fn new(origin: Origin, kind: Kind) -> Tag {
        Tag { origin, kind, schema: 2 }
    }

    /// The tag without split ordinals: the face as it was first made.
    pub fn root(&self) -> &Tag {
        match &self.origin {
            Origin::Split { of, .. } | Origin::Copy { of, .. } | Origin::Patch { of, .. } => of.root(),
            _ => self,
        }
    }

    /// This face as piece `n` of itself after an operation cut it up.
    pub fn split(&self, n: u32) -> Tag {
        Tag::new(Origin::Split { of: Box::new(self.clone()), n }, self.kind)
    }

    /// This face on copy `n` of a pattern.
    pub fn copy(&self, n: u32) -> Tag {
        Tag::new(Origin::Copy { of: Box::new(self.clone()), n }, self.kind)
    }

    /// Split pieces may change when a face is cut. Primitive face ordinals and
    /// pattern-copy identities must remain distinct: dropping either can silently
    /// move a reference to another face after the selected one is removed.
    pub fn family(&self) -> Family {
        fn without_splits(tag: &Tag) -> Tag {
            match &tag.origin {
                Origin::Split { of, .. } | Origin::Patch { of, .. } => without_splits(of),
                Origin::Copy { of, n } => without_splits(of).copy(*n),
                _ => tag.clone(),
            }
        }
        Family(without_splits(self))
    }

    /// The feature that made the face.
    pub fn feature(&self) -> Id {
        match &self.root().origin {
            Origin::Swept { feature, .. } | Origin::Cap { feature, .. } | Origin::ProfileCap { feature, .. } | Origin::Made { feature, .. } | Origin::Semantic { feature, .. } | Origin::Derived { feature, .. } | Origin::Surface { feature, .. } | Origin::Section { feature, .. } => *feature,
            Origin::Merged { sources } => sources.first().map_or(0, Tag::feature),
            Origin::Split { .. } | Origin::Copy { .. } | Origin::Patch { .. } => unreachable!("root has no split or copy"),
        }
    }

    pub fn legacy(&self) -> bool { self.schema < 2 }

    /// Whether this current face descends from the wanted one. Generated fillet
    /// faces are not descendants of their bounding faces; merged faces are.
    pub fn descends_from(&self, wanted: &Tag) -> bool {
        self == wanted || match &self.origin {
            Origin::Merged { sources } => sources.iter().any(|s| s.descends_from(wanted)),
            Origin::Patch { of, .. } | Origin::Split {of,..} => of.descends_from(wanted),
            Origin::Copy {of,n} => matches!(&wanted.origin,Origin::Copy {of:source,n:copy} if n==copy && of.descends_from(source)),
            _ => false,
        }
    }

    /// Legacy ordinals/copy numbers cannot prove identity. Restrict migration
    /// to this making feature and surface kind, then require a unique exact
    /// saved-location hit in the resolver (never a mapped/nearest guess).
    pub fn legacy_compatible(&self, current: &Tag) -> bool {
        self.kind == current.kind && (self.feature() == current.feature()
            || matches!(&current.root().origin, Origin::Merged {sources} if sources.iter().any(|s|self.legacy_compatible(s))))
    }

    /// One line for tooltips and object info.
    pub fn describe(&self) -> String {
        let kind = match self.kind {
            Kind::Plane => "flat",
            Kind::Cylinder => "cylindrical",
            Kind::Cone => "conical",
            Kind::Sphere => "spherical",
            Kind::Torus => "toroidal",
            Kind::Freeform => "freeform",
        };
        let origin = match &self.origin {
            Origin::Swept { feature, entity } => format!("swept from sketch entity {entity} by feature {feature}"),
            Origin::Cap { feature, end: false } => format!("the start cap of feature {feature}"),
            Origin::Cap { feature, end: true } => format!("the end cap of feature {feature}"),
            Origin::Semantic { feature, role } => format!("{role} of feature {feature}"),
            Origin::Derived { feature, .. } => format!("generated from named boundaries by feature {feature}"),
            Origin::Merged { .. } => "merged source faces".into(),
            Origin::Patch { of, .. } => format!("bounded piece of {}", of.describe()),
            Origin::Surface { feature, .. } => format!("verified surface of feature {feature}"),
            Origin::Section { feature, positive, .. } => format!("{} side of split feature {feature}",if *positive {"positive"}else{"negative"}),
            Origin::ProfileCap { feature, end, .. } => format!("the {} cap of a profile in feature {feature}", if *end {"end"} else {"start"}),
            Origin::Made { feature, n } => format!("face {n} made by feature {feature}"),
            Origin::Split { of, n } => format!("piece {n} of {}", of.describe()),
            Origin::Copy { of, n } => format!("copy {n} of {}", of.describe()),
        };
        format!("{kind} face, {origin}")
    }
}

/// The making face and pattern-copy identity, with only split ordinals removed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Family(Tag);

/// An edge is where two faces meet, so it is named by both their tags in a fixed order.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeTag {
    pub faces: [Tag; 2],
}

impl EdgeTag {
    pub fn new(a: Tag, b: Tag) -> EdgeTag {
        if a <= b { EdgeTag { faces: [a, b] } } else { EdgeTag { faces: [b, a] } }
    }

    pub fn legacy(&self) -> bool { self.faces.iter().any(Tag::legacy) }
    pub fn legacy_compatible(&self, current: &EdgeTag) -> bool {
        let [a,b] = &self.faces; let [c,d] = &current.faces;
        (a.legacy_compatible(c) && b.legacy_compatible(d)) || (a.legacy_compatible(d) && b.legacy_compatible(c))
    }

    /// Whether the two edges come from the same pair of original faces, whatever later splits did.
    pub fn same_family(&self, other: &EdgeTag) -> bool {
        let [a,b]=&self.faces;let [c,d]=&other.faces;
        (a.descends_from(c) && b.descends_from(d)) || (a.descends_from(d) && b.descends_from(c))
    }

    pub fn describe(&self) -> String {
        format!("edge between {} and {}", self.faces[0].describe(), self.faces[1].describe())
    }
}

/// How a stored reference was found again on rebuild, strongest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// The very tag the reference stored.
    Tag,
    /// A uniquely identified descendant of the original face, split or merged.
    Origin,
    /// An untagged pick or a unique saved-location migration from early 0.4.
    Position,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Tag => "tag",
            Level::Origin => "origin",
            Level::Position => "position",
        }
    }
}

/// The weakest level among several picks, which is what a feature reports.
pub fn weakest(levels: impl IntoIterator<Item = Level>) -> Option<Level> {
    levels.into_iter().max()
}
