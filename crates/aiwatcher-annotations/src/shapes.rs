//! The drawings themselves, and the rules that decide a drawing is finished.
//!
//! Everything here is vector. A mask is produced from these shapes and is
//! never the source — see ADR_0017. The validation in this module is the
//! difference between "somebody drew something" and "this is a training
//! target": a door with no hinge, an opening pointing at a wall that is not in
//! the image, or a polygon with two points are all things a labeller produces
//! and none of them should reach an export.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::schema::{AttributeKind, GeometryKind, LabelSchema};
use crate::{Error, Result, validate_slug};

/// How far outside the image a point may sit before it is a mistake.
///
/// Not zero: a wall centreline is drawn through the middle of a stroke that
/// runs to the edge, and rounding puts it a pixel over. Not unbounded: a shape
/// a hundred pixels off the canvas is a pan that went wrong, and catching it
/// on save is cheaper than finding it in a mask three weeks later.
const OUT_OF_FRAME_MARGIN: f64 = 8.0;

/// A position in the original image's pixel coordinates, origin top-left.
///
/// Original, not resized: a model input is a crop and a scale of this, and the
/// transform belongs to the training run rather than to the label. Storing
/// normalised coordinates instead would make every annotation depend on a
/// preprocessing decision nobody can recover later.
pub type Point = [f64; 2];

/// One named position inside a keypoint instance.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Keypoint {
    pub name: String,
    #[schema(value_type = Vec<f64>)]
    pub at: Point,
    /// False when the plan shows the element but not this position — a door
    /// whose leaf is drawn open past the page edge still has a hinge.
    #[serde(default = "default_true")]
    pub visible: bool,
}

/// What was drawn.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Geometry {
    Point {
        #[schema(value_type = Vec<f64>)]
        at: Point,
    },
    /// Renamed for the same reason [`GeometryKind::BBox`] is: the derived
    /// snake_case of `BBox` is `b_box`.
    #[serde(rename = "bbox")]
    BBox {
        #[schema(value_type = Vec<f64>)]
        min: Point,
        #[schema(value_type = Vec<f64>)]
        max: Point,
    },
    Polyline {
        #[schema(value_type = Vec<Vec<f64>>)]
        points: Vec<Point>,
    },
    Polygon {
        #[schema(value_type = Vec<Vec<f64>>)]
        exterior: Vec<Point>,
        /// Interior rings. A room with a chimney is one polygon with one hole,
        /// not two polygons whose difference a consumer has to infer.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[schema(value_type = Vec<Vec<Vec<f64>>>)]
        holes: Vec<Vec<Point>>,
    },
    Keypoints {
        points: Vec<Keypoint>,
    },
}

impl Geometry {
    #[must_use]
    pub const fn kind(&self) -> GeometryKind {
        match self {
            Self::Point { .. } => GeometryKind::Point,
            Self::BBox { .. } => GeometryKind::BBox,
            Self::Polyline { .. } => GeometryKind::Polyline,
            Self::Polygon { .. } => GeometryKind::Polygon,
            Self::Keypoints { .. } => GeometryKind::Keypoints,
        }
    }

    /// Every point in this shape, in drawing order.
    #[must_use]
    pub fn points(&self) -> Vec<Point> {
        match self {
            Self::Point { at } => vec![*at],
            Self::BBox { min, max } => vec![*min, *max],
            Self::Polyline { points } => points.clone(),
            Self::Polygon { exterior, holes } => exterior
                .iter()
                .chain(holes.iter().flatten())
                .copied()
                .collect(),
            Self::Keypoints { points } => points.iter().map(|point| point.at).collect(),
        }
    }

    /// The axis-aligned box around this shape: `[min_x, min_y, width, height]`.
    ///
    /// `None` for a shape with no points, which validation rejects anyway.
    #[must_use]
    pub fn bounds(&self) -> Option<[f64; 4]> {
        let points = self.points();
        let first = points.first()?;
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (first[0], first[1], first[0], first[1]);
        for point in &points {
            min_x = min_x.min(point[0]);
            min_y = min_y.min(point[1]);
            max_x = max_x.max(point[0]);
            max_y = max_y.max(point[1]);
        }
        Some([min_x, min_y, max_x - min_x, max_y - min_y])
    }

    /// The shoelace area of a polygon's exterior, less its holes. Zero for
    /// every other kind.
    ///
    /// Used for the COCO `area` field and for the sanity check that compares a
    /// room's drawn area against the one printed on the plan.
    #[must_use]
    pub fn area(&self) -> f64 {
        match self {
            Self::Polygon { exterior, holes } => {
                let outer = ring_area(exterior);
                let inner: f64 = holes.iter().map(|hole| ring_area(hole)).sum();
                (outer - inner).max(0.0)
            }
            _ => 0.0,
        }
    }

