//! Issue #410 — how a Composio action catalogue is *narrowed* and *rendered*
//! for an agent, and why every cut it makes describes itself.
//!
//! ## The failure this exists to stop
//!
//! `composio_list_tools` serialized the backend's whole `ComposioToolsResponse`
//! with `serde_json::to_string_pretty` and passed it to
//! [`scrub`](crate::harness::mcp_probe::scrub) on the way out. `scrub` is the
//! MCP *message* sanitiser: its third pass caps at
//! [`SCRUB_MAX_BYTES`](crate::harness::mcp_probe::SCRUB_MAX_BYTES) — **300
//! bytes**, the right size for a one-line failure sentence and three orders of
//! magnitude too small for a catalogue.
//!
//! So a 260-action catalogue reached the model as this, verbatim:
//!
//! ```text
//! {
//!   "tools": [
//!     {
//!       "type": "function",
//!       "function": {
//!         "name": "GITHUB_ACTION_000",
//!         "description": "Performs repository operation number 0. This action…
//! ```
//!
//! The first action, half of its schema, and a bare `…`. Every Composio tool was
//! affected, `composio_execute` included — a successful provider call returned
//! 300 bytes of whatever it actually said.
//!
//! That is worse than a small listing. The agent can see that actions exist but
//! cannot read the name or the parameters of the one it needs, and — because the
//! fragment does not say it is a fragment — it has no reason to ask differently.
//! It reissues the identical call, gets the identical fragment, and is halted by
//! the repetition guard. **A silent cut is what turns one failure into a retry
//! loop.**
//!
//! The fix is in two halves. The *security* half of `scrub` (credential
//! redaction, URL-query stripping) is unconditional and moves to
//! [`redact`](crate::harness::mcp_probe::redact), which never shortens. The
//! *length* half moves here, where a body can be sized as a body and a cut can
//! describe itself.
//!
//! ## The shape
//!
//! Two axes, both generic — nothing here knows what a "GitHub" is:
//!
//! * **Narrowing.** `search` matches whitespace-separated words,
//!   case-insensitively, against the action slug *and* its description; every
//!   word must match (AND), so `list issues` finds `GITHUB_LIST_ISSUES` without
//!   also dragging in every action whose description mentions "list".
//!   `toolkits` still narrows by toolkit, and `limit` bounds the count.
//! * **Progressive disclosure.** [`Detail::Names`] (the default) renders one
//!   line per action — slug plus a clipped description. It is cheap enough that
//!   a whole toolkit fits, which is what lets an agent *discover* a slug.
//!   [`Detail::Schemas`] renders the full JSON parameter schema for the few
//!   actions that matched, which is what lets it *call* one. That is the
//!   two-step an agent actually reasons in: find the name, then read the
//!   arguments.
//!
//! ## The invariant
//!
//! [`render`] bounds its own output — by count *and* by
//! [`MAX_RENDER_BYTES`] — and **never cuts an entry in half**. It stops at a
//! whole-entry boundary, so it always knows exactly how many entries it
//! dropped, and it always says so, in the result, naming the argument to pass
//! next. The budget also sits below the *next* cut downstream — the agent
//! harness's shared 16 KiB per-tool-result budget, which truncates on a byte
//! boundary — so the notice the model reads is the one that counted itself,
//! never an anonymous byte slice. The turn tests assert that directly, on the
//! wire.
//!
//! One deliberate exception: the **first** entry is always emitted whole, even
//! if it alone exceeds the budget. A schemas listing whose only match is a
//! single huge schema must still deliver that schema; returning "0 of 1 shown"
//! would be a correctly-described way of being useless.
//!
//! ## Why this module is not behind the `composio` feature
//!
//! Everything here is pure: no network, no credential, no `openhuman` Composio
//! type. The live tools in [`composio`](crate::harness::composio) are gated on
//! the opt-in `composio` feature, and **CI never builds that feature** (it runs
//! `--features openhuman,tinymemory`; `--all-features` is a `cargo check`, not a
//! `cargo test`). Keeping the narrowing and the truncation notice out here means
//! the behaviour this issue is about is exercised by the test lane that actually
//! runs, rather than by a lane that only type-checks.

use serde_json::{Value, json};

use openhuman_core as oh;

/// Default number of actions a [`Detail::Names`] listing renders.
///
/// Sized to hold a whole large toolkit in one call — the point of the names
/// view is that discovery does not need a second round-trip.
pub const NAMES_DEFAULT_LIMIT: usize = 200;

/// Ceiling an explicit `limit` is clamped to in [`Detail::Names`] mode.
pub const NAMES_MAX_LIMIT: usize = 400;

/// Default number of actions a [`Detail::Schemas`] listing renders. Small on
/// purpose: a single Composio parameter schema routinely runs to a few
/// kilobytes, so "show me five" is already a large result.
pub const SCHEMAS_DEFAULT_LIMIT: usize = 5;

/// Ceiling an explicit `limit` is clamped to in [`Detail::Schemas`] mode.
pub const SCHEMAS_MAX_LIMIT: usize = 20;

/// Byte budget for the rendered body, before the header and the truncation
/// notice are added.
///
/// Chosen to stay comfortably under the agent harness's shared per-tool-result
/// budget (16 KiB at the time of writing) so that **this** module's cut — the
/// one that counts what it dropped and names the argument to narrow with — is
/// the cut the model sees. If the two ever swap places the notice is still
/// emitted; it just risks being clipped, which the header (rendered first)
/// guards against.
pub const MAX_RENDER_BYTES: usize = 11 * 1024;

/// Characters of an action's description kept on a [`Detail::Names`] line.
const DESCRIPTION_PREVIEW_CHARS: usize = 140;

/// Byte budget for a Composio tool body this module cannot structure — an
/// action's provider output, a connections list.
///
/// Same order as [`MAX_RENDER_BYTES`] and for the same reason: large enough
/// that a real answer survives, small enough that the harness's own anonymous
/// cut never fires first.
pub const MAX_BODY_BYTES: usize = 12 * 1024;

