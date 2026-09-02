//! What a labeller is allowed to draw, and what each drawing has to say.
//!
//! Lifted from CVAT's model rather than Roboflow's: a class carries typed
//! attributes and declares its geometry, because a door that cannot say which
//! way it swings is a door the downstream JSON cannot describe. A flat list of
//! class names would make `door` and `door_swing_in` two classes, which is how
//! a label set grows to forty names that no two labellers apply the same way.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::{Error, Result, digest, validate_slug};

/// The shape a class is drawn as.
///
/// One kind per class, checked on save. A class drawn as a polygon in one
/// image and a box in another produces a training target nothing can decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GeometryKind {
    /// A single position: a column, a north marker, a text anchor.
    Point,
    /// An axis-aligned rectangle. The cheapest useful label, and the one that
    /// says least — a bounding box around a wall is the whole room.
    ///
    /// Renamed explicitly: `rename_all = "snake_case"` turns `BBox` into
    /// `b_box`, which is not a word anybody would type.
    #[serde(rename = "bbox")]
    BBox,
    /// An open chain. The wall centreline, and the dimension line whose two
    /// ends fix the scale.
    Polyline,
    /// A closed ring with optional holes. A room, a terrace, an ignore region.
    Polygon,
    /// Named positions that belong to one instance: a door's opening ends, its
    /// hinge and the end of its leaf.
    ///
    /// This is the kind that makes the difference between a mask and a usable
    /// annotation, so it is the one worth naming rather than encoding as four
    /// separate point classes with a grouping convention.
    Keypoints,
}

impl GeometryKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::BBox => "bbox",
            Self::Polyline => "polyline",
            Self::Polygon => "polygon",
            Self::Keypoints => "keypoints",
        }
    }
}

/// The type of one attribute a class carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttributeKind {
    /// One of `values`. The only kind whose vocabulary is closed, which is why
    /// it is the right kind for `swing`, `door_type` and `wall_role`.
    Enum,
    Bool,
    Number,
    Text,
}

/// One typed field on a class.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct AttributeDef {
    pub name: String,
    pub kind: AttributeKind,
    /// The closed vocabulary for [`AttributeKind::Enum`]. Ignored otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// A required attribute is refused when missing, which is what stops a
    /// half-labelled door from reaching a training export.
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// One drawable class.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LabelClass {
    /// Stable machine name — `wall_exterior`, not `Exterior wall`. Renaming it
    /// is a new schema version, never an edit.
    pub name: String,
    pub geometry: GeometryKind,
    /// What the canvas draws it in. Presentation only; nothing downstream may
    /// depend on it.
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<AttributeDef>,
    /// The named positions a [`GeometryKind::Keypoints`] class expects, in the
    /// order the tool asks for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keypoints: Vec<String>,
    /// Keypoints that may be left out because the plan does not show them.
    /// Everything else in `keypoints` is required.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub optional_keypoints: Vec<String>,
    /// Named references this class may hold to other instances in the same
    /// image: `wall` for an opening, `connects` for a door's two rooms.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkDef>,
    /// Excluded from every training target and from the loss.
    ///
    /// The furniture, the hatching and the title block are not background —
    /// they are pixels a model must not be scored on either way. Marking them
    /// is cheaper than labelling them and far cheaper than the false positives
    /// they produce.
    #[serde(default)]
    pub ignore: bool,
    /// Which grid this class paints into when a revision is rasterised.
    ///
    /// The generic form of a problem that does not look generic: some classes
    /// *overlay* others and must not erase them. An opening in a wall, a
    /// defect on a component, a marking on a road — in every case the thing
    /// underneath is still there, and one grid could only represent the
    /// overlay by deleting it.
    ///
    /// So the schema says so. Classes on the same layer share one integer grid
    /// and paint in declaration order, last wins; classes on different layers
    /// never contend, and a model reads one head per layer. Most vocabularies
    /// need exactly one layer and never set this.
    #[serde(default)]
    pub layer: u8,
}

