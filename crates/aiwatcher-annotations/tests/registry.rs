//! End-to-end behaviour of the annotation registry, over an in-memory store.
//!
//! The tests here are named after the mistakes they exist to prevent, because
//! every one of them produces a working pipeline and a wrong number: a test set
//! that shares a building with the training set, a commercial export containing
//! a non-commercial corpus, a door with no hinge that trains a model to predict
//! swing from nothing at all.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use aiwatcher_annotations::{
    Annotation, ExclusionReason, ExportRequest, Geometry, Keypoint, LabelClass, Origin,
    RegisterImageRequest, Registry, ReviewRequest, ReviewState, RightsPolicy, SaveProjectRequest,
    SaveRevisionRequest, Split, SplitRatios, UsageRights, ViewType, floor_plan_classes, split_for,
};
use aiwatcher_prompts::adapters::memory::MemoryObjectStore;
use serde_json::{Value, json};

fn registry() -> Registry {
    Registry::new(Arc::new(MemoryObjectStore::new()), "annotations")
}

fn project_request() -> SaveProjectRequest {
    SaveProjectRequest {
        name: "floor-plans/dom-projekt".to_owned(),
        description: "Catalogue plans".to_owned(),
        classes: floor_plan_classes(),
        splits: SplitRatios::default(),
        split_salt: "2026-09".to_owned(),
        split_overrides: BTreeMap::new(),
    }
}

async fn seeded() -> (Registry, String) {
    let registry = registry();
    let project = registry.save_project(project_request()).await.unwrap();
    (registry, project.name)
}

fn image_id(seed: u8) -> String {
    (0..32).map(|_| format!("{seed:02x}")).collect()
}

fn register(project: &str, seed: u8, group: &str, rights: UsageRights) -> RegisterImageRequest {
    RegisterImageRequest {
        project: project.to_owned(),
        image_id: image_id(seed),
        uri: format!("{}{}", aiwatcher_annotations::BLOB_SCHEME, image_id(seed)),
        width: 1064,
        height: 1021,
        group_id: group.to_owned(),
        source: "dom-projekt".to_owned(),
        rights,
        view: ViewType::FloorPlan,
        level: Some("ground_floor".to_owned()),
        metadata: BTreeMap::new(),
    }
}

fn owned() -> UsageRights {
    UsageRights::Owned {
        grant: "supplier agreement 2026-03".to_owned(),
    }
}

fn wall(id: &str, points: Vec<[f64; 2]>) -> Annotation {
    Annotation {
        id: id.to_owned(),
        class: "wall".to_owned(),
        geometry: Geometry::Polyline { points },
        attributes: BTreeMap::from([
            ("role".to_owned(), json!("exterior")),
            ("thickness_px".to_owned(), json!(14.0)),
        ]),
        links: BTreeMap::new(),
        origin: Origin::Human,
        confidence: None,
        text: None,
    }
}

fn keypoints(names: &[(&str, [f64; 2])]) -> Geometry {
    Geometry::Keypoints {
        points: names
            .iter()
            .map(|(name, at)| Keypoint {
                name: (*name).to_owned(),
                at: *at,
                visible: true,
            })
            .collect(),
    }
}

fn door(id: &str, wall_id: &str) -> Annotation {
    Annotation {
        id: id.to_owned(),
        class: "door".to_owned(),
        geometry: keypoints(&[
            ("opening_start", [120.0, 200.0]),
            ("opening_end", [200.0, 200.0]),
            ("hinge", [120.0, 200.0]),
            ("leaf_end", [120.0, 280.0]),
        ]),
        attributes: BTreeMap::from([("door_type".to_owned(), json!("hinged"))]),
        links: BTreeMap::from([("wall".to_owned(), vec![wall_id.to_owned()])]),
        origin: Origin::Human,
        confidence: None,
        text: None,
    }
}

fn export_request(project: &str) -> ExportRequest {
    ExportRequest {
        project: project.to_owned(),
        note: String::new(),
        rights_policy: RightsPolicy::Commercial,
        require_human_review: true,
        classes: Vec::new(),
        splits: None,
        split_salt: None,
        all_view_types: false,
    }
}

