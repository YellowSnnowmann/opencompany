use super::*;

/// A catalogue of `count` synthetic actions for `toolkit`, each with a
/// realistically chunky description and parameter schema.
fn catalogue(toolkit: &str, count: usize) -> Vec<CatalogAction> {
    (0..count)
        .map(|i| CatalogAction {
            slug: format!("{}_ACTION_{i:03}", toolkit.to_ascii_uppercase()),
            toolkit: toolkit.to_string(),
            description: format!(
                "Performs operation {i} on the {toolkit} account. {}",
                "Long upstream prose that Composio publishes for every action. ".repeat(4)
            ),
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "owner": {"type": "string", "description": "x".repeat(200)},
                    "repo": {"type": "string", "description": "y".repeat(200)},
                },
                "required": ["owner"]
            })),
        })
        .collect()
}

fn request(search: &str, detail: Detail, toolkits: &[&str]) -> ListRequest {
    ListRequest {
        toolkits: toolkits.iter().map(|t| t.to_string()).collect(),
        search: search_terms(search),
        tags: Vec::new(),
        curated: false,
        detail,
        limit: detail.default_limit(),
    }
}

// ── the root cause: a message cap applied to a body ────────────────

// ── the toolkit listing ────────────────────────────────────────────

fn toolkit_catalogue(count: usize) -> Vec<CatalogToolkit> {
    (0..count)
        .map(|i| CatalogToolkit {
            slug: format!("toolkit{i:03}"),
            name: format!("Toolkit {i}"),
            description: "An integration with a long upstream description. ".repeat(4),
            connected: Some(i % 3 == 0),
        })
        .collect()
}

// -----------------------------------------------------------------------
// The capability-grounding + Composio-first routing brief (issue #1759)
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// The http_request deflection guardrail (issue #1759, slice S2)
// -----------------------------------------------------------------------

#[path = "composio_catalog_tests_part1.rs"]
mod tests_part1;
#[path = "composio_catalog_tests_part2.rs"]
mod tests_part2;