    fn validate(&self, width: u32, height: u32) -> Result<()> {
        let minimum = match self {
            Self::Point { .. } => 1,
            Self::BBox { .. } => 2,
            Self::Polyline { points } => {
                if points.len() < 2 {
                    return Err(Error::Invalid(
                        "a polyline needs at least two points".to_owned(),
                    ));
                }
                2
            }
            Self::Polygon { exterior, holes } => {
                if exterior.len() < 3 {
                    return Err(Error::Invalid(
                        "a polygon needs at least three points".to_owned(),
                    ));
                }
                if let Some(hole) = holes.iter().find(|hole| hole.len() < 3) {
                    return Err(Error::Invalid(format!(
                        "a polygon hole needs at least three points; one has {}",
                        hole.len()
                    )));
                }
                3
            }
            Self::Keypoints { points } => {
                if points.is_empty() {
                    return Err(Error::Invalid(
                        "a keypoint instance needs at least one point".to_owned(),
                    ));
                }
                1
            }
        };
        let _ = minimum;

        if let Self::BBox { min, max } = self
            && (max[0] <= min[0] || max[1] <= min[1])
        {
            return Err(Error::Invalid(
                "a bounding box needs a positive width and height".to_owned(),
            ));
        }

        let limit_x = f64::from(width) + OUT_OF_FRAME_MARGIN;
        let limit_y = f64::from(height) + OUT_OF_FRAME_MARGIN;
        for point in self.points() {
            if !point[0].is_finite() || !point[1].is_finite() {
                return Err(Error::Invalid(
                    "a coordinate is not a finite number".to_owned(),
                ));
            }
            if point[0] < -OUT_OF_FRAME_MARGIN
                || point[1] < -OUT_OF_FRAME_MARGIN
                || point[0] > limit_x
                || point[1] > limit_y
            {
                return Err(Error::Invalid(format!(
                    "the point [{:.1}, {:.1}] is outside a {width}x{height} image",
                    point[0], point[1]
                )));
            }
        }
        Ok(())
    }
}

/// Who drew a shape.
///
/// The whole reason a model-assisted pass is affordable: pre-annotation is
/// welcome and is recorded as such, so an export can require that a human
/// looked at it. A field that defaulted to `human` for machine output would
/// make the distinction unrecoverable one import later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Human,
    /// A model proposed it. Still a draft until somebody accepts the revision.
    Model,
    /// It came from another annotation tool or a public dataset.
    Import,
    /// A text recogniser produced it, and `text` holds what it read.
    Ocr,
}

impl Origin {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Model => "model",
            Self::Import => "import",
            Self::Ocr => "ocr",
        }
    }
}

/// One drawn instance.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct Annotation {
    /// Unique within its revision. Supplied by the client so links can be
    /// drawn before a save, and validated rather than trusted.
    pub id: String,
    pub class: String,
    pub geometry: Geometry,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub attributes: BTreeMap<String, Value>,
    /// Named references to other instances in this revision.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schema(value_type = Object)]
    pub links: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub origin: Origin,
    /// What the producer thought of its own proposal. Present for `model` and
    /// `ocr`, meaningless for `human`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    /// What a recogniser read, for a text instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// Check one revision's worth of shapes against a schema and an image.