/// Bounds an unstructured tool body, appending a trailer that says it was cut,
/// by how much, and what to do about it.
///
/// The counterpart to [`render`] for payloads with no entries to count: a
/// provider's action output. There is no generic argument that makes *that*
/// smaller — the narrowing lives in the action's own parameters — so the
/// trailer says exactly that rather than inventing an argument that does not
/// exist. Naming the size and the cause is still the difference between an
/// agent that adjusts and an agent that repeats itself.
///
/// `what` names the payload for the trailer, e.g. `"GITHUB_LIST_ISSUES output"`.
/// Keys carrying a provider's *self-referential* API plumbing — the endpoints
/// a client would call to re-fetch related collections, never a value anyone
/// asked for.
///
/// **This list used to include `url`, `href` and every `*_url`, and that was
/// wrong** (codex and CodeRabbit on tinyhumansai/opencompany#2153). The
/// premise was that a link is never an answer. It plainly can be: "list the
/// issues with their browser links" makes `html_url` *the* answer, and this
/// projection runs **before** the task-aware extractor and before the artifact
/// store, so a field dropped here cannot be recovered by anything downstream.
///
/// That is the exact mistake this whole change exists to correct — a
/// task-blind reduction deciding what matters without knowing what was asked.
/// Dropping generic link fields bought about 1.6x on measured payloads; the
/// extraction pass is worth orders of magnitude more and knows the task. The
/// trade is not close.
///
/// What remains are the `*_url` siblings that are unambiguously navigation
/// *within the API*: they address collections rather than the record, and a
/// caller that wants them has the record's own id to build them from.
fn is_link_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    matches!(
        key.as_str(),
        "labels_url"
            | "comments_url"
            | "events_url"
            | "timeline_url"
            | "notifications_url"
            | "collaborators_url"
            | "contributors_url"
            | "subscribers_url"
            | "subscription_url"
            | "commits_url"
            | "git_commits_url"
            | "issue_comment_url"
            | "issue_events_url"
            | "assignees_url"
            | "branches_url"
            | "tags_url"
            | "blobs_url"
            | "trees_url"
            | "statuses_url"
            | "languages_url"
            | "stargazers_url"
            | "forks_url"
            | "downloads_url"
            | "releases_url"
            | "deployments_url"
            | "compare_url"
            | "merges_url"
            | "archive_url"
            | "hooks_url"
            | "keys_url"
            | "teams_url"
            | "milestones_url"
            | "pulls_url"
    )
}

/// The single field that stands in for a nested object — a user becomes its
/// `login`, a label its `name`, a milestone its `title`.
///
/// Tried in order, so an object carrying several answers the most specific.
const NESTED_STAND_INS: [&str; 5] = ["login", "name", "title", "slug", "id"];

/// Reduce one record to the fields that carry an answer.
///
/// Scalars are kept as they are. A nested object collapses to its stand-in
/// (`user` → `"octocat"`), an array of objects to the list of theirs
/// (`labels` → `["bug", "p2"]`). Anything else — a nested structure with no
/// obvious name, an empty container — is dropped: it costs bytes and answers
/// nothing, and a model that needs it can ask the action for that record by id.
fn project_record(value: &serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(map) = value else {
        return value.clone();
    };
    let mut out = serde_json::Map::new();
    for (key, val) in map {
        if is_link_key(key) {
            continue;
        }
        match val {
            serde_json::Value::Object(inner) => {
                if let Some(stand_in) = NESTED_STAND_INS
                    .iter()
                    .find_map(|name| inner.get(*name).filter(|v| !v.is_null()))
                {
                    out.insert(key.clone(), stand_in.clone());
                }
            }
            serde_json::Value::Array(items) => {
                let names: Vec<serde_json::Value> = items
                    .iter()
                    .filter_map(|item| match item {
                        serde_json::Value::Object(inner) => NESTED_STAND_INS
                            .iter()
                            .find_map(|name| inner.get(*name).filter(|v| !v.is_null()))
                            .cloned(),
                        scalar if !scalar.is_null() => Some(scalar.clone()),
                        _ => None,
                    })
                    .collect();
                if !names.is_empty() {
                    out.insert(key.clone(), serde_json::Value::Array(names));
                }
            }
            scalar => {
                if !scalar.is_null() {
                    out.insert(key.clone(), scalar.clone());
                }
            }
        }
    }
    serde_json::Value::Object(out)
}

/// Project an array-of-records payload down to its answering fields, returning
/// `None` when the body is not that shape.
///
/// The gap this fills is named in [`bound_body`]'s own docs: "there is no
/// generic argument that makes *that* smaller". True of the action's
/// **arguments** — the narrowing lives in the provider's own parameters — but
/// not of its **encoding**. Most provider output is a list of records, and the
/// bulk of each record is navigation rather than answer. Cutting the payload on
/// a byte boundary keeps whole records and discards whole records; projecting it
/// keeps every record and discards the parts of each that nobody asked for.
///
/// Deliberately conservative: a payload that is not a list of objects is left
/// alone, and every scalar the records do carry survives. This is a lossy
/// transform, so it is reported (see the `[composio] projected` log) rather than
/// applied invisibly — and the trailer `bound_body` appends when the projection
/// is still too big says the same thing it always did.
/// Project an already-parsed payload, returning it unchanged when it holds no
/// array of records. The entry point [`scrubbed_ok`] uses, because by the time
/// a body is a `String` it has been through [`redact`] and is no longer JSON.
pub fn project_records_value(mut value: serde_json::Value) -> serde_json::Value {
    let before = serde_json::to_string(&value).map(|s| s.len()).unwrap_or(0);
    if before <= MAX_BODY_BYTES {
        // Nothing to gain: the payload already fits, and projecting it would
        // drop fields for no reason.
        return value;
    }
    if project_in_place(&mut value) {
        let after = serde_json::to_string(&value)
            .map(|s| s.len())
            .unwrap_or(before);
        tracing::info!(
            from_bytes = before,
            to_bytes = after,
            ratio = format!("{:.1}x", before as f64 / after.max(1) as f64),
            fits_now = after <= MAX_BODY_BYTES,
            "[composio] projected an array-of-records payload to its answering fields"
        );
    }
    value
}

pub fn project_records(body: &str) -> Option<String> {
    let mut parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    // Rewrites every record array in place, wherever it sits. The first cut of
    // this looked one level down and found nothing: Composio returns the
    // provider payload inside its own result envelope, so the records are never
    // where a naive reader expects them, and the shape differs per action. A
    // recursive walk needs no table of envelope names and cannot be defeated by
    // the next provider nesting one level deeper.
    let projected_any = project_in_place(&mut parsed);
    projected_any.then(|| serde_json::to_string(&parsed).ok())?
}

/// Project every array-of-records found anywhere in `value`, in place.
///
/// Returns whether anything was projected, so the caller can tell "nothing to
/// do" from "done" without comparing serialisations.
fn project_in_place(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(items) => {
            // The premise is many records sharing a shape; one object is better
            // shown as it came, and a list of scalars has nothing to prune.
            if items.len() >= 2 && items.iter().all(serde_json::Value::is_object) {
                for item in items.iter_mut() {
                    *item = project_record(item);
                }
                return true;
            }
            let mut any = false;
            for item in items.iter_mut() {
                any |= project_in_place(item);
            }
            any
        }
        serde_json::Value::Object(map) => {
            let mut any = false;
            for (_, val) in map.iter_mut() {
                any |= project_in_place(val);
            }
            any
        }
        _ => false,
    }
}

