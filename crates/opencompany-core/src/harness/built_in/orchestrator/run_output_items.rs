//! The `items` list of one node's cached run output, read without mistaking
//! an unreadable shape for an empty one.

use serde_json::Value;

/// A node state's `items` array, or `None` when it has no `items` list.
pub(super) fn node_items(state: &Value) -> Option<&Vec<Value>> {
    state.get("items").and_then(Value::as_array)
}

/// The count shown for one node in the unknown-node listing.
pub(super) fn listing_count(state: &Value) -> String {
    match node_items(state) {
        Some(items) => format!("{} item(s)", items.len()),
        None => "output unreadable".to_string(),
    }
}

/// The tool error for a node whose output has no `items` list.
pub(super) fn unreadable_shape(node: &str, run_id: &str) -> String {
    format!(
        "Node `{node}` of run `{run_id}` has output in a shape this tool cannot read (no `items` \
         list). That is not the same as no output — open the run in the console's run drawer \
         instead of re-running the workflow."
    )
}