async fn accept(registry: &Registry, project: &str, seed: u8, annotations: Vec<Annotation>) {
    registry
        .save_revision(
            SaveRevisionRequest {
                project: project.to_owned(),
                image_id: image_id(seed),
                annotations,
                notes: String::new(),
                accept: true,
            },
            "kasia",
        )
        .await
        .unwrap();
}

#[test]
fn every_rendering_of_one_building_lands_on_the_same_side_of_the_split() {
    // The whole reason `group_id` exists. The plain plan, its mirror and the
    // garage variant are one family; splitting them apart measures whether the
    // network memorised a house.
    let ratios = SplitRatios::default();
    let family = split_for("komancza-dws", "salt", ratios);
    for _ in 0..3 {
        assert_eq!(split_for("komancza-dws", "salt", ratios), family);
    }
}

#[test]
fn adding_a_family_never_moves_an_existing_one_and_a_new_salt_re_deals_them_all() {
    let ratios = SplitRatios::default();
    let deal = |salt: &str| -> Vec<Split> {
        (0..60)
            .map(|index| split_for(&format!("house-{index}"), salt, ratios))
            .collect()
    };
    assert_eq!(deal("salt"), deal("salt"));
    assert_ne!(deal("salt"), deal("other"));
}

#[tokio::test]
async fn a_door_missing_a_required_keypoint_is_refused_and_the_message_names_it() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 1, "komancza-dws", owned()))
        .await
        .unwrap();

    // `hinge` and `leaf_end` are optional — a sliding door has neither.
    // `opening_end` is not, because an opening with one end has no width.
    let half_drawn = Annotation {
        geometry: keypoints(&[("opening_start", [120.0, 200.0])]),
        ..door("door_1", "wall_1")
    };
    let error = registry
        .save_revision(
            SaveRevisionRequest {
                project,
                image_id: image_id(1),
                annotations: vec![
                    wall("wall_1", vec![[100.0, 200.0], [400.0, 200.0]]),
                    half_drawn,
                ],
                notes: String::new(),
                accept: false,
            },
            "kasia",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("opening_end"), "{error}");
}

#[tokio::test]
async fn a_sliding_door_with_no_hinge_is_accepted() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 21, "komancza-dws", owned()))
        .await
        .unwrap();
    let sliding = Annotation {
        geometry: keypoints(&[
            ("opening_start", [120.0, 200.0]),
            ("opening_end", [200.0, 200.0]),
        ]),
        attributes: BTreeMap::from([("door_type".to_owned(), json!("sliding"))]),
        ..door("door_1", "wall_1")
    };
    registry
        .save_revision(
            SaveRevisionRequest {
                project,
                image_id: image_id(21),
                annotations: vec![
                    wall("wall_1", vec![[100.0, 200.0], [400.0, 200.0]]),
                    sliding,
                ],
                notes: String::new(),
                accept: false,
            },
            "kasia",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn an_opening_pointing_at_a_wall_that_is_not_in_the_image_is_refused() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 2, "komancza-dws", owned()))
        .await
        .unwrap();
    let error = registry
        .save_revision(
            SaveRevisionRequest {
                project,
                image_id: image_id(2),
                annotations: vec![door("door_1", "wall_that_does_not_exist")],
                notes: String::new(),
                accept: false,
            },
            "kasia",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not in this image"), "{error}");
}