pub fn bound_body(body: String, what: &str) -> String {
    if body.len() <= MAX_BODY_BYTES {
        return body;
    }
    // Try to keep every record before resorting to keeping only the first few.
    let body = match project_records(&body) {
        Some(projected) if projected.len() < body.len() => {
            tracing::info!(
                what,
                from_bytes = body.len(),
                to_bytes = projected.len(),
                ratio = format!("{:.1}x", body.len() as f64 / projected.len().max(1) as f64),
                fits_now = projected.len() <= MAX_BODY_BYTES,
                "[composio] projected an array-of-records payload to its answering fields"
            );
            projected
        }
        _ => {
            tracing::info!(
                what,
                bytes = body.len(),
                head = body.chars().take(180).collect::<String>(),
                "[composio] no array-of-records to project; bounding the payload as it came"
            );
            body
        }
    };
    if body.len() <= MAX_BODY_BYTES {
        return body;
    }
    let mut end = MAX_BODY_BYTES;
    while end > 0 && !body.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = body.len() - end;
    let mut out = body[..end].to_string();
    out.push_str(&format!(
        "\n\n[TRUNCATED — the {what} was {dropped} bytes longer than this and the rest was NOT \
         shown. What you can see above is incomplete; do not treat it as the whole answer, and do \
         not repeat this call unchanged. Ask the action itself for less — a page size, a limit, a \
         date range or a filter argument — or request a specific record by id.]\n"
    ));
    out
}

/// How much detail one listing renders per action.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Detail {
    /// `SLUG — one-line description`. Cheap; the discovery view.
    #[default]
    Names,
    /// The full JSON parameter schema. Expensive; the call-it view.
    Schemas,
}

impl Detail {
    /// Parses the `detail` argument. Anything unrecognised (including a missing
    /// value) is [`Detail::Names`] — the cheap, complete view is the safe
    /// default, and a typo must never silently produce a 200-schema answer.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw
            .map(str::trim)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "schemas" | "schema" | "full" => Self::Schemas,
            _ => Self::Names,
        }
    }

    /// The default entry count for this mode.
    fn default_limit(self) -> usize {
        match self {
            Self::Names => NAMES_DEFAULT_LIMIT,
            Self::Schemas => SCHEMAS_DEFAULT_LIMIT,
        }
    }

    /// The ceiling an explicit `limit` is clamped to in this mode.
    fn max_limit(self) -> usize {
        match self {
            Self::Names => NAMES_MAX_LIMIT,
            Self::Schemas => SCHEMAS_MAX_LIMIT,
        }
    }
}

/// One callable Composio action, flattened out of the backend's function-schema
/// envelope so this module owns no upstream type (and so it compiles without
/// the `composio` feature).
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogAction {
    /// The action slug, e.g. `GITHUB_LIST_ISSUES` — what `composio_execute`
    /// takes.
    pub slug: String,
    /// The toolkit the slug belongs to, lowercased (`github`).
    pub toolkit: String,
    /// The human-readable description, or empty when the backend published
    /// none.
    pub description: String,
    /// The JSON schema of the action's input parameters, when published.
    pub parameters: Option<Value>,
}

/// A parsed `composio_list_tools` request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListRequest {
    /// Toolkit slugs the caller asked for (already intersected with the tenant
    /// allowlist by the caller).
    pub toolkits: Vec<String>,
    /// Lowercased search words. Every word must match the slug or the
    /// description.
    pub search: Vec<String>,
    /// Composio's own action tags, forwarded server-side.
    ///
    /// Carried through to `LiveClient::list_tools` rather than applied here:
    /// tags are a property of the catalogue, not of the text this module
    /// renders, so a tag filter applied client-side could only narrow what
    /// already survived the page budget — the same defect the server-side
    /// `search` forwarding fixed.
    pub tags: Vec<String>,
    /// Whether the listing that came back was **curated** — Composio's featured
    /// actions rather than the toolkit's whole catalogue.
    ///
    /// Set by the caller from what the client actually asked for, not derived
    /// here: only the BYOK route requests curation, and only when nothing
    /// narrows the call. Without it the header reports ~50 featured rows as the
    /// number "available", and an agent taking an inventory of GitHub is told
    /// that is everything (codex on tinyhumansai/opencompany#2153).
    pub curated: bool,
    /// How much per action to render.
    pub detail: Detail,
    /// Entry cap, already clamped to the mode's ceiling.
    pub limit: usize,
}

impl ListRequest {
    /// Parses the tool arguments. `toolkits` is *not* read here — the live tool
    /// intersects it with the tenant allowlist before the request is built, and
    /// that resolution is a security decision this module must not duplicate.
    pub fn parse(args: &Value, toolkits: Vec<String>) -> Self {
        let detail = Detail::parse(args.get("detail").and_then(Value::as_str));
        let search = args
            .get("search")
            .and_then(Value::as_str)
            .map(search_terms)
            .unwrap_or_default();
        let tags = args
            .get("tags")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).clamp(1, detail.max_limit()))
            .unwrap_or_else(|| detail.default_limit());
        Self {
            toolkits,
            search,
            tags,
            // The caller sets this from the client's own answer; parsing the
            // arguments cannot know which route served them.
            curated: false,
            detail,
            limit,
        }
    }

    /// Whether `action` survives this request's `search` words. Toolkit
    /// narrowing happens upstream (server-side, and again against the
    /// allowlist), so it is deliberately not re-applied here.
    fn matches(&self, action: &CatalogAction) -> bool {
        self.search.iter().all(|term| {
            action.slug.to_ascii_lowercase().contains(term)
                || action.description.to_ascii_lowercase().contains(term)
                || action.toolkit.contains(term)
        })
    }
}

