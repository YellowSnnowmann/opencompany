//! Which workspace documents each role is told to reason from.
//!
//! Implements `docs/spec/runtime/orchestration/context-routing.md`. The rule it
//! exists to enforce:
//!
//! > **Context is authority.** A document routed into a role's system prompt is
//! > something that role is being told to reason from. Route it deliberately,
//! > and record why each exclusion is an exclusion.
//!
//! Three pieces live here, all compiled and tested in every build even though
//! the harness that spends the result is behind the `openhuman` feature — the
//! exclusions are controls, and a control deserves tests wherever it is defined:
//!
//! * [`routed_documents`] — the per-tier default table, and how an explicit
//!   `context` key overrides it.
//! * [`excluded_documents`] — the class-based exclusions, which are subtractive
//!   and apply to defaults and explicit lists alike.
//! * [`resolve_routed_documents`] — reads the selected notes out of a
//!   [`WorkspaceStore`](crate::ports::WorkspaceStore), skipping any that do not
//!   exist.
//!
//! The harness resolves these ahead of the (synchronous) agent build — see
//! `Harness::resolve_routed_context` — and appends them to the persona last, via
//! [`crate::company::prompt::context_section`].

use crate::company::Agent;

/// The one document every role is routed, whatever its tier or classes.
///
/// The company's method policy: how this company works, as distinct from what it
/// currently believes. Universal because a role that does not know the method
/// cannot follow it, and unlike every other document here it asserts nothing
/// about the work in progress, so no exclusion can apply to it.
pub const UNIVERSAL_DOCUMENT: &str = "method.md";

/// The company's per-workspace working agreement, routed to every role
/// alongside [`UNIVERSAL_DOCUMENT`].
///
/// Distinct from `method.md`: `method.md` is the company's method policy,
/// authored per company; `AGENTS.md` is the bundle-level agreement every
/// teammate in the roster shares — which files exist and what they are for,
/// how work is expected to be handed off, conventions a person and every
/// agent are both bound by. Also asserts nothing about work in progress, so it
/// is exempt from class exclusions the same way `method.md` is.
pub const AGENTS_DOC: &str = "agents.md";

/// Every document routed to every role, whatever its tier, classes, or
/// explicit `context` list — in the order they are placed in the prompt.
pub const UNIVERSAL_DOCUMENTS: &[&str] = &[UNIVERSAL_DOCUMENT, AGENTS_DOC];

/// The company's summarized picture: what is established, what is ruled out.
pub const BRIEF: &str = "brief.md";
/// The evidence ledger — what already holds true, with its derivation.
pub const CLAIMS: &str = "claims.md";
/// The open-question tracker the orchestrator routes work from.
pub const THREADS: &str = "threads.md";
/// The assertion board: posts are asserted, not established.
pub const BOARD: &str = "board.md";
/// Provisional working-out, kept out of any role that judges.
pub const SCRATCH: &str = "scratch.md";

/// The documents a role is routed when its manifest declares no `context` key.
///
/// Keyed on the role's tier, per the spec's default table. Every row is a
/// default, never a floor or a ceiling: an explicit `context` — including an
/// empty one — always wins for that role.
///
/// **An agent with no `tier` takes the `reasoning` row.** `tier` is optional and
/// most roster entries omit it, so the table needs a defined fallback or it
/// covers almost nobody. `reasoning` is right because it is what an undeclared
/// teammate *is*: a worker doing the substantive job its description names.
/// Defaulting to `orchestrator` would hand every unlabelled agent the routing
/// picture, and defaulting to none would leave the ordinary case with no working
/// context at all.
fn tier_defaults(tier: Option<&str>) -> &'static [&'static str] {
    match tier {
        // Decides what happens next across the whole company, so it needs the
        // established picture plus both derived ledgers — without them it would
        // re-derive routing decisions from raw notes every cycle.
        Some("orchestrator") => &[BRIEF, CLAIMS, THREADS],
        // Talks to the operator or another company: needs the summarized picture
        // to speak from, not the derivation detail behind it.
        Some("frontend") => &[BRIEF],
        // Reads and summarizes raw workspace notes to *write* the brief. Routing
        // it the brief would be circular.
        Some("compress") => &[],
        // Runs over compressed history between cycles, not the live workspace: a
        // routed document would be stale by construction before the tick that
        // read it ran.
        Some("subconscious") => &[],
        // `reasoning`, and every agent that declares no tier: does the
        // substantive work a demand asks for, so it needs what is established
        // and what already holds — but not the open-question tracker, which is
        // the orchestrator's routing concern.
        _ => &[BRIEF, CLAIMS],
    }
}

/// The documents a role's classes forbid, whatever the routing table says.
///
/// Each rule prevents a specific observed failure, and each is subtractive: an
/// exclusion outranks both the tier default and an explicit `context` list,
/// because the point of declaring a class is that the exclusion cannot be lost
/// by someone editing a routing line.
///
/// * A role that **weighs evidence** must not be routed the assertion board. A
///   post is asserted, not established; a critic scoring a deliverable beside an
///   unevidenced sentence is one prompt away from scoring the sentence.
/// * A role that **judges** must not be routed the scratch. Provisional
///   working-out read as progress is what keeps a loop retrying.
/// * A role **acting on an operator directive** must not be routed the claim
///   ledger. A directive is asserted, and a role holding the evidence ledger
///   while carrying out an instruction is one prompt away from filing the
///   instruction as a finding.
pub fn excluded_documents(classes: &[String]) -> Vec<&'static str> {
    let mut excluded = Vec::new();
    for class in classes {
        match class.as_str() {
            "evidence" => excluded.push(BOARD),
            "judge" => excluded.push(SCRATCH),
            "directive" => excluded.push(CLAIMS),
            _ => {}
        }
    }
    excluded
}

