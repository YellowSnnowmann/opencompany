use super::*;
use crate::feedback::types::{ConsentMode, FeedbackInput};

fn item(category: FeedbackCategory, template: Option<&str>) -> FeedbackItem {
    FeedbackItem::capture(
        FeedbackInput {
            category,
            note: "n".into(),
            work_ref: None,
            template_name: template.map(str::to_string),
            template_version: None,
        },
        "0.1.0",
        ConsentMode::Auto,
    )
}

#[test]
fn labels_cover_all_axes_and_source() {
    let labels = labels_for(&item(FeedbackCategory::Bug, None));
    assert!(labels.contains(&"feedback".to_string()));
    assert!(labels.contains(&"type/bug".to_string()));
    assert!(labels.contains(&"area/runtime".to_string()));
    assert!(labels.iter().any(|l| l.starts_with("sev/")));
    assert!(labels.contains(&"source/agent-filed".to_string()));
}

#[test]
fn template_gap_names_the_template_area() {
    let labels = labels_for(&item(
        FeedbackCategory::TemplateGap,
        Some("marketing_agency"),
    ));
    assert!(labels.contains(&"area/template:marketing_agency".to_string()));
}

#[test]
fn wrong_output_owns_the_brain() {
    let labels = labels_for(&item(FeedbackCategory::WrongOutput, None));
    assert!(labels.contains(&"area/brain".to_string()));
}