/// Splits a raw `search` argument into lowercased words.
///
/// Underscores are separators too, so `GITHUB_LIST_ISSUES` pasted back in as a
/// search term still matches itself (the words `github`, `list` and `issues`
/// are all contained in the slug), which is how the agent asks for one specific
/// schema after reading the names view.
fn search_terms(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || c == '_' || c == ',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

/// The name of the tool this module renders for. Kept here so the rendered
/// guidance and the tool registration cannot drift apart.
pub const LIST_TOOLS_TOOL: &str = "composio_list_tools";

/// Renders the agent-facing listing for `actions` under `request`.
///
/// `actions` is the complete set the backend returned for the requested
/// toolkits, already filtered to the tenant allowlist. The rendering is
/// self-bounding: see the module docs for the invariant.
pub fn render(actions: &[CatalogAction], request: &ListRequest) -> String {
    let available = actions.len();
    let matched: Vec<&CatalogAction> = actions.iter().filter(|a| request.matches(a)).collect();

    if matched.is_empty() {
        return render_empty(available, request);
    }

    let (body, shown, last_shown, terse) = match request.detail {
        Detail::Schemas => {
            let (body, shown, last) = fill(&matched, request.limit, render_schema_block);
            (body, shown, last, false)
        }
        // Names mode prefers *completeness* over prose. Try slug + description
        // first; if that would drop entries, re-render slug-only, which is
        // roughly six times denser and usually fits the whole catalogue. "List
        // action names (cheap, complete), then fetch one schema on demand" is
        // the shape the issue asks for, and a complete list of slugs is what
        // makes the second step possible.
        Detail::Names => {
            let (body, shown, last) = fill(&matched, request.limit, render_name_line);
            if shown == matched.len() {
                (body, shown, last, false)
            } else {
                let (terse_body, terse_shown, terse_last) =
                    fill(&matched, request.limit, render_slug_line);
                if terse_shown > shown {
                    (terse_body, terse_shown, terse_last, true)
                } else {
                    (body, shown, last, false)
                }
            }
        }
    };

    let dropped = matched.len() - shown;
    let mut out = render_header(available, matched.len(), shown, request);
    if terse {
        out.push_str(
            "(Descriptions omitted so more of the list fits — ask for one action's description \
             and parameters with `detail: \"schemas\"`.)\n\n",
        );
    }
    out.push_str(&body);
    if dropped > 0 {
        out.push_str(&render_cut_notice(dropped, &last_shown, request));
    }
    out
}

/// Renders entries with `entry_of` until `limit` or [`MAX_RENDER_BYTES`] is
/// reached, never splitting one. Returns the body, how many were rendered, and
/// the slug the list was cut after.
///
/// The **first** entry always goes in whole, even if it alone blows the budget:
/// a schemas listing that matched exactly one huge schema must still hand it
/// over, and "0 of 1 shown" is a correctly-described way of being useless.
fn fill(
    matched: &[&CatalogAction],
    limit: usize,
    entry_of: fn(&CatalogAction) -> String,
) -> (String, usize, String) {
    let mut body = String::new();
    let mut shown = 0usize;
    let mut last_shown = String::new();
    for action in matched.iter().take(limit) {
        let entry = entry_of(action);
        if shown > 0 && body.len() + entry.len() > MAX_RENDER_BYTES {
            break;
        }
        body.push_str(&entry);
        last_shown = action.slug.clone();
        shown += 1;
    }
    (body, shown, last_shown)
}

/// The header every listing opens with: what was available, what matched, what
/// is being shown, and — always — how to get from here to a callable slug.
fn render_header(available: usize, matched: usize, shown: usize, request: &ListRequest) -> String {
    let scope = match request.toolkits.as_slice() {
        [] => "every connected toolkit".to_string(),
        toolkits => toolkits.join(", "),
    };
    let filter = if request.search.is_empty() {
        String::new()
    } else {
        format!(" matching `{}`", request.search.join(" "))
    };
    let mut header = if request.curated {
        // Not "available": these are Composio's featured actions, and the long
        // tail is reachable only by narrowing. Saying "available" here is what
        // told an agent that ~50 rows were all GitHub could do.
        format!(
            "Composio actions in {scope} — showing {shown} of {available} **featured** actions{filter}. \
             This is a curated preview, not the full catalogue: many more are callable but not \
             listed. Narrow with `search` or `tags` to reach them, e.g. \
             {LIST_TOOLS_TOOL}({{\"toolkits\": [\"<slug>\"], \"search\": \"issue\"}}).\n"
        )
    } else {
        format!(
            "Composio actions in {scope} — {available} available, {matched}{filter}, showing {shown}.\n"
        )
    };
    match request.detail {
        Detail::Names => header.push_str(&format!(
            "Each line is `SLUG — description`. Read one action's parameters before calling it:\n  \
             {LIST_TOOLS_TOOL}({{\"search\": \"<SLUG>\", \"detail\": \"schemas\"}})\n\n"
        )),
        Detail::Schemas => header.push_str(
            "Each block is one action's callable slug and its JSON input schema. Pass the slug \
             to `composio_execute` as `tool`.\n\n",
        ),
    }
    header
}

/// One `Detail::Names` line.
fn render_name_line(action: &CatalogAction) -> String {
    let description = action
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if description.is_empty() {
        format!("{}\n", action.slug)
    } else {
        format!(
            "{} — {}\n",
            action.slug,
            oh::util::truncate_with_suffix(&description, DESCRIPTION_PREVIEW_CHARS, "…")
        )
    }
}

/// One `Detail::Names` line with the description dropped — the dense fallback
/// that lets a whole large catalogue be listed completely.
fn render_slug_line(action: &CatalogAction) -> String {
    format!("{}\n", action.slug)
}

/// One `Detail::Schemas` block: the slug, the description, and the verbatim
/// input schema. Rendered as compact JSON — pretty-printing a Composio schema
/// triples its size for no gain to a model that reads it as text.
fn render_schema_block(action: &CatalogAction) -> String {
    let schema = action
        .parameters
        .clone()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
    let mut block = format!("## {}\n", action.slug);
    let description = action
        .description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if !description.is_empty() {
        block.push_str(&format!("{description}\n"));
    }
    block.push_str(&format!(
        "parameters: {}\n\n",
        serde_json::to_string(&schema).unwrap_or_else(|_| "{}".to_string())
    ));
    block
}

/// The notice a cut listing carries. Says it was cut, how much was dropped,
/// where the cut fell, and the exact arguments that make the next call smaller.
///
/// This is the whole point of the module: an agent that can read this has a
/// reason to change its request, so it never re-issues the identical call into
/// the repetition guard.
fn render_cut_notice(dropped: usize, last_shown: &str, request: &ListRequest) -> String {
    let max = request.detail.max_limit();
    let plural = if dropped == 1 { "" } else { "s" };
    format!(
        "\n[TRUNCATED — this listing is NOT complete. {dropped} more matching action{plural} were \
         not shown (the list was cut after `{last_shown}`). Do NOT repeat this call unchanged; it \
         will return the same partial list. Narrow it instead:\n  \
         • `search`: words matched against the action slug and description, e.g. \
         {LIST_TOOLS_TOOL}({{\"search\": \"list issues\"}})\n  \
         • `toolkits`: restrict to one toolkit, e.g. \
         {LIST_TOOLS_TOOL}({{\"toolkits\": [\"github\"]}})\n  \
         • `limit`: raise the cap (max {max}) if you truly need more at once.]\n"
    )
}

/// The answer when nothing matched. A completed listing that found nothing is a
/// fact, and saying so plainly — with the search that produced it — is what
/// stops the agent guessing a slug or retrying the same words.
fn render_empty(available: usize, request: &ListRequest) -> String {
    let scope = match request.toolkits.as_slice() {
        [] => "every connected toolkit".to_string(),
        toolkits => toolkits.join(", "),
    };
    // Whether the *server* was asked to narrow. This is the distinction that
    // matters now that `search` and `tags` travel to Composio: an empty
    // response to a narrowed query says nothing about the toolkit, only about
    // the filter, and `available` is the count of what came back rather than
    // what exists (codex on tinyhumansai/opencompany#2153).
    let filter = match (request.search.is_empty(), request.tags.is_empty()) {
        (true, true) => None,
        (false, true) => Some(format!("`{}`", request.search.join(" "))),
        (true, false) => Some(format!("tags `{}`", request.tags.join(", "))),
        (false, false) => Some(format!(
            "`{}` with tags `{}`",
            request.search.join(" "),
            request.tags.join(", ")
        )),
    };
    let Some(filter) = filter else {
        // Unnarrowed and empty is the only case that says anything about the
        // toolkit itself.
        return format!(
            "Composio actions in {scope} — none. This toolkit has no callable actions available to \
             this company (it may not be connected). Check `composio_list_connections`, and do not \
             guess an action slug.\n"
        );
    };
    // Narrowed and empty. Saying "no callable actions (it may not be
    // connected)" here would be a lie about a connected toolkit, and the
    // expensive kind: an agent told a capability does not exist stops looking
    // for it, which is the exact failure this listing was rewritten to end.
    let total = if available == 0 {
        "The filter was applied by Composio, so this is what it matched, not what the toolkit has."
            .to_string()
    } else {
        format!("{available} returned, 0 matching after filtering.")
    };
    format!(
        "Composio actions in {scope} — nothing matched {filter}. {total}\n\
         Try fewer or different words (the search matches the action slug and its description), \
         drop the tags, or list the toolkit unnarrowed with \
         {LIST_TOOLS_TOOL}({{\"toolkits\": [\"<slug>\"]}}). Do NOT guess a slug that was not \
         listed, and do not conclude the toolkit is unavailable from this result.\n"
    )
}

/// The `parameters_schema` JSON for `composio_list_tools`.
///
/// Lives here so the argument names the model is told about are the same
/// literals [`ListRequest::parse`] reads and [`render_cut_notice`] tells it to
/// use. A filter the model never discovers is the same bug as no filter at all.
pub fn list_tools_parameters_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "toolkits": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Toolkit slugs to list actions for (e.g. `gmail`, `slack`, `github`). Omit to search every connected toolkit."
            },
            "search": {
                "type": "string",
                "description": "Narrow the listing to actions whose slug or description contains ALL of these words (case-insensitive), e.g. `list issues` or `send email`. Pass an exact slug here with `detail: \"schemas\"` to read just that action's parameters."
            },
            "tags": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Composio action tags to narrow by, server-side (e.g. `important`). Combine with `search` to cut a large toolkit down before it is paged."
            },
            "detail": {
                "type": "string",
                "enum": ["names", "schemas"],
                "description": "`names` (default) lists `SLUG — description` lines: cheap, use it to discover the action you need. `schemas` returns the full JSON input schema for the matching actions: use it once you know the slug, before calling `composio_execute`."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "description": "Maximum actions to return (default 200 for `names`, 5 for `schemas`)."
            }
        },
        "additionalProperties": false
    })
}

