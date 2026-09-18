use super::*;

fn chunk(label: &str, body: &str) -> RawChunk {
    chunk_at(label, body, 0)
}

fn chunk_at(label: &str, body: &str, stored_at_millis: u64) -> RawChunk {
    RawChunk {
        addr: format!("addr-{label}"),
        label: label.to_string(),
        body: body.to_string(),
        stored_at_millis,
    }
}

/// A chunk whose address is set explicitly, for the stamp-tie tie-break
/// (`chunk_at` derives the addr from the label, so two chunks with
/// different labels can never share one).
fn chunk_at_addr(addr: &str, label: &str, body: &str, stored_at_millis: u64) -> RawChunk {
    RawChunk {
        addr: addr.to_string(),
        label: label.to_string(),
        body: body.to_string(),
        stored_at_millis,
    }
}

fn fact(id: &str, kind: FactKind, title: &str) -> FactRecord {
    FactRecord {
        id: id.to_string(),
        kind,
        title: title.to_string(),
        body: format!("{title} body"),
        source: "You".to_string(),
        updated_at_millis: 100,
    }
}

#[test]
fn zero_facts_plus_context_yields_readonly_nonempty_list() {
    // The reported bug: header counts N context chunks but the list is empty
    // because it only read the (empty) FactStore. Prove context now surfaces.
    let chunks = vec![
        chunk("agent-1/notes", "Learned a thing\nmore detail"),
        chunk("task-outcome/agent-1", "Task: ship it\nOutcome: done"),
    ];
    let entries = context_entries(chunks, None);

    assert_eq!(entries.len(), 2, "both context chunks surface as entries");
    assert!(
        entries.iter().all(|e| !e.editable),
        "context rows are read-only"
    );
    assert!(
        entries.iter().all(|e| e.kind.is_none()),
        "context rows carry no fact kind"
    );
    // Ordering: agent memory before task outcomes.
    assert!(matches!(entries[0].origin, MemoryOrigin::AgentMemory));
    assert!(matches!(entries[1].origin, MemoryOrigin::TaskOutcome));
    assert_eq!(entries[1].source, "agent-1", "outcome source = agent id");
    // Title/body split off the first line.
    assert_eq!(entries[0].title, "Learned a thing");
    assert_eq!(entries[0].body, "more detail");
}

/// The #1290 review's M2 red-proof, now green: a deliberate memory's
/// `agent-memory/<agent>/<slug>` label attributes to the AGENT, not to
/// the literal first segment "agent-memory".
#[test]
fn deliberate_memories_are_attributed_to_the_storing_agent() {
    let chunks = vec![chunk(
        "agent-memory/ceo/fiscal-year",
        "Fiscal year\n\nFebruary",
    )];
    let entries = context_entries(chunks, None);
    assert_eq!(entries.len(), 1);
    assert!(matches!(entries[0].origin, MemoryOrigin::AgentMemory));
    assert_eq!(
        entries[0].source, "ceo",
        "the Brain view must name the agent that stored the memory"
    );
    assert_eq!(entries[0].title, "Fiscal year");
}

#[test]
fn operator_fact_mirror_is_not_double_listed() {
    // The `operator-fact/{id}` chunk mirrors a FactStore row; it must NOT
    // appear as a second read-only row duplicating that fact.
    let chunks = vec![
        chunk("operator-fact/fact-123", "Client prefers Friday\nreviews"),
        chunk("agent-2/x", "genuine agent memory"),
    ];
    let entries = context_entries(chunks, None);

    assert_eq!(
        entries.len(),
        1,
        "mirror dropped; only agent memory remains"
    );
    assert!(matches!(entries[0].origin, MemoryOrigin::AgentMemory));
}

#[test]
fn facts_editable_context_readonly() {
    // Facts get the delete affordance; read-only rows never do.
    let fact_entry = MemoryEntry::from(fact("f1", FactKind::Person, "Ada"));
    assert!(fact_entry.editable, "operator facts are deletable");
    assert!(matches!(fact_entry.origin, MemoryOrigin::Fact));
    assert_eq!(fact_entry.kind, Some(FactKind::Person));

    let ctx = context_entries(vec![chunk("task-outcome/a", "Task: t\nOutcome: o")], None);
    assert!(
        !ctx[0].editable,
        "read-only rows expose no edit/delete affordance"
    );
}

#[test]
fn context_rows_carry_their_stored_at_stamp() {
    // Context rows used to hardcode `updated_at: 0`, so every agent-written
    // memory rendered "—" no matter how recently it landed.
    let entries = context_entries(
        vec![
            chunk_at("agent-1/notes", "recent", 2_000),
            chunk_at("task-outcome/agent-1", "Task: t\nOutcome: o", 3_000),
        ],
        None,
    );
    // Newest first, so the fresher task outcome heads the pair.
    assert_eq!(entries[0].updated_at, 3_000);
    assert_eq!(entries[1].updated_at, 2_000);
}