#[tokio::test]
async fn drawing_the_same_shapes_twice_is_one_revision() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 3, "komancza-dws", owned()))
        .await
        .unwrap();
    let shapes = vec![wall("wall_1", vec![[100.0, 200.0], [400.0, 200.0]])];
    let save = |annotations: Vec<Annotation>, notes: &str, author: &'static str| {
        let registry = &registry;
        let project = project.clone();
        let notes = notes.to_owned();
        async move {
            registry
                .save_revision(
                    SaveRevisionRequest {
                        project,
                        image_id: image_id(3),
                        annotations,
                        notes,
                        accept: false,
                    },
                    author,
                )
                .await
                .unwrap()
        }
    };

    let first = save(shapes.clone(), "first pass", "kasia").await;
    let second = save(shapes, "same drawing, different hands", "marek").await;

    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.revision.revision, second.revision.revision);
    assert_eq!(second.head.revisions.len(), 1);
    // Identity is the drawing, not who saved it — the same rule a prompt
    // version follows.
    assert_eq!(second.revision.author, "kasia");
}

#[tokio::test]
async fn a_commercial_export_leaves_out_a_research_only_image_and_says_so_by_name() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 4, "komancza-dws", owned()))
        .await
        .unwrap();
    registry
        .register_image(register(
            &project,
            5,
            "cubicasa-00042",
            UsageRights::ResearchOnly {
                license: "CC BY-NC 4.0".to_owned(),
                url: None,
            },
        ))
        .await
        .unwrap();
    let shapes = vec![wall("wall_1", vec![[100.0, 200.0], [400.0, 200.0]])];
    accept(&registry, &project, 4, shapes.clone()).await;
    accept(&registry, &project, 5, shapes).await;

    let built = registry.export(export_request(&project)).await.unwrap();
    assert_eq!(built.manifest.samples.len(), 1);
    assert_eq!(built.manifest.samples[0].image_id, image_id(4));
    let excluded = &built.manifest.excluded;
    assert_eq!(excluded.len(), 1);
    assert_eq!(excluded[0].image_id, image_id(5));
    assert_eq!(excluded[0].reason, ExclusionReason::Rights);
    assert!(excluded[0].detail.contains("CC BY-NC"), "{:?}", excluded[0]);

    // The same corpus is available to a research export, which has to be asked
    // for and says so in its manifest forever.
    let research = registry
        .export(ExportRequest {
            rights_policy: RightsPolicy::Research,
            ..export_request(&project)
        })
        .await
        .unwrap();
    assert_eq!(research.manifest.samples.len(), 2);
    assert_ne!(research.manifest.export, built.manifest.export);
}

#[tokio::test]
async fn an_image_nobody_accepted_is_excluded_rather_than_silently_dropped() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 6, "house-a", owned()))
        .await
        .unwrap();
    registry
        .save_revision(
            SaveRevisionRequest {
                project: project.clone(),
                image_id: image_id(6),
                annotations: vec![wall("wall_1", vec![[10.0, 20.0], [400.0, 20.0]])],
                notes: String::new(),
                accept: false,
            },
            "kasia",
        )
        .await
        .unwrap();

    let built = registry.export(export_request(&project)).await.unwrap();
    assert!(built.manifest.samples.is_empty());
    assert_eq!(
        built.manifest.excluded[0].reason,
        ExclusionReason::Unreviewed
    );
    assert_eq!(built.manifest.counts.excluded, 1);
}

#[tokio::test]
async fn a_revision_that_is_entirely_model_output_does_not_reach_a_reviewed_export() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 7, "house-b", owned()))
        .await
        .unwrap();
    let proposed = Annotation {
        origin: Origin::Model,
        confidence: Some(0.91),
        ..wall("wall_1", vec![[10.0, 20.0], [400.0, 20.0]])
    };
    accept(&registry, &project, 7, vec![proposed]).await;

    let strict = registry.export(export_request(&project)).await.unwrap();
    assert!(strict.manifest.samples.is_empty());
    assert_eq!(
        strict.manifest.excluded[0].reason,
        ExclusionReason::Unreviewed
    );

    let permissive = registry
        .export(ExportRequest {
            require_human_review: false,
            ..export_request(&project)
        })
        .await
        .unwrap();
    assert_eq!(permissive.manifest.samples.len(), 1);
}