// ---------------------------------------------------------------------------
// The toolkit listing
// ---------------------------------------------------------------------------
//
// Same class of bug, one level up (issue #410 point 4): `composio_list_toolkits`
// serialized the backend's whole catalogue — Composio publishes several hundred
// toolkits, each with a name, a description, a categories array and a logo URL —
// as pretty JSON with no filter and no bound. It is rarer to blow the budget
// than the action listing is, but it is the *same* silent cut, so it gets the
// same treatment rather than a note in a follow-up issue.

/// Tool name for the toolkit listing, for the same reason [`LIST_TOOLS_TOOL`]
/// is a constant.
pub const LIST_TOOLKITS_TOOL: &str = "composio_list_toolkits";

/// Default number of toolkits one listing renders.
pub const TOOLKITS_DEFAULT_LIMIT: usize = 200;

/// Ceiling an explicit `limit` is clamped to when listing toolkits.
pub const TOOLKITS_MAX_LIMIT: usize = 500;

/// One integration this company could use, flattened out of the backend's
/// catalogue entry. The logo URL is deliberately dropped: it is display
/// metadata for the console, and it costs an agent tokens to read a URL it can
/// never act on.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogToolkit {
    /// Toolkit slug, e.g. `googlecalendar` — what `composio_authorize` and the
    /// `toolkits` argument take.
    pub slug: String,
    /// Human-readable name, or empty when the backend published none.
    pub name: String,
    /// Short description, or empty.
    pub description: String,
    /// Whether this company is connected to it, when known.
    pub connected: Option<bool>,
}

/// A parsed `composio_list_toolkits` request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolkitListRequest {
    /// Lowercased search words matched against slug, name and description.
    pub search: Vec<String>,
    /// Entry cap, already clamped.
    pub limit: usize,
}

impl ToolkitListRequest {
    /// Parses the tool arguments, defaulting and clamping rather than failing.
    pub fn parse(args: &Value) -> Self {
        let search = args
            .get("search")
            .and_then(Value::as_str)
            .map(search_terms)
            .unwrap_or_default();
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| (n as usize).clamp(1, TOOLKITS_MAX_LIMIT))
            .unwrap_or(TOOLKITS_DEFAULT_LIMIT);
        Self { search, limit }
    }

    fn matches(&self, toolkit: &CatalogToolkit) -> bool {
        self.search.iter().all(|term| {
            toolkit.slug.to_ascii_lowercase().contains(term)
                || toolkit.name.to_ascii_lowercase().contains(term)
                || toolkit.description.to_ascii_lowercase().contains(term)
        })
    }
}

/// Renders the agent-facing toolkit listing, bounded and self-describing on the
/// same terms as [`render`].
pub fn render_toolkits(toolkits: &[CatalogToolkit], request: &ToolkitListRequest) -> String {
    let available = toolkits.len();
    let matched: Vec<&CatalogToolkit> = toolkits.iter().filter(|t| request.matches(t)).collect();

    if available == 0 {
        return "Composio toolkits — none available to this company. No integration can be used \
                or connected until the operator configures one.\n"
            .to_string();
    }
    if matched.is_empty() {
        return format!(
            "Composio toolkits — {available} available, 0 matching `{search}`.\n\
             Nothing matched those words. Try fewer or different words, or call \
             {LIST_TOOLKITS_TOOL}({{}}) to see everything. Do NOT guess a toolkit slug.\n",
            search = request.search.join(" ")
        );
    }

    let mut body = String::new();
    let mut shown = 0usize;
    let mut last_shown = "";
    for toolkit in matched.iter().take(request.limit) {
        let mut entry = toolkit.slug.clone();
        if !toolkit.name.is_empty() && !toolkit.name.eq_ignore_ascii_case(&toolkit.slug) {
            entry.push_str(&format!(" ({})", toolkit.name));
        }
        match toolkit.connected {
            Some(true) => entry.push_str(" [connected]"),
            Some(false) => entry.push_str(" [not connected]"),
            None => {}
        }
        let description = toolkit
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !description.is_empty() {
            entry.push_str(&format!(
                " — {}",
                oh::util::truncate_with_suffix(&description, DESCRIPTION_PREVIEW_CHARS, "…")
            ));
        }
        entry.push('\n');
        if shown > 0 && body.len() + entry.len() > MAX_RENDER_BYTES {
            break;
        }
        body.push_str(&entry);
        last_shown = &toolkit.slug;
        shown += 1;
    }

    let filter = if request.search.is_empty() {
        String::new()
    } else {
        format!(" matching `{}`", request.search.join(" "))
    };
    let mut out = format!(
        "Composio toolkits — {available} available, {matched}{filter}, showing {shown}.\n\
         Each line is `slug (name) — description`. List a toolkit's callable actions with \
         {LIST_TOOLS_TOOL}({{\"toolkits\": [\"<slug>\"]}}).\n\n",
        matched = matched.len()
    );
    out.push_str(&body);
    let dropped = matched.len() - shown;
    if dropped > 0 {
        let plural = if dropped == 1 { "" } else { "s" };
        out.push_str(&format!(
            "\n[TRUNCATED — this listing is NOT complete. {dropped} more matching toolkit{plural} \
             were not shown (the list was cut after `{last_shown}`). Do NOT repeat this call \
             unchanged; it will return the same partial list. Narrow it with \
             {LIST_TOOLKITS_TOOL}({{\"search\": \"<words>\"}}), or raise `limit` (max \
             {TOOLKITS_MAX_LIMIT}).]\n"
        ));
    }
    out
}

