use super::*;

pub(super) fn model(id: &str) -> InferenceModel {
    InferenceModel {
        id: id.to_string(),
        name: None,
        context_length: None,
    }
}