#[tokio::test]
async fn rebuilding_an_unchanged_project_is_the_same_export() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 8, "house-c", owned()))
        .await
        .unwrap();
    accept(
        &registry,
        &project,
        8,
        vec![wall("wall_1", vec![[10.0, 20.0], [400.0, 20.0]])],
    )
    .await;

    let first = registry.export(export_request(&project)).await.unwrap();
    let second = registry.export(export_request(&project)).await.unwrap();
    assert!(first.created);
    assert!(!second.created);
    assert_eq!(first.manifest.export, second.manifest.export);
    assert_eq!(registry.exports(&project).await.unwrap().exports.len(), 1);
}

#[tokio::test]
async fn changing_the_schema_excludes_every_revision_drawn_under_the_old_one_by_name() {
    // The loud failure is the correct one. A rename that quietly relabelled
    // history would be undetectable afterwards.
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 9, "house-d", owned()))
        .await
        .unwrap();
    accept(
        &registry,
        &project,
        9,
        vec![wall("wall_1", vec![[10.0, 20.0], [400.0, 20.0]])],
    )
    .await;

    let mut classes = floor_plan_classes();
    classes.push(LabelClass {
        name: "chimney".to_owned(),
        geometry: aiwatcher_annotations::GeometryKind::Polygon,
        color: "#000000".to_owned(),
        description: String::new(),
        attributes: Vec::new(),
        keypoints: Vec::new(),
        optional_keypoints: Vec::new(),
        links: Vec::new(),
        ignore: false,
    });
    registry
        .save_project(SaveProjectRequest {
            classes,
            ..project_request()
        })
        .await
        .unwrap();

    let built = registry.export(export_request(&project)).await.unwrap();
    assert!(built.manifest.samples.is_empty());
    assert_eq!(
        built.manifest.excluded[0].reason,
        ExclusionReason::SchemaMismatch
    );
}

#[tokio::test]
async fn a_section_drawing_never_reaches_a_floor_plan_export() {
    let (registry, project) = seeded().await;
    registry
        .register_image(RegisterImageRequest {
            view: ViewType::Section,
            ..register(&project, 10, "house-e", owned())
        })
        .await
        .unwrap();
    accept(
        &registry,
        &project,
        10,
        vec![wall("wall_1", vec![[10.0, 20.0], [400.0, 20.0]])],
    )
    .await;

    let built = registry.export(export_request(&project)).await.unwrap();
    assert_eq!(built.manifest.excluded[0].reason, ExclusionReason::ViewType);
}

#[tokio::test]
async fn coco_category_ids_follow_the_schema_and_not_the_order_shapes_were_drawn() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 11, "house-f", owned()))
        .await
        .unwrap();
    accept(
        &registry,
        &project,
        11,
        vec![
            door("door_1", "wall_1"),
            wall("wall_1", vec![[100.0, 200.0], [400.0, 200.0]]),
        ],
    )
    .await;
    let built = registry.export(export_request(&project)).await.unwrap();

    let coco = registry
        .coco(&project, &built.manifest.export, None)
        .await
        .unwrap();
    let categories = coco["categories"].as_array().unwrap();
    assert_eq!(categories[0]["name"], json!("wall"));
    assert_eq!(categories[0]["id"], json!(1));

    let annotations = coco["annotations"].as_array().unwrap();
    assert_eq!(annotations.len(), 2);
    let door = annotations
        .iter()
        .find(|record| record["aiwatcher"]["annotation_id"] == json!("door_1"))
        .unwrap();
    // Four declared keypoints, COCO's flat (x, y, v) triples, in schema order.
    assert_eq!(door["keypoints"].as_array().unwrap().len(), 12);
    assert_eq!(door["aiwatcher"]["geometry"]["kind"], json!("keypoints"));
    assert_eq!(door["aiwatcher"]["links"]["wall"], json!(["wall_1"]));
}

#[tokio::test]
async fn accepting_an_image_has_to_name_the_revision_it_accepts() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 12, "house-g", owned()))
        .await
        .unwrap();
    let error = registry
        .review(
            ReviewRequest {
                project,
                image_id: image_id(12),
                review: ReviewState::Accepted,
                revision: None,
                note: String::new(),
            },
            "reviewer",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("name the revision"), "{error}");
}