/// The `parameters_schema` JSON for `composio_list_toolkits`.
pub fn list_toolkits_parameters_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "search": {
                "type": "string",
                "description": "Narrow the listing to toolkits whose slug, name or description contains ALL of these words (case-insensitive), e.g. `calendar` or `issue tracker`."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "description": "Maximum toolkits to return (default 200)."
            }
        },
        "additionalProperties": false
    })
}

/// The `composio_list_toolkits` description shown to the model.
pub fn list_toolkits_description() -> &'static str {
    "List the Composio toolkits (integrations such as Gmail, Slack, GitHub, Notion) available to \
     this company, with whether each is connected. Pass `search` words to narrow a large \
     catalogue; a truncated result always says so and how to narrow it. Use \
     `composio_list_tools` next to see one toolkit's callable actions. Read-only."
}

/// The `composio_list_tools` description shown to the model.
pub fn list_tools_description() -> &'static str {
    "Discover the callable Composio actions (Gmail, Slack, GitHub, Notion, …) this company can \
     use. Two steps: call it with `search` words (and optionally `toolkits`) to list matching \
     `SLUG — description` lines, then call it again with `detail: \"schemas\"` for the one slug \
     you need to read its parameters before `composio_execute`. Large catalogues are truncated; \
     the result always says so and how to narrow it. Read-only."
}

/// The capability-grounding + Composio-first routing brief (issue #1759).
///
/// Wired into the agent turn system prompt by
/// [`build_agent`](crate::harness::build::build_agent) whenever the per-tenant
/// Composio tools are actually on the belt (an explicit `composio` grant AND a
/// resolved credential). It does two jobs the rest of the prompt did not:
///
/// * **Grounds the agent in what it can actually do.** The observed failure
///   (issue #1759) is an agent that reached for `http_request` against
///   `api.github.com`, got a 403, and then *promised* to "manually review it on
///   GitHub" — a browser action it has no tool for. A truthful line about the
///   surface it holds, and an explicit "do not promise what you have no tool
///   for", is what stops both halves.
/// * **Routes provider actions through the connected toolkit.** GitHub and every
///   connected SaaS is reachable ONLY through the company's Composio connection;
///   the raw web tools (`http_request`/`curl`/`web_fetch`) hit those APIs with no
///   credential and are refused. This is the agent-turn twin of the
///   workflow-node framing in
///   [`orchestrator`](crate::harness::built_in::orchestrator) ("for
///   Composio/GitHub use an agent node, not a `tool_call`"): the same rule, one
///   layer down.
///
/// Pure, and deliberately NOT behind the `composio` feature — the reason argued
/// in this module's header: that feature is built but never *run* by CI, so a
/// brief authored behind it would ship untested. `toolkits` is the company's
/// manifest allowlist
/// ([`TenantComposio::toolkits`](crate::harness::composio::TenantComposio)):
/// non-empty names exactly the connected toolkits, and empty is open mode, where
/// the agent is pointed at `composio_list_connections` to discover them rather
/// than promised a provider (GitHub, say) that may not be connected.
///
/// `native_caps` are the native capability namespaces the agent actually holds a
/// built-in tool for ([`native_capabilities_on_belt`](crate::harness::toolbelt::native_capabilities_on_belt)).
/// Non-empty adds a precedence line: use the built-in tool for those directly,
/// reserve Composio for connected third-party accounts with no built-in tool.
/// Empty renders no such line, leaving the brief byte-identical to a brief that
/// names only the Composio surface.
pub fn composio_brief(toolkits: &[String], native_caps: &[&str]) -> String {
    let named: Vec<String> = toolkits
        .iter()
        .map(|toolkit| toolkit.trim().to_ascii_lowercase())
        .filter(|toolkit| !toolkit.is_empty())
        .collect();

    // Only claim GitHub is reachable through Composio when it is actually named
    // among the connected toolkits. An empty allowlist is open mode — the
    // CLIENT applies no restriction, but that says nothing about what is
    // actually connected; the backend's own server-enforced allowlist still
    // decides (see `TenantComposio::toolkits`'s doc comment). Treating "no
    // client-side restriction" as "GitHub is connected" promised a capability
    // before `composio_list_connections` had confirmed it, the same failure a
    // non-empty allowlist that excludes GitHub (e.g. `["slack"]`) already
    // guards against — the live tools enforce the allowlist and would reject
    // the authorization/execution either way (PR #1780 review, rounds 6-7).
    let github_reachable = named.iter().any(|toolkit| toolkit == "github");
    // The capability *claim* above must not name GitHub before discovery, but
    // the "don't hand-roll a provider API" warning below is a guardrail, not a
    // claim — a concrete `api.github.com` example is what makes it land, and
    // withholding it in open mode weakened the warning for the case where the
    // agent is most likely to try exactly that (issue #1780 review). Keep the
    // example whenever GitHub is plausibly in play: named explicitly, or open
    // mode, where the backend allowlist may well include it.
    let github_example = github_reachable || named.is_empty();

    let mut brief = String::from(if github_reachable {
        "\n\n## Connected integrations (GitHub and other SaaS)\n\
         You reach GitHub and the company's other connected accounts through its Composio \
         integration, never by calling those services' web APIs yourself. Discover what is \
         available with `composio_list_toolkits` and `composio_list_connections`, find the action \
         you need with `composio_list_tools` (search words first, then `detail: \"schemas\"` on the \
         one slug), and run it with `composio_execute`; `composio_authorize` starts a connection \
         that is not set up yet.\n"
    } else {
        "\n\n## Connected integrations (Composio)\n\
         You reach the company's connected accounts through its Composio integration, never by \
         calling those services' web APIs yourself. Discover what is available with \
         `composio_list_toolkits` and `composio_list_connections`, find the action you need with \
         `composio_list_tools` (search words first, then `detail: \"schemas\"` on the one slug), \
         and run it with `composio_execute`; `composio_authorize` starts a connection that is not \
         set up yet.\n"
    });

    if named.is_empty() {
        brief.push_str(
            "Which toolkits this company has connected is not fixed here — call \
             `composio_list_connections` to see them before you rely on one.\n",
        );
    } else {
        brief.push_str(&format!(
            "Connected toolkits for this company: {}. Confirm their live state with \
             `composio_list_connections`.\n",
            named.join(", ")
        ));
    }

    if !native_caps.is_empty() {
        brief.push_str(&format!(
            "Some of what you might reach for you already hold as built-in tools of your own: {}. \
             Use those built-in tools directly for these — do not route them through Composio. \
             Composio is only for the company's connected third-party accounts that have no \
             built-in tool of your own.\n",
            native_caps.join(", ")
        ));
    }

    brief.push_str(if github_example {
        "Do NOT use `http_request`, `curl` or `web_fetch` against a connected provider's API (for \
         example `api.github.com`) — those tools call it with no credential and it answers 401 or \
         403; only the Composio tools carry the company's connection. And do not promise an action \
         before checking you hold a tool that can carry it out: unless you were separately granted \
         a browser tool, you have no browser and cannot open a page or \"review it on GitHub\" by \
         hand — either run the action through a Composio tool (or a browser tool you actually \
         hold) or say plainly that you cannot."
    } else {
        "Do NOT use `http_request`, `curl` or `web_fetch` against a connected provider's API — \
         those tools call it with no credential and it answers 401 or 403; only the Composio \
         tools carry the company's connection. And do not promise an action before checking you \
         hold a tool that can carry it out: unless you were separately granted a browser tool, \
         you have no browser and cannot open a page or complete it by hand — either run the \
         action through a Composio tool (or a browser tool you actually hold) or say plainly that \
         you cannot."
    });
    brief
}

