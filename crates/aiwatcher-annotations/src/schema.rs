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

/// The label schema this stack was built for: a residential floor plan.
///
/// Shipped as a starting point rather than as the only option, because the
/// first hour of an annotation project is otherwise spent inventing a
/// vocabulary — and the vocabulary somebody invents in that hour is usually the
/// one without a hinge point, which is the one that has to be re-drawn.
///
/// Three choices in here are the ones worth arguing with:
///
/// * a **wall is a polyline plus a thickness**, not a filled rectangle, because
///   the centreline is what an editor drags and what a 3D extrusion needs, and
///   the rectangle is recoverable from it;
/// * a **door is a keypoint instance**, so the opening's two ends, the hinge
///   and the leaf's end are four named positions on one thing rather than four
///   classes and a grouping convention;
/// * **space and functional zone are separate**, so a kitchen inside an open
///   living room is one enclosure with two zones and nothing has to invent a
///   wall that is not there.
#[must_use]
pub fn floor_plan_classes() -> Vec<LabelClass> {
    let wall_link = LinkDef {
        name: "wall".to_owned(),
        targets: vec!["wall".to_owned()],
        min: 0,
        max: 1,
    };
    let connects = LinkDef {
        name: "connects".to_owned(),
        targets: vec!["space".to_owned()],
        min: 0,
        max: 2,
    };
    let opening_attributes = vec![AttributeDef {
        name: "width_cm".to_owned(),
        kind: AttributeKind::Number,
        values: Vec::new(),
        required: false,
        default: None,
        description: "Filled in once the scale is known; left out while labelling.".to_owned(),
    }];

    vec![
        LabelClass {
            name: "wall".to_owned(),
            geometry: GeometryKind::Polyline,
            color: "#1f2937".to_owned(),
            description: "The centreline of a wall run, with its drawn thickness.".to_owned(),
            attributes: vec![
                AttributeDef {
                    name: "role".to_owned(),
                    kind: AttributeKind::Enum,
                    values: vec![
                        "exterior".to_owned(),
                        "interior".to_owned(),
                        "unknown".to_owned(),
                    ],
                    required: true,
                    default: Some(Value::String("unknown".to_owned())),
                    description: "Exterior or interior. Load-bearing is not in a marketing plan and is deliberately not a value here.".to_owned(),
                },
                AttributeDef {
                    name: "thickness_px".to_owned(),
                    kind: AttributeKind::Number,
                    values: Vec::new(),
                    required: true,
                    default: None,
                    description: "Drawn thickness in image pixels. With the scale, this becomes millimetres.".to_owned(),
                },
            ],
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: Vec::new(),
            ignore: false,
        },
        LabelClass {
            name: "space".to_owned(),
            geometry: GeometryKind::Polygon,
            color: "#2563eb".to_owned(),
            description: "One physically enclosed room, as drawn.".to_owned(),
            attributes: vec![
                AttributeDef {
                    name: "room_id".to_owned(),
                    kind: AttributeKind::Text,
                    values: Vec::new(),
                    required: false,
                    default: None,
                    description: "The number printed on the plan, e.g. 1.6.".to_owned(),
                },
                AttributeDef {
                    name: "printed_area_m2".to_owned(),
                    kind: AttributeKind::Number,
                    values: Vec::new(),
                    required: false,
                    default: None,
                    description: "The area printed on the plan. The cross-check the scale is validated against, never the source of the scale.".to_owned(),
                },
            ],
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: Vec::new(),
            ignore: false,
        },
        LabelClass {
            name: "functional_zone".to_owned(),
            geometry: GeometryKind::Polygon,
            color: "#38bdf8".to_owned(),
            description: "A named use inside a space: the kitchen half of an open living room."
                .to_owned(),
            attributes: vec![AttributeDef {
                name: "use".to_owned(),
                kind: AttributeKind::Enum,
                values: [
                    "living", "kitchen", "dining", "bedroom", "bathroom", "wc", "hall",
                    "utility", "garage", "terrace", "balcony", "stairwell", "storage", "other",
                ]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
                required: true,
                default: None,
                description: String::new(),
            }],
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: vec![LinkDef {
                name: "space".to_owned(),
                targets: vec!["space".to_owned()],
                min: 0,
                max: 1,
            }],
            ignore: false,
        },
        LabelClass {
            name: "door".to_owned(),
            geometry: GeometryKind::Keypoints,
            color: "#f97316".to_owned(),
            description: "An opening with a leaf: the two ends of the opening, the hinge, and where the leaf ends when open.".to_owned(),
            attributes: vec![
                AttributeDef {
                    name: "door_type".to_owned(),
                    kind: AttributeKind::Enum,
                    values: ["hinged", "double", "sliding", "folding", "unknown"]
                        .into_iter()
                        .map(ToOwned::to_owned)
                        .collect(),
                    required: true,
                    default: Some(Value::String("hinged".to_owned())),
                    description: String::new(),
                },
                AttributeDef {
                    name: "exterior".to_owned(),
                    kind: AttributeKind::Bool,
                    values: Vec::new(),
                    required: false,
                    default: None,
                    description: "True for a front or terrace door.".to_owned(),
                },
            ],
            keypoints: ["opening_start", "opening_end", "hinge", "leaf_end"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
            // A sliding door has no hinge and no swept leaf, and a plan that
            // draws neither should not force a labeller to invent them.
            optional_keypoints: vec!["hinge".to_owned(), "leaf_end".to_owned()],
            links: vec![wall_link.clone(), connects.clone()],
            ignore: false,
        },
        LabelClass {
            name: "window".to_owned(),
            geometry: GeometryKind::Keypoints,
            color: "#22c55e".to_owned(),
            description: "The two ends of a window opening in a wall.".to_owned(),
            attributes: {
                let mut attributes = opening_attributes.clone();
                attributes.push(AttributeDef {
                    name: "window_type".to_owned(),
                    kind: AttributeKind::Enum,
                    values: ["window", "roof_window", "balcony_door", "unknown"]
                        .into_iter()
                        .map(ToOwned::to_owned)
                        .collect(),
                    required: true,
                    default: Some(Value::String("window".to_owned())),
                    description: "A balcony door is drawn like both and is its own class value for exactly that reason.".to_owned(),
                });
                attributes
            },
            keypoints: vec!["opening_start".to_owned(), "opening_end".to_owned()],
            optional_keypoints: Vec::new(),
            links: vec![wall_link.clone()],
            ignore: false,
        },
        LabelClass {
            name: "passage".to_owned(),
            geometry: GeometryKind::Keypoints,
            color: "#a855f7".to_owned(),
            description: "An opening with no leaf. The thing a mask cannot distinguish from a gap in a wall.".to_owned(),
            attributes: opening_attributes,
            keypoints: vec!["opening_start".to_owned(), "opening_end".to_owned()],
            optional_keypoints: Vec::new(),
            links: vec![wall_link, connects],
            ignore: false,
        },
        LabelClass {
            name: "stairs".to_owned(),
            geometry: GeometryKind::Polygon,
            color: "#eab308".to_owned(),
            description: "The stair's footprint. The direction keypoints go on a separate `stair_direction` instance.".to_owned(),
            attributes: vec![AttributeDef {
                name: "direction".to_owned(),
                kind: AttributeKind::Enum,
                values: ["up", "down", "both", "unknown"]
                    .into_iter()
                    .map(ToOwned::to_owned)
                    .collect(),
                required: false,
                default: Some(Value::String("unknown".to_owned())),
                description: "Which way the arrow points on this floor.".to_owned(),
            }],
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: Vec::new(),
            ignore: false,
        },
        LabelClass {
            name: "column".to_owned(),
            geometry: GeometryKind::Polygon,
            color: "#64748b".to_owned(),
            description: "A pillar. Often outside the building outline, which is why it is not a wall.".to_owned(),
            attributes: Vec::new(),
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: Vec::new(),
            ignore: false,
        },
        LabelClass {
            name: "dimension".to_owned(),
            geometry: GeometryKind::Polyline,
            color: "#ec4899".to_owned(),
            description: "One dimension line, drawn end to end, carrying the value printed against it. Two or three of these fix the scale.".to_owned(),
            attributes: vec![
                AttributeDef {
                    name: "value".to_owned(),
                    kind: AttributeKind::Number,
                    values: Vec::new(),
                    required: true,
                    default: None,
                    description: "The printed number.".to_owned(),
                },
                AttributeDef {
                    name: "unit".to_owned(),
                    kind: AttributeKind::Enum,
                    values: ["mm", "cm", "m"].into_iter().map(ToOwned::to_owned).collect(),
                    required: true,
                    default: Some(Value::String("cm".to_owned())),
                    description: String::new(),
                },
                AttributeDef {
                    name: "measures".to_owned(),
                    kind: AttributeKind::Enum,
                    values: ["building", "room", "opening", "terrace", "unknown"]
                        .into_iter()
                        .map(ToOwned::to_owned)
                        .collect(),
                    required: false,
                    default: Some(Value::String("unknown".to_owned())),
                    description: "A terrace dimension calibrated as a building one is the classic way to get a scale that is 8% wrong.".to_owned(),
                },
            ],
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: Vec::new(),
            ignore: false,
        },
        LabelClass {
            name: "text".to_owned(),
            geometry: GeometryKind::BBox,
            color: "#94a3b8".to_owned(),
            description: "A run of text, with what it says and what it is for.".to_owned(),
            attributes: vec![AttributeDef {
                name: "role".to_owned(),
                kind: AttributeKind::Enum,
                values: [
                    "room_id", "room_name", "room_area", "dimension", "level_name", "other",
                ]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
                required: true,
                default: Some(Value::String("other".to_owned())),
                description: String::new(),
            }],
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: vec![LinkDef {
                name: "space".to_owned(),
                targets: vec!["space".to_owned()],
                min: 0,
                max: 1,
            }],
            ignore: false,
        },
        LabelClass {
            name: "ignore".to_owned(),
            geometry: GeometryKind::Polygon,
            color: "#dc2626".to_owned(),
            description: "Furniture, hatching, the title block, the legend. Excluded from the loss rather than labelled as background.".to_owned(),
            attributes: Vec::new(),
            keypoints: Vec::new(),
            optional_keypoints: Vec::new(),
            links: Vec::new(),
            ignore: true,
        },
    ]
}