#[tokio::test]
async fn an_upload_is_keyed_by_the_digest_the_server_computed() {
    let registry = registry();
    let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
    let stored = registry.put_blob(png.clone(), "").await.unwrap();
    assert_eq!(stored.content_type, "image/png");
    assert!(stored.created);
    assert_eq!(
        stored.uri,
        format!("{}{}", aiwatcher_annotations::BLOB_SCHEME, stored.image_id)
    );

    let again = registry.put_blob(png, "image/png").await.unwrap();
    assert!(!again.created);
    assert_eq!(again.image_id, stored.image_id);

    let (body, content_type) = registry.blob(&stored.image_id).await.unwrap();
    assert_eq!(content_type, "image/png");
    assert!(body.starts_with(b"\x89PNG"));
}

#[tokio::test]
async fn an_identifier_that_could_walk_out_of_its_prefix_is_refused_before_it_reaches_a_key() {
    let (registry, project) = seeded().await;
    let error = registry
        .image(&project, "../../../etc/passwd", None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("SHA-256"), "{error}");
}

#[test]
fn the_source_catalogue_can_be_narrowed_to_what_a_commercial_model_may_use() {
    let page = aiwatcher_annotations::sources::search(
        None,
        Some(aiwatcher_annotations::SourceUsage::Commercial),
        None,
    );
    assert!(!page.sources.is_empty());
    assert!(
        page.sources
            .iter()
            .all(|source| source.usage == aiwatcher_annotations::SourceUsage::Commercial)
    );
    assert!(page.total > page.sources.len());
    // Every row says when somebody last read the licence at the other end.
    assert!(
        page.sources
            .iter()
            .all(|source| !source.verified_on.is_empty())
    );
    assert!(!page.directories.is_empty());
}

#[tokio::test]
async fn the_project_summary_counts_instances_over_accepted_revisions_only() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 13, "house-h", owned()))
        .await
        .unwrap();
    registry
        .register_image(register(&project, 14, "house-i", owned()))
        .await
        .unwrap();
    accept(
        &registry,
        &project,
        13,
        vec![
            wall("wall_1", vec![[100.0, 200.0], [400.0, 200.0]]),
            door("door_1", "wall_1"),
        ],
    )
    .await;
    registry
        .save_revision(
            SaveRevisionRequest {
                project: project.clone(),
                image_id: image_id(14),
                annotations: vec![wall("wall_9", vec![[1.0, 2.0], [40.0, 2.0]])],
                notes: String::new(),
                accept: false,
            },
            "kasia",
        )
        .await
        .unwrap();

    let summary = registry.project_summary(&project).await.unwrap();
    assert_eq!(summary.images, 2);
    assert_eq!(summary.accepted, 1);
    assert_eq!(summary.groups, 2);
    assert_eq!(summary.instances, 2);
    assert_eq!(summary.per_class.get("door"), Some(&1));
    assert_eq!(summary.per_class.get("wall"), Some(&1));
}

#[tokio::test]
async fn an_export_reference_is_the_string_a_training_run_records() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 15, "house-j", owned()))
        .await
        .unwrap();
    accept(
        &registry,
        &project,
        15,
        vec![wall("wall_1", vec![[10.0, 20.0], [400.0, 20.0]])],
    )
    .await;
    let built = registry.export(export_request(&project)).await.unwrap();
    let reference = built.manifest.reference();
    let (name, version) = reference.rsplit_once('@').unwrap();
    assert_eq!(name, project);
    assert_eq!(version, built.manifest.export);
    assert_eq!(version.len(), 64);
}

#[tokio::test]
async fn a_shape_drawn_off_the_canvas_is_refused_with_the_image_size_in_the_message() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 16, "house-k", owned()))
        .await
        .unwrap();
    let error = registry
        .save_revision(
            SaveRevisionRequest {
                project,
                image_id: image_id(16),
                annotations: vec![wall("wall_1", vec![[10.0, 20.0], [9_000.0, 20.0]])],
                notes: String::new(),
                accept: false,
            },
            "kasia",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("1064x1021"), "{error}");
}