// ---------------------------------------------------------------------------
// The http_request deflection guardrail (issue #1759, slice S2)
// ---------------------------------------------------------------------------
//
// S1 (`composio_brief`) TELLS the agent to route GitHub and other connected
// SaaS through Composio and not hand-roll HTTP. This is the enforcement twin:
// even when the agent ignores that brief, a raw `http_request` / `curl` /
// `web_fetch` aimed at a CONNECTED provider's API host is refused with a message
// that names the Composio route. It is defense-in-depth — the observed failure
// was a raw `http_request` to `api.github.com` that returned 403 (the web tools
// carry no connection credential), followed by the agent promising a browser
// action it has no tool for.
//
// Everything here is pure — a static slug→host table and a URL host check — so,
// like the rest of this module, it is NOT behind the `composio` feature and is
// exercised by the test lane that actually runs. The live wiring that feeds it
// the company's connected-toolkit set lives at the policy construction site.

/// The API host(s) a Composio toolkit fronts — the endpoints an agent would
/// reach by hand if it ignored the routing brief.
///
/// Deliberately small and provider-anchored: each arm is a toolkit slug
/// (lowercased, as [`composio_brief`] normalises them) and the API host(s) that
/// toolkit's Composio actions call. It does **not** try to enumerate every host
/// a provider owns — only the API hosts an agent plausibly curls for data, which
/// is what the observed failure did (`api.github.com`). A host not listed here
/// is never deflected, so the table erring small only ever means the S1 brief
/// still applies while the hard block does not — never a false deny.
///
/// Each entry pairs a host with a *set* of acceptable path prefixes (PR #1780
/// review, rounds 2-4): an empty set means the bare host is the whole API
/// surface, a non-empty set means the URL's path must start with at least one
/// of them. This is a set, not a single `Option<&str>`, because two rounds of
/// review (findings 7 and 8) landed on the same lesson from opposite
/// providers — a shared gateway's API surface for one product is not always
/// describable as a single path prefix, so the table needs to express "any of
/// these prefixes" once per host instead of gaining a second row (or
/// under-matching) every time a provider turns out to have more than one API
/// path family. Most providers front their API from a dedicated subdomain
/// (`api.github.com`, `*.googleapis.com`, …), where a bare host match — the
/// empty set — is exactly right. Slack and Discord are the exception: their
/// REST API is served from the SAME host as their public web product
/// (`slack.com/api/*`, `discord.com/api/*`), so matching the bare host would
/// also deflect a `web_fetch` of a public help page or invite link that needs
/// no connection and has no equivalent Composio action.
///
/// `www.googleapis.com` is the same shape one layer up: it is Google's shared
/// legacy API gateway for many products, not just Drive
/// (`www.googleapis.com/youtube/v3/...` is unrelated to Drive), so each
/// product's entry for it is scoped to that product's prefixes — the
/// dedicated `drive.googleapis.com` / `calendar.googleapis.com` /
/// `gmail.googleapis.com` hosts stay an unscoped match since they front
/// nothing else. Drive's REST surface on the shared gateway is not one
/// prefix: `/drive/v3/...` is the CRUD API, but the resumable/media upload
/// route an agent uploading a file actually hits is
/// `/upload/drive/v3/files` (finding 7) — a *sibling* prefix, not a deeper
/// path under `/drive/`, so it needs its own entry in the set rather than a
/// looser single prefix.
///
/// Jira is a third shape (PR #1780 review): `api.atlassian.com` is the
/// OAuth-3LO gateway (`/ex/jira/{cloudId}/...`), but the far more common path
/// an agent curls by hand is the tenant's own domain,
/// `https://<site>.atlassian.net/rest/api/3/...` — every Jira Cloud site has
/// one, and it is what the product surfaces as "your Jira URL". The host
/// itself is per-tenant, not fixed, so the table entry is the shared parent
/// `atlassian.net`: [`host_is`]'s suffix match catches any `<site>.` in front
/// of it. `/rest/api/` alone under-matched (finding 8): Jira Software's Agile
/// endpoints (boards, sprints) live under `/rest/agile/`, and Jira ships
/// other REST families the same way (`/rest/servicedeskapi/`,
/// `/rest/greenhopper/`, …) — the tenant host's REST surface is not one
/// family, it is everything under `/rest/`. The prefix is widened to
/// `/rest/` rather than enumerated family-by-family: the tenant host's *web*
/// UI (browsing issues, dashboards, `/jira/software/...`,
/// `/browse/...`) lives outside `/rest/` entirely, so `/rest/` is still the
/// exact API/UI boundary on this host, just drawn at its real location
/// instead of one family under it.
///
/// GitHub is the fourth shape (PR #1780 review, round 5): `api.github.com` is
/// the dedicated REST/GraphQL host, but release-asset upload/download traffic
/// (`POST/GET .../releases/{id}/assets`) is served from a SIBLING host,
/// `uploads.github.com` — not a sub-domain of `api.github.com`, so
/// [`host_is`]'s suffix match never caught it. Unlike Drive/Jira this is not a
/// second path prefix on the same host; it is a second, unrelated host that
/// fronts the same toolkit's API surface, so it gets its own entry in the
/// slice rather than a wider prefix set on the first one.
///
/// Stripe repeats the GitHub shape (PR #1780 review, round 6): `api.stripe.com`
/// is the general REST host, but file uploads
/// (`POST https://files.stripe.com/v1/files`, used for dispute evidence,
/// identity documents, etc.) are served from `files.stripe.com` — a sibling
/// host again, not a subdomain of `api.stripe.com`, so it needs its own entry
/// the same way `uploads.github.com` did.
///
/// Gmail repeats the Drive shape (PR #1780 review, round 7): sending or
/// importing a message through the legacy media/resumable upload route hits
/// `www.googleapis.com/upload/gmail/v1/users/me/messages/send` — a sibling
/// path prefix under the shared gateway host, not a deeper path under
/// `/gmail/`, exactly like Drive's `/upload/drive/` entry above. It needs its
/// own prefix in Gmail's set the same way.
///
/// Drive has a THIRD sibling prefix on the shared gateway (PR #1780 review,
/// round 8): batching several Drive calls into one HTTP request goes to
/// `www.googleapis.com/batch/drive/v3`, a product-scoped batch endpoint —
/// another sibling of `/drive/`, not a deeper path under it, same shape as
/// `/upload/drive/`. It needs its own prefix in Drive's set too.
///
/// Calendar has the same batch-endpoint gap as Drive (PR #1780 review, round
/// 9): `www.googleapis.com/batch/calendar/v3` is a sibling of `/calendar/`,
/// not a deeper path under it. Calendar's prefix set only had `/calendar/`,
/// so a batch request bypassed deflection the same way Drive's did before
/// round 8.
///
/// Gmail has the same batch-endpoint gap as Drive and Calendar (PR #1780
/// review, round 10): `www.googleapis.com/batch/gmail/v1` is a sibling of
/// `/gmail/`, not a deeper path under it — the same shape as the
/// `/upload/gmail/` sibling above. Gmail's prefix set only had `/gmail/` and
/// `/upload/gmail/`, so a batched Gmail request bypassed deflection the same
/// way Drive's and Calendar's batch requests did before rounds 8 and 9.
///
/// Dropbox was missing from this table entirely (PR #1780 review, round 11):
/// it is a first-class toolkit in the operator console's connection
/// catalogue (`frontend/src/lib/connections.ts`), but had no match arm here,
/// so it fell through to the `_ => &[]` default — no hosts, meaning no
/// deflection ever fired for a connected Dropbox toolkit. Like GitHub and
/// Stripe, Dropbox's API is split across two SIBLING hosts rather than one
/// host with path prefixes: `api.dropboxapi.com` for the RPC-style calls
/// (`/2/files/list_folder`, ...) and `content.dropboxapi.com` for the
/// content-transfer calls (`/2/files/upload`, `/2/files/download`), so both
/// get their own unscoped entry the same way `uploads.github.com` and
/// `files.stripe.com` do.
///
/// X (`twitter`) and LinkedIn had the same gap as Dropbox (PR #1780 review,
/// round 13): both are first-class toolkits in the connection catalogue but
/// had no match arm here. X ships two live host spellings — the legacy
/// `api.twitter.com` and the current `api.x.com` — so both are listed;
/// LinkedIn has a single dedicated REST host, `api.linkedin.com`.
fn toolkit_api_hosts(toolkit: &str) -> &'static [(&'static str, &'static [&'static str])] {
    match toolkit {
        "github" => &[("api.github.com", &[]), ("uploads.github.com", &[])],
        "gmail" => &[
            ("gmail.googleapis.com", &[]),
            (
                "www.googleapis.com",
                &["/gmail/", "/upload/gmail/", "/batch/gmail/"],
            ),
        ],
        "googlecalendar" => &[
            ("calendar.googleapis.com", &[]),
            ("www.googleapis.com", &["/calendar/", "/batch/calendar/"]),
        ],
        "googledrive" => &[
            ("drive.googleapis.com", &[]),
            (
                "www.googleapis.com",
                &["/drive/", "/upload/drive/", "/batch/drive/"],
            ),
        ],
        "slack" => &[("slack.com", &["/api/"])],
        "notion" => &[("api.notion.com", &[])],
        "linear" => &[("api.linear.app", &[])],
        "hubspot" => &[("api.hubapi.com", &[])],
        "stripe" => &[("api.stripe.com", &[]), ("files.stripe.com", &[])],
        "jira" => &[
            ("api.atlassian.com", &["/ex/jira/"]),
            ("atlassian.net", &["/rest/"]),
        ],
        "discord" => &[("discord.com", &["/api/"]), ("discordapp.com", &["/api/"])],
        "dropbox" => &[("api.dropboxapi.com", &[]), ("content.dropboxapi.com", &[])],
        "twitter" => &[("api.twitter.com", &[]), ("api.x.com", &[])],
        "linkedin" => &[("api.linkedin.com", &[])],
        _ => &[],
    }
}