///
/// Everything is checked in one pass and every failure is reported, because a
/// labeller fixing one error at a time through six round trips stops using the
/// tool.
pub fn validate(
    annotations: &[Annotation],
    schema: &LabelSchema,
    width: u32,
    height: u32,
) -> Result<()> {
    let mut problems: Vec<String> = Vec::new();
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    let mut classes: BTreeMap<&str, &str> = BTreeMap::new();

    for annotation in annotations {
        if let Err(error) = validate_slug(&annotation.id, "an annotation id") {
            problems.push(error.to_string());
            continue;
        }
        if !ids.insert(annotation.id.as_str()) {
            problems.push(format!("two annotations share the id {}", annotation.id));
        }
        classes.insert(annotation.id.as_str(), annotation.class.as_str());
    }

    for annotation in annotations {
        let Some(class) = schema.class(&annotation.class) else {
            problems.push(format!(
                "{}: the class {} is not in schema {}",
                annotation.id,
                annotation.class,
                &schema.version[..12.min(schema.version.len())]
            ));
            continue;
        };
        if annotation.geometry.kind() != class.geometry {
            problems.push(format!(
                "{}: {} is drawn as a {}, not a {}",
                annotation.id,
                class.name,
                class.geometry.as_str(),
                annotation.geometry.kind().as_str()
            ));
        }
        if let Err(error) = annotation.geometry.validate(width, height) {
            problems.push(format!("{}: {error}", annotation.id));
        }
        if let Geometry::Keypoints { points } = &annotation.geometry {
            let mut present: BTreeSet<&str> = BTreeSet::new();
            for point in points {
                if !class.keypoints.iter().any(|name| name == &point.name) {
                    problems.push(format!(
                        "{}: {} has no keypoint called {}",
                        annotation.id, class.name, point.name
                    ));
                }
                if !present.insert(point.name.as_str()) {
                    problems.push(format!(
                        "{}: the keypoint {} is given twice",
                        annotation.id, point.name
                    ));
                }
            }
            for required in &class.keypoints {
                if class.optional_keypoints.contains(required) {
                    continue;
                }
                if !present.contains(required.as_str()) {
                    problems.push(format!(
                        "{}: {} is missing the keypoint {required}",
                        annotation.id, class.name
                    ));
                }
            }
        }

        for attribute in &class.attributes {
            match annotation.attributes.get(&attribute.name) {
                None if attribute.required && attribute.default.is_none() => {
                    problems.push(format!("{}: {} is required", annotation.id, attribute.name))
                }
                None => {}
                Some(value) => {
                    if let Err(reason) = check_attribute(attribute.kind, &attribute.values, value) {
                        problems.push(format!("{}: {}: {reason}", annotation.id, attribute.name));
                    }
                }
            }
        }
        for name in annotation.attributes.keys() {
            if !class
                .attributes
                .iter()
                .any(|declared| &declared.name == name)
            {
                problems.push(format!(
                    "{}: {} declares no attribute {name}",
                    annotation.id, class.name
                ));
            }
        }

        for (name, targets) in &annotation.links {
            let Some(link) = class.links.iter().find(|link| &link.name == name) else {
                problems.push(format!(
                    "{}: {} declares no link {name}",
                    annotation.id, class.name
                ));
                continue;
            };
            if targets.len() < link.min || targets.len() > link.max {
                problems.push(format!(
                    "{}: {name} needs between {} and {} targets and has {}",
                    annotation.id,
                    link.min,
                    link.max,
                    targets.len()
                ));
            }
            for target in targets {
                match classes.get(target.as_str()) {
                    None => problems.push(format!(
                        "{}: {name} points at {target}, which is not in this image",
                        annotation.id
                    )),
                    Some(target_class)
                        if !link.targets.is_empty()
                            && !link.targets.iter().any(|allowed| allowed == target_class) =>
                    {
                        problems.push(format!(
                            "{}: {name} may not point at a {target_class}",
                            annotation.id
                        ));
                    }
                    Some(_) => {}
                }
            }
        }
        for link in &class.links {
            if link.min > 0 && !annotation.links.contains_key(&link.name) {
                problems.push(format!("{}: {} is required", annotation.id, link.name));
            }
        }
    }

    if problems.is_empty() {
        return Ok(());
    }
    Err(Error::Rejected(problems))
}

fn check_attribute(
    kind: AttributeKind,
    values: &[String],
    value: &Value,
) -> std::result::Result<(), String> {
    match kind {
        AttributeKind::Enum => match value.as_str() {
            Some(text) if values.iter().any(|allowed| allowed == text) => Ok(()),
            Some(text) => Err(format!("{text} is not one of {}", values.join(", "))),
            None => Err("expected one of a closed set of strings".to_owned()),
        },
        AttributeKind::Bool if value.is_boolean() => Ok(()),
        AttributeKind::Bool => Err("expected true or false".to_owned()),
        AttributeKind::Number => match value.as_f64() {
            Some(number) if number.is_finite() => Ok(()),
            _ => Err("expected a finite number".to_owned()),
        },
        AttributeKind::Text if value.is_string() => Ok(()),
        AttributeKind::Text => Err("expected a string".to_owned()),
    }
}

fn ring_area(ring: &[Point]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut total = 0.0;
    for index in 0..ring.len() {
        let current = ring[index];
        let next = ring[(index + 1) % ring.len()];
        total += current[0].mul_add(next[1], -(next[0] * current[1]));
    }
    (total / 2.0).abs()
}

const fn default_true() -> bool {
    true
}
