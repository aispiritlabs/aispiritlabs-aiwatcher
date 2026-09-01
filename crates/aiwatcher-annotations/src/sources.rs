//! Where floor-plan training data comes from, and what may be done with it.
//!
//! A table a human wrote and dated, not a client. Hugging Face, Kaggle and
//! Roboflow Universe all mirror these corpora and all of them routinely
//! restate the licence wrongly — a CC BY-NC dataset re-uploaded as MIT is
//! common enough that fetching a licence live would be worse than useless,
//! because it would arrive looking authoritative.
//!
//! So every row here says what it says *as of* a date, links the original, and
//! errs towards [`SourceUsage::Unclear`]. It is a signpost. It is never a
//! permission, and the only thing that is is the licence text at the other end
//! of the link.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// What kind of thing a corpus contains.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Rendered or scanned plans with pixel or vector labels.
    FloorPlan,
    /// Geometry and topology with no source raster. Useful by rendering it.
    Vector,
    /// CAD drawings and symbol libraries.
    Cad,
    /// Captured interiors: panoramas, point clouds, and a plan derived from
    /// them.
    Capture,
}

/// What the licence permits, stated conservatively.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceUsage {
    /// The licence permits commercial use of the data and of a model trained
    /// on it.
    Commercial,
    /// Research or non-commercial only.
    NonCommercial,
    /// Mixed, unstated, or stated by the authors as not theirs to give.
    Unclear,
}

impl SourceUsage {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commercial => "commercial",
            Self::NonCommercial => "non_commercial",
            Self::Unclear => "unclear",
        }
    }
}

/// How the bytes are obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceAccess {
    /// Downloadable.
    Open,
    /// A form, an agreement, or an email.
    Request,
    /// Restricted to academic or public research institutions.
    Academic,
}

/// One corpus.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct DatasetSource {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    pub summary: String,
    /// Best published figure. `None` where the authors give a range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<u64>,
    pub item_label: String,
    /// What it labels, in this crate's vocabulary: `walls`, `rooms`, `doors`,
    /// `windows`, `openings`, `stairs`, `text`, `scale`, `graph`, `furniture`,
    /// `symbols`.
    pub labels: Vec<String>,
    /// `raster`, `vector`, `cad`, `panorama`.
    pub media: Vec<String>,
    pub license: String,
    pub usage: SourceUsage,
    pub access: SourceAccess,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paper: Option<String>,
    /// When somebody last read the licence at `url`.
    pub verified_on: String,
    pub notes: String,
}

/// A place to go looking, rather than a corpus.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SourceDirectory {
    pub name: String,
    pub url: String,
    pub notes: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct SourcePage {
    pub sources: Vec<DatasetSource>,
    pub directories: Vec<SourceDirectory>,
    /// Rows before the filter, so a caller can tell an empty filter from an
    /// empty table.
    pub total: usize,
}

/// The fields every row shares, so a row states only what is specific to it.
fn row() -> DatasetSource {
    DatasetSource {
        id: String::new(),
        name: String::new(),
        kind: SourceKind::FloorPlan,
        summary: String::new(),
        items: None,
        item_label: "floor plans".to_owned(),
        labels: Vec::new(),
        media: Vec::new(),
        license: String::new(),
        usage: SourceUsage::Unclear,
        access: SourceAccess::Open,
        url: String::new(),
        paper: None,
        verified_on: VERIFIED_ON.to_owned(),
        notes: String::new(),
    }
}