/// The route used to bucket rows by origin and concatenate — agent
/// memories, then task outcomes — which pushed EVERY outcome behind EVERY
/// memory whatever their stamps. A company whose newest memory is a task
/// outcome saw it render last: the #1488 symptom by a second route.
#[test]
fn context_rows_interleave_the_two_origins_by_stamp() {
    let entries = context_entries(
        vec![
            chunk_at("task-outcome/agent-1", "newest outcome", 900),
            chunk_at("agent-memory/agent-1/five", "middle memory", 500),
            chunk_at("agent-memory/agent-1/one", "oldest memory", 100),
        ],
        None,
    );
    let stamps: Vec<u64> = entries.iter().map(|e| e.updated_at).collect();
    assert_eq!(
        stamps,
        vec![900, 500, 100],
        "context rows order by stamp across origins, not by origin"
    );
    assert!(matches!(entries[0].origin, MemoryOrigin::TaskOutcome));
}

#[test]
fn context_rows_break_stamp_ties_by_addr_like_the_cap() {
    // Same tie-break as `capped_newest_first`, so chunks sharing a
    // millisecond cannot swap places between two calls.
    let entries = context_entries(
        vec![
            chunk_at_addr("z", "agent-1/z", "zulu", 500),
            chunk_at_addr("a", "task-outcome/agent-1", "alpha", 500),
        ],
        None,
    );
    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["ctx:a:task-outcome/agent-1", "ctx:z:agent-1/z"],
        "the id carries addr then label, so the tie-break stays addr-first"
    );
}

/// One address, two labels — the #1300 shape — renders as two rows with
/// DISTINCT ids. Keyed by address alone they collided, and the console
/// renders these rows by id (React keys, row identity).
#[test]
fn two_labels_on_one_address_render_as_two_distinct_rows() {
    let entries = context_entries(
        vec![
            chunk_at_addr("shared", "agent-memory/ann/note", "same text", 500),
            chunk_at_addr("shared", "agent-memory/bob/note", "same text", 500),
        ],
        None,
    );
    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "ctx:shared:agent-memory/ann/note",
            "ctx:shared:agent-memory/bob/note"
        ],
    );
    let sources: Vec<&str> = entries.iter().map(|e| e.source.as_str()).collect();
    assert_eq!(sources, vec!["ann", "bob"], "each row keeps its own author");
}

#[test]
fn unstamped_context_rows_stay_dashed() {
    // A chunk written before backends stamped has no store time; reporting
    // `0` keeps the console's "—" rather than inventing the epoch.
    let entries = context_entries(vec![chunk("agent-1/notes", "legacy")], None);
    assert_eq!(entries[0].updated_at, 0);
}

#[test]
fn query_matches_across_context_rows() {
    let chunks = vec![
        chunk("agent-1/a", "alpha content"),
        chunk("agent-1/b", "beta content"),
    ];
    // Case-insensitive substring, same as fact search.
    let entries = context_entries(chunks, Some("BETA"));
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].title, "beta content");
}

#[test]
fn json_shape_facts_stable_context_omits_kind() {
    // Fact serialization is unchanged (kind present); context omits kind and
    // carries the read-only discriminator the console keys off.
    let fact_json =
        serde_json::to_value(MemoryEntry::from(fact("f", FactKind::Fact, "t"))).unwrap();
    assert_eq!(fact_json["kind"], "fact");
    assert_eq!(fact_json["origin"], "fact");
    assert_eq!(fact_json["editable"], true);

    let ctx = &context_entries(vec![chunk("agent-1/a", "hello world")], None)[0];
    let ctx_json = serde_json::to_value(ctx).unwrap();
    assert!(ctx_json.get("kind").is_none(), "context row omits kind");
    assert_eq!(ctx_json["origin"], "agent-memory");
    assert_eq!(ctx_json["editable"], false);
}

#[test]
fn blank_body_falls_back_to_origin_label() {
    // A whitespace-only chunk body has no title line, so the row shows the
    // origin label rather than an empty heading.
    let ctx = context_entries(vec![chunk("task-outcome/a", "   ")], None);
    assert_eq!(ctx[0].title, "Task outcome");
    assert_eq!(ctx[0].body, "");
}

fn meta(addr: &str, label: &str, stored_at_millis: u64) -> ChunkMeta {
    ChunkMeta {
        addr: ChunkAddr::new(addr),
        label: label.to_string(),
        len: 0,
        stored_at_millis,
    }
}

/// #1488: backends list oldest-first, so capping before sorting pinned the
/// Brain view to the oldest chunks forever — the cap must keep the newest.
#[test]
fn the_cap_keeps_the_newest_chunks_not_the_oldest() {
    let metas = vec![
        meta("a1", "agent-1/one", 100),
        meta("a2", "agent-1/two", 200),
        meta("a3", "agent-1/three", 300),
        meta("a4", "agent-1/four", 400),
    ];
    let capped = capped_newest_first(metas, "operator-fact/", 2);
    let stamps: Vec<u64> = capped.iter().map(|m| m.stored_at_millis).collect();
    assert_eq!(
        stamps,
        vec![400, 300],
        "the newest chunks survive the cap, newest first; the oldest fall off"
    );
}

#[test]
fn the_cap_drops_mirrors_first_and_breaks_stamp_ties_by_addr() {
    // The mirror must not consume a cap slot even as the newest chunk, and
    // equal stamps must order deterministically (by addr) between calls.
    let metas = vec![
        meta("b", "agent-1/two", 500),
        meta("m", "operator-fact/f1", 900),
        meta("a", "agent-1/one", 500),
    ];
    let capped = capped_newest_first(metas, "operator-fact/", 2);
    let addrs: Vec<&str> = capped.iter().map(|m| m.addr.as_ref()).collect();
    assert_eq!(addrs, vec!["a", "b"]);
}