/// Whether `host` is `api_host` or a subdomain of it, case-insensitively.
///
/// Sub-domain matching (not just equality) so `uploads.api.github.com` is caught
/// alongside `api.github.com`, while an unrelated host that merely *ends with*
/// the same letters (`notapi.github.com.evil.test`) is not — the boundary dot is
/// required.
///
/// `host` first has a single trailing dot stripped (PR #1780 review, round
/// 12): `https://api.github.com./repos/o/r` is a valid absolute-FQDN
/// spelling — the DNS root label — and resolves to the exact same host, but
/// [`url::Url::host_str`] keeps that dot verbatim, so without stripping it
/// neither the equality nor the subdomain-suffix arm below would match a
/// request spelled that way. `api_host` is always one of this module's own
/// literal constants and never carries a trailing dot, so only `host` needs
/// normalizing.
fn host_is(host: &str, api_host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    host == api_host || host.ends_with(&format!(".{api_host}"))
}

/// The S2 decision: given the company's CONNECTED Composio toolkits and the URL
/// a raw web tool (`http_request` / `curl` / `web_fetch`) is about to call,
/// return `Some(reason)` — the operator-facing deny message naming the Composio
/// route — when the URL's host (and, where the table requires it, path) belongs
/// to a connected toolkit's API, or `None` when the call must pass through
/// unchanged.
///
/// `None` (pass through) covers the four cases the guardrail must NOT block: a
/// non-provider host, a provider host whose toolkit is **not** in `connected`
/// (the company may legitimately hit a public endpoint of a provider it has not
/// wired), a provider host outside every API path prefix the table requires for
/// it (a public Slack/Discord page, not the Web API), and a URL that does not
/// parse to a host. The deny is scoped strictly to the API surface of toolkits
/// this company actually connected, per requirement #2.
pub fn web_call_deflection(connected: &[String], url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_ascii_lowercase();
    let path = parsed.path();
    for toolkit in connected {
        let toolkit = toolkit.trim().to_ascii_lowercase();
        if toolkit.is_empty() {
            continue;
        }
        if toolkit_api_hosts(&toolkit)
            .iter()
            .any(|(api_host, prefixes)| {
                host_is(&host, api_host)
                    && (prefixes.is_empty() || prefixes.iter().any(|p| path.starts_with(p)))
            })
        {
            return Some(web_deflection_message(&toolkit, &host));
        }
    }
    None
}

/// The refusal a deflected web call carries. Mirrors the S1 routing sentence:
/// the raw web tools reach the provider with no credential and get 401/403, and
/// the Composio two-step (`composio_list_tools` → `composio_execute`) is the
/// door that carries the company's connection.
fn web_deflection_message(toolkit: &str, host: &str) -> String {
    format!(
        "Blocked: `{host}` is the `{toolkit}` provider's API, and `{toolkit}` is connected to this \
         company through Composio. `http_request`, `curl` and `web_fetch` call it with no \
         credential and get 401/403 — only the Composio tools carry the company's connection. Use \
         them instead: find the action with `composio_list_tools` (search words, then \
         `detail: \"schemas\"` on the one slug) and run it with `composio_execute`."
    )
}

#[cfg(test)]
#[path = "composio_catalog_projection_prototype_tests.rs"]
mod projection_prototype_tests;
#[cfg(test)]
#[path = "composio_catalog_tests.rs"]
mod tests;