fn words(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// The date every row below was last checked against its original.
const VERIFIED_ON: &str = "2026-09-01";

/// Every corpus this build knows about.
#[must_use]
pub fn catalog() -> Vec<DatasetSource> {
    use SourceAccess::{Academic, Open, Request};
    use SourceKind::{Cad, Capture, FloorPlan, Vector};
    use SourceUsage::{Commercial, NonCommercial, Unclear};

    vec![
        DatasetSource {
            id: "cubicasa5k".to_owned(),
            name: "CubiCasa5K".to_owned(),
            kind: FloorPlan,
            summary: "5,000 Finnish plans as raster plus SVG annotations, with walls, openings, rooms and fixtures as separate layers."
                .to_owned(),
            items: Some(5_000),
            labels: words(&["walls", "rooms", "doors", "windows", "furniture"]),
            media: words(&["raster", "vector"]),
            license: "CC BY-NC 4.0".to_owned(),
            usage: NonCommercial,
            access: Open,
            url: "https://github.com/CubiCasa/CubiCasa5k".to_owned(),
            paper: Some("https://arxiv.org/abs/1904.01920".to_owned()),
            notes: "The best pre-training corpus for this task and the one that cannot ship in a commercial product. Pre-train on it, fine-tune on licensed data, and keep the weights lineage recorded."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "resplan".to_owned(),
            name: "ResPlan".to_owned(),
            kind: Vector,
            summary: "17,000 residential layouts as polygons for walls, doors, windows and rooms, with a room-connectivity graph and a metric scale."
                .to_owned(),
            items: Some(17_000),
            item_label: "layouts".to_owned(),
            labels: words(&["walls", "rooms", "doors", "windows", "graph", "scale"]),
            media: words(&["vector"]),
            license: "CC BY 4.0 (data), MIT (code)".to_owned(),
            usage: Commercial,
            access: Open,
            url: "https://github.com/gsr-lab/ResPlan".to_owned(),
            paper: None,
            notes: "Ships geometry, not source rasters. Its value here is synthetic pre-training: render it in the target drawing style and you have exact raster-to-JSON pairs for free."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "msd".to_owned(),
            name: "Modified Swiss Dwellings".to_owned(),
            kind: FloorPlan,
            summary: "5,372 plans covering 18,900 apartments, with geometry, room types and an access graph."
                .to_owned(),
            items: Some(5_372),
            labels: words(&["walls", "rooms", "doors", "windows", "graph"]),
            media: words(&["raster", "vector"]),
            license: "check the Zenodo record".to_owned(),
            usage: Unclear,
            access: Open,
            url: "https://zenodo.org/records/11073900".to_owned(),
            paper: None,
            notes: "Large Swiss apartment buildings — good for topology and irregular rooms, a weak match for single-family catalogue houses. Read the record's own licence field before using it."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "cvc-fp".to_owned(),
            name: "CVC-FP".to_owned(),
            kind: FloorPlan,
            summary: "122 scanned plans in four drawing styles, annotated with structural elements."
                .to_owned(),
            items: Some(122),
            item_label: "scanned plans".to_owned(),
            labels: words(&["walls", "doors", "windows", "rooms"]),
            media: words(&["raster"]),
            license: "research use, see the site".to_owned(),
            usage: NonCommercial,
            access: Open,
            url: "http://dag.cvc.uab.es/resources/floorplans/".to_owned(),
            paper: None,
            notes: "Too small to train on and unusually useful as a generalisation test: four styles means four ways for a model that memorised one supplier to fail."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "floorplancad".to_owned(),
            name: "FloorPlanCAD".to_owned(),
            kind: Cad,
            summary: "15,663 CAD drawings with panoptic symbol annotations over line primitives."
                .to_owned(),
            items: Some(15_663),
            item_label: "CAD drawings".to_owned(),
            labels: words(&["symbols", "walls", "doors", "windows"]),
            media: words(&["cad", "vector"]),
            license: "CC BY-NC 4.0 (annotations)".to_owned(),
            usage: NonCommercial,
            access: Open,
            url: "https://floorplancad.github.io/".to_owned(),
            paper: Some("https://arxiv.org/abs/2105.07147".to_owned()),
            notes: "The authors state the annotations are theirs and the drawings are not. Two separate rights questions, and the second has no answer on the site."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "waffle".to_owned(),
            name: "WAFFLE".to_owned(),
            kind: FloorPlan,
            summary: "Roughly 20,000 plans scraped from the open web with metadata and OCR, of which only a small subset carries segmentation masks."
                .to_owned(),
            items: Some(20_000),
            labels: words(&["text", "rooms"]),
            media: words(&["raster"]),
            license: "per-item, mostly Wikimedia".to_owned(),
            usage: Unclear,
            access: Open,
            url: "https://github.com/tau-vailab/WAFFLE".to_owned(),
            paper: Some("https://arxiv.org/abs/2406.12734".to_owned()),
            notes: "Style diversity is what it is for: it is the corpus that tells you whether a model learned floor plans or learned one publisher. Its licences are per-item and have to be resolved per-item."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "r2v".to_owned(),
            name: "Raster-to-Vector (R2V) and R3D".to_owned(),
            kind: FloorPlan,
            summary: "The two benchmark splits most floor-plan segmentation papers report on, including DeepFloorplan and the M&M line of work."
                .to_owned(),
            items: None,
            labels: words(&["walls", "rooms", "openings"]),
            media: words(&["raster"]),
            license: "unstated for the source images".to_owned(),
            usage: Unclear,
            access: Open,
            url: "https://github.com/art-programmer/FloorplanTransformation".to_owned(),
            paper: None,
            notes: "Report on these to be comparable with the literature. Note that their 'opening' class merges doors and windows, so a model trained on them alone cannot tell one from the other."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "zind".to_owned(),
            name: "Zillow Indoor Dataset".to_owned(),
            kind: Capture,
            summary: "1,575 homes as panoramas with room layouts, merged floor plans and window, door and opening annotations."
                .to_owned(),
            items: Some(1_575),
            item_label: "homes".to_owned(),
            labels: words(&["rooms", "doors", "windows", "openings", "scale"]),
            media: words(&["panorama", "vector"]),
            license: "non-commercial research licence".to_owned(),
            usage: NonCommercial,
            access: Request,
            url: "https://github.com/zillow/zind".to_owned(),
            paper: Some("https://arxiv.org/abs/2109.13748".to_owned()),
            notes: "Captured rather than drawn, so it teaches geometry and not draughting convention. Useful for the metric-scale half of the problem."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "rplan".to_owned(),
            name: "RPLAN".to_owned(),
            kind: Vector,
            summary: "Tens of thousands of vectorised residential plans, widely used for layout generation."
                .to_owned(),
            items: None,
            labels: words(&["rooms", "walls", "graph"]),
            media: words(&["vector", "raster"]),
            license: "agreement required, research use".to_owned(),
            usage: NonCommercial,
            access: Request,
            url: "http://staff.ustc.edu.cn/~fuxm/projects/DeepLayout/index.html".to_owned(),
            paper: None,
            notes: "Access is by signed agreement. Built for generation rather than extraction, so its labels are room-level and it says little about openings."
                .to_owned(),
            ..row()
        },
        DatasetSource {
            id: "lifull".to_owned(),
            name: "LIFULL HOME'S".to_owned(),
            kind: FloorPlan,
            summary: "Over five million high-resolution Japanese listing plans."
                .to_owned(),
            items: Some(5_300_000),
            labels: words(&["text"]),
            media: words(&["raster"]),
            license: "academic access only".to_owned(),
            usage: NonCommercial,
            access: Academic,
            url: "https://www.nii.ac.jp/dsc/idr/lifull/".to_owned(),
            paper: None,
            notes: "Restricted to universities and public research institutions. Effectively unavailable to a company, whatever its size makes it look like."
                .to_owned(),
            ..row()
        },
    ]
}

/// Places to look for something this table does not list.
#[must_use]
pub fn directories() -> Vec<SourceDirectory> {
    [
        (
            "IAPR TC10 document datasets",
            "https://iapr-tc10.univ-lr.fr/index.php/resources/datasets/",
            "The graphics-recognition community's own list. Where CVC-FP and the older symbol datasets are catalogued.",
        ),
        (
            "Zenodo",
            "https://zenodo.org/search?q=floor+plan+dataset",
            "Where a paper's dataset is deposited when the authors intend it to be citable. Each record states its own licence.",
        ),
        (
            "Hugging Face Datasets",
            "https://huggingface.co/datasets?search=floorplan",
            "Fast to try and unreliable on provenance: many entries are re-uploads whose declared licence does not match the original.",
        ),
        (
            "Kaggle",
            "https://www.kaggle.com/datasets?search=floor+plan",
            "Same caveat as Hugging Face, more strongly. Treat every licence field as a claim by the uploader.",
        ),
        (
            "Roboflow Universe",
            "https://universe.roboflow.com/search?q=floor+plan",
            "Good for a same-day detection experiment. Not a source of rights: most projects are somebody's re-annotation of images they did not own.",
        ),
        (
            "Papers with Code — floor plan analysis",
            "https://paperswithcode.com/task/floorplan-analysis",
            "The fastest way to find which benchmark a new method reports on, which is usually R2V or R3D.",
        ),
    ]
    .into_iter()
    .map(|(name, url, notes)| SourceDirectory {
        name: name.to_owned(),
        url: url.to_owned(),
        notes: notes.to_owned(),
    })
    .collect()
}

/// Filter the table.
///
/// `usage` is the filter that matters: "show me only what a commercial model
/// may be trained on" is the question that should be one click, because the
/// alternative is finding out after the training run.
#[must_use]
pub fn search(query: Option<&str>, usage: Option<SourceUsage>, label: Option<&str>) -> SourcePage {
    let all = catalog();
    let total = all.len();
    let needle = query.map(str::to_lowercase);
    let label = label.map(str::to_lowercase);
    let sources = all
        .into_iter()
        .filter(|source| usage.is_none_or(|wanted| source.usage == wanted))
        .filter(|source| {
            label
                .as_ref()
                .is_none_or(|wanted| source.labels.iter().any(|held| held == wanted))
        })
        .filter(|source| {
            needle.as_ref().is_none_or(|needle| {
                source.id.to_lowercase().contains(needle)
                    || source.name.to_lowercase().contains(needle)
                    || source.summary.to_lowercase().contains(needle)
                    || source.notes.to_lowercase().contains(needle)
                    || source.license.to_lowercase().contains(needle)
                    || source.labels.iter().any(|value| value.contains(needle))
                    || source.media.iter().any(|value| value.contains(needle))
            })
        })
        .collect();
    SourcePage {
        sources,
        directories: directories(),
        total,
    }
}
