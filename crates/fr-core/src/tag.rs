//! Names for the faces and edges of exact bodies, derived from how they were
//! made rather than from where they are. A fillet remembers "the edge between
//! the side swept from sketch line 7 and the end cap of extrude 3", so when an
//! upstream edit moves that edge the fillet follows it instead of landing on
//! whatever is nearest its old position.
//!
//! Tags are assigned when a feature creates faces and carried through later
//! operations by the kernel's face history (OpenCascade's `Modified` and
//! `Generated` relations, which cadrum exposes for booleans, fillets,
//! chamfers, shells and face unification) or, where that is not available, by
//! matching surfaces. References store the tag beside the point they always
//! stored, and resolve by tag first, by tag family second and by position last.

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
    /// Made by a fillet, chamfer, shell, hole, thread, primitive, split or text: the feature and the face's ordinal within it.
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
}

impl Tag {
    pub fn new(origin: Origin, kind: Kind) -> Tag {
        Tag { origin, kind }
    }

    /// The tag without split ordinals: the face as it was first made.
    pub fn root(&self) -> &Tag {
        match &self.origin {
            Origin::Split { of, .. } | Origin::Copy { of, .. } => of.root(),
            _ => self,
        }
    }

    /// This face as piece `n` of itself after an operation cut it up.
    pub fn split(&self, n: u32) -> Tag {
        Tag { kind: self.kind, origin: Origin::Split { of: Box::new(self.clone()), n } }
    }

    /// This face on copy `n` of a pattern.
    pub fn copy(&self, n: u32) -> Tag {
        Tag { kind: self.kind, origin: Origin::Copy { of: Box::new(self.clone()), n } }
    }

    /// Split pieces may change when a face is cut. Primitive face ordinals and
    /// pattern-copy identities must remain distinct: dropping either can silently
    /// move a reference to another face after the selected one is removed.
    pub fn family(&self) -> Family {
        fn without_splits(tag: &Tag) -> Tag {
            match &tag.origin {
                Origin::Split { of, .. } => without_splits(of),
                Origin::Copy { of, n } => without_splits(of).copy(*n),
                _ => tag.clone(),
            }
        }
        Family(without_splits(self))
    }

    /// The feature that made the face.
    pub fn feature(&self) -> Id {
        match &self.root().origin {
            Origin::Swept { feature, .. } | Origin::Cap { feature, .. } | Origin::Made { feature, .. } => *feature,
            Origin::Split { .. } | Origin::Copy { .. } => unreachable!("root has no split or copy"),
        }
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

    /// Whether the two edges come from the same pair of original faces, whatever later splits did.
    pub fn same_family(&self, other: &EdgeTag) -> bool {
        let (a, b) = (self.faces[0].family(), self.faces[1].family());
        let (c, d) = (other.faces[0].family(), other.faces[1].family());
        (a == c && b == d) || (a == d && b == c)
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
    /// A face of the same origin, split differently; the nearest one to the stored point.
    Origin,
    /// Nothing by tag; the face or edge nearest the stored point, as before 0.4.
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