/// The workspace documents to route into `agent`'s system prompt.
///
/// Resolution order:
///
/// 1. [`UNIVERSAL_DOCUMENTS`], always;
/// 2. the agent's explicit `context` list if it declared one, else its tier's
///    default row;
/// 3. minus anything its classes exclude.
///
/// `Some(vec![])` (an explicit `context = []`) and `None` (an omitted key) are
/// deliberately different: the first means "the universal documents and
/// nothing else", the second means "take the default". `Agent::context` is
/// `Option<Vec<ContextEntry>>` precisely so that distinction is representable.
///
/// Returned in routing order with duplicates removed, so a manifest that lists
/// a universal document explicitly does not get it twice.
pub fn routed_documents(agent: &Agent) -> Vec<String> {
    let excluded = excluded_documents(&agent.classes);

    let chosen: Vec<String> = match agent.context.as_deref() {
        Some(explicit) => explicit
            .iter()
            .map(|entry| entry.path().to_string())
            .collect(),
        None => tier_defaults(agent.tier.as_deref())
            .iter()
            .map(|doc| doc.to_string())
            .collect(),
    };

    let mut routed = Vec::with_capacity(chosen.len() + UNIVERSAL_DOCUMENTS.len());
    let mut seen = std::collections::HashSet::new();
    let universal = UNIVERSAL_DOCUMENTS.iter().map(|doc| doc.to_string());
    for document in universal.chain(chosen) {
        let document = document.trim().to_string();
        if document.is_empty() {
            continue;
        }
        // The universal documents are exempt from exclusion: neither asserts
        // anything about the work in progress, so no class has a reason to
        // withhold either — and a role excluded from the method or the
        // working agreement could not follow it.
        if !UNIVERSAL_DOCUMENTS.contains(&document.as_str())
            && excluded.contains(&document.as_str())
        {
            continue;
        }
        if seen.insert(document.clone()) {
            routed.push(document);
        }
    }
    routed
}

/// Reads the documents [`routed_documents`] selected for `agent` out of the
/// company's workspace tree.
///
/// Returns `(path, body)` pairs in routing order, ready for
/// [`context_section`](crate::company::prompt::context_section).
///
/// **A named document that does not exist is skipped, not an error.** These are
/// live operator-owned notes: a company that has not written its brief yet is
/// not a misconfigured company, and failing the roster build over one would take
/// the whole company down for a missing file anybody could create. This is the
/// opposite rule to
/// [`prompt_files`](crate::company::Agent::prompt_files), which names a file in
/// the same commit as the agent referencing it — see `runtime/agents.md` for why
/// the two differ.
///
/// Async and always compiled, so it is exercised by the default-build test
/// suite against a real store rather than only where the agent runtime links.
/// The harness calls this before building the (synchronous) agent, the same way
/// it resolves skill deltas ahead of time.
pub async fn resolve_routed_documents(
    workspace: &dyn crate::ports::WorkspaceStore,
    company: &crate::ports::types::CompanyId,
    agent: &Agent,
) -> crate::Result<Vec<(String, String)>> {
    let wanted = routed_documents(agent);
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    // One tree read for the whole roster's worth of lookups, rather than a read
    // per document: the tree is the only way to resolve a logical path to a node
    // id, and a per-document walk would multiply that cost by the routing table.
    let nodes = workspace.tree(company).await?;
    let by_id: std::collections::HashMap<&str, &crate::ports::workspace::WorkspaceNode> =
        nodes.iter().map(|node| (node.id.as_str(), node)).collect();

    let mut by_path: std::collections::HashMap<String, &str> =
        std::collections::HashMap::with_capacity(nodes.len());
    for node in &nodes {
        if node.kind != crate::ports::workspace::NodeKind::File {
            continue;
        }
        if let Some(path) = super::workspace_paths::render_path(node, &by_id) {
            // Keyed by the normalized path as well as the literal one, because
            // the two spellings both occur in a real company: the routing names
            // above are lowercase-dashed like everything else the runtime mints
            // (`crate::company::workspace_names`), while a company that predates
            // that rule holds `brief.md`, and a manifest written then still says
            // `brief.md` in its `context` list. Routing a role its brief must
            // not depend on which of those it is looking at.
            //
            // The literal insert wins a collision: `Brief.md` and `brief.md` in
            // one tree resolve to themselves, and only the *unmatched* spelling
            // falls through to the normalized key.
            let canonical = super::workspace_names::kebab_path(&path);
            if canonical != path {
                by_path.entry(canonical).or_insert(node.id.as_str());
            }
            by_path.insert(path, node.id.as_str());
        }
    }

    let mut resolved = Vec::with_capacity(wanted.len());
    for path in wanted {
        // Normalise the manifest's spelling the same way the agent tools do, so
        // `/brand/Voice.md` and `brand/Voice.md` name the same note.
        let key = match super::workspace_paths::split_logical_path(&path) {
            Ok(segments) => segments.join("/"),
            // A traversal-shaped or malformed entry resolves to nothing, exactly
            // like a missing document. Refusing the boot would let one bad
            // manifest line stop a company whose other routing is fine.
            Err(_) => continue,
        };
        let id = by_path
            .get(&key)
            .or_else(|| by_path.get(&super::workspace_names::kebab_path(&key)));
        let Some(id) = id else {
            continue;
        };
        if let Some((_, body)) = workspace.read(company, id).await? {
            resolved.push((key, body));
        }
    }
    Ok(resolved)
}

#[cfg(test)]
#[path = "context_routing_tests.rs"]
mod tests;