#[tokio::test]
async fn the_split_a_labeller_sees_is_the_one_the_export_will_use() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 17, "house-l", owned()))
        .await
        .unwrap();
    let detail = registry.image(&project, &image_id(17), None).await.unwrap();
    assert_eq!(
        detail.split,
        split_for("house-l", "2026-09", SplitRatios::default())
    );
}

#[tokio::test]
async fn an_explicit_override_beats_the_hash_so_a_named_house_can_be_held_out() {
    let registry = registry();
    let project = registry
        .save_project(SaveProjectRequest {
            split_overrides: BTreeMap::from([("komancza-dws".to_owned(), Split::Test)]),
            ..project_request()
        })
        .await
        .unwrap();
    registry
        .register_image(register(&project.name, 18, "komancza-dws", owned()))
        .await
        .unwrap();
    let detail = registry
        .image(&project.name, &image_id(18), None)
        .await
        .unwrap();
    assert_eq!(detail.split, Split::Test);
}

#[tokio::test]
async fn split_ratios_that_do_not_add_up_are_refused_when_the_project_is_saved() {
    let registry = registry();
    let error = registry
        .save_project(SaveProjectRequest {
            splits: SplitRatios {
                train: 80,
                validation: 15,
                test: 15,
            },
            ..project_request()
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("add up to 100"), "{error}");
}

#[tokio::test]
async fn every_problem_in_a_drawing_is_reported_at_once() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 19, "house-m", owned()))
        .await
        .unwrap();
    let missing_thickness_and_a_bad_enum = Annotation {
        attributes: BTreeMap::from([("role".to_owned(), json!("bearing"))]),
        ..wall("wall_1", vec![[10.0, 20.0], [400.0, 20.0]])
    };
    let unknown_class = Annotation {
        id: "thing_1".to_owned(),
        class: "chimney".to_owned(),
        geometry: Geometry::Point { at: [5.0, 5.0] },
        attributes: BTreeMap::new(),
        links: BTreeMap::new(),
        origin: Origin::Human,
        confidence: None,
        text: None,
    };
    let error = registry
        .save_revision(
            SaveRevisionRequest {
                project,
                image_id: image_id(19),
                annotations: vec![missing_thickness_and_a_bad_enum, unknown_class],
                notes: String::new(),
                accept: false,
            },
            "kasia",
        )
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("thickness_px"), "{message}");
    assert!(message.contains("bearing"), "{message}");
    assert!(message.contains("chimney"), "{message}");
}

#[tokio::test]
async fn the_generated_coco_carries_the_export_it_came_from() {
    let (registry, project) = seeded().await;
    registry
        .register_image(register(&project, 20, "house-n", owned()))
        .await
        .unwrap();
    accept(
        &registry,
        &project,
        20,
        vec![Annotation {
            id: "space_1".to_owned(),
            class: "space".to_owned(),
            geometry: Geometry::Polygon {
                exterior: vec![[0.0, 0.0], [100.0, 0.0], [100.0, 50.0], [0.0, 50.0]],
                holes: Vec::new(),
            },
            attributes: BTreeMap::from([("printed_area_m2".to_owned(), json!(49.01))]),
            links: BTreeMap::new(),
            origin: Origin::Human,
            confidence: None,
            text: None,
        }],
    )
    .await;
    let built = registry.export(export_request(&project)).await.unwrap();
    let coco: Value = registry
        .coco(&project, &built.manifest.export, None)
        .await
        .unwrap();
    assert_eq!(
        coco["info"]["aiwatcher"]["export"],
        json!(built.manifest.export)
    );
    let annotation = &coco["annotations"][0];
    assert_eq!(annotation["area"], json!(5000.0));
    assert_eq!(
        annotation["segmentation"],
        json!([[0.0, 0.0, 100.0, 0.0, 100.0, 50.0, 0.0, 50.0]])
    );
}