/// One named reference from an instance to another instance.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LinkDef {
    pub name: String,
    /// Classes the target may belong to. Empty means any class.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<String>,
    /// How many targets this link must have. `(1, 1)` for an opening's wall,
    /// `(2, 2)` for the two spaces it connects.
    #[serde(default)]
    pub min: usize,
    #[serde(default = "default_max_links")]
    pub max: usize,
}

/// The complete, versioned vocabulary of one project.
///
/// Content-addressed like everything else authored here: the version is the
/// digest of the classes, so an unchanged schema re-saved is the same version
/// and a renamed class is unmistakably a different one.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LabelSchema {
    pub version: String,
    pub classes: Vec<LabelClass>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

impl LabelSchema {
    /// Validate a proposed class list and stamp it with its content digest.
    pub fn build(classes: Vec<LabelClass>, created_at: OffsetDateTime) -> Result<Self> {
        if classes.is_empty() {
            return Err(Error::Invalid(
                "a label schema needs at least one class".to_owned(),
            ));
        }
        let mut seen = BTreeMap::new();
        for class in &classes {
            validate_slug(&class.name, "a class")?;
            if seen.insert(class.name.clone(), ()).is_some() {
                return Err(Error::Invalid(format!(
                    "the class {} is declared twice",
                    class.name
                )));
            }
            validate_class(class)?;
        }
        for class in &classes {
            for link in &class.links {
                for target in &link.targets {
                    if !seen.contains_key(target) {
                        return Err(Error::Invalid(format!(
                            "{}.{} points at the class {target}, which the schema does not declare",
                            class.name, link.name
                        )));
                    }
                }
            }
        }

        let identity = serde_json::to_vec(&classes).map_err(|error| {
            Error::Invalid(format!("the label schema could not be encoded: {error}"))
        })?;
        Ok(Self {
            version: digest(&identity),
            classes,
            created_at,
        })
    }

    #[must_use]
    pub fn class(&self, name: &str) -> Option<&LabelClass> {
        self.classes.iter().find(|class| class.name == name)
    }

    /// Class names in schema order, which is the order a training export
    /// assigns category ids in.
    #[must_use]
    pub fn class_names(&self) -> Vec<String> {
        self.classes
            .iter()
            .map(|class| class.name.clone())
            .collect()
    }
}

fn validate_class(class: &LabelClass) -> Result<()> {
    if class.geometry == GeometryKind::Keypoints && class.keypoints.is_empty() {
        return Err(Error::Invalid(format!(
            "the keypoint class {} declares no keypoints",
            class.name
        )));
    }
    if class.geometry != GeometryKind::Keypoints && !class.keypoints.is_empty() {
        return Err(Error::Invalid(format!(
            "{} declares keypoints but is drawn as a {}",
            class.name,
            class.geometry.as_str()
        )));
    }
    for keypoint in &class.keypoints {
        validate_slug(keypoint, "a keypoint")?;
    }
    for optional in &class.optional_keypoints {
        if !class.keypoints.iter().any(|name| name == optional) {
            return Err(Error::Invalid(format!(
                "{} marks the keypoint {optional} optional without declaring it",
                class.name
            )));
        }
    }
    for attribute in &class.attributes {
        validate_slug(&attribute.name, "an attribute")?;
        if attribute.kind == AttributeKind::Enum && attribute.values.is_empty() {
            return Err(Error::Invalid(format!(
                "the enum attribute {}.{} declares no values",
                class.name, attribute.name
            )));
        }
        if attribute.kind != AttributeKind::Enum && !attribute.values.is_empty() {
            return Err(Error::Invalid(format!(
                "{}.{} lists values but is a {:?} attribute",
                class.name, attribute.name, attribute.kind
            )));
        }
    }
    for link in &class.links {
        validate_slug(&link.name, "a link")?;
        if link.max < link.min || link.max == 0 {
            return Err(Error::Invalid(format!(
                "{}.{} allows between {} and {} targets",
                class.name, link.name, link.min, link.max
            )));
        }
    }
    Ok(())
}

fn default_color() -> String {
    "#7c8794".to_owned()
}

const fn default_max_links() -> usize {
    8
}
