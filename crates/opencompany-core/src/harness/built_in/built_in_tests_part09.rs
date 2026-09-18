//! `built_in`'s own inline tests, part 9 of 10. Split out of the
//! single inline `mod tests` block because it exceeded the 750-line file
//! limit; grouped in the original file's order, not by topic (the block
//! covered dozens of unrelated issues with no existing topical boundaries).
//! Shared setup lives in [`super::built_in_test_fixtures`] and
//! [`super::built_in_test_fixtures_2`].

use super::built_in_test_fixtures::*;
use super::built_in_test_fixtures_2::*;
use super::*;

/// **The no-restart proof for the one-click grant.** A namespace granted
/// through the company store — the exact path `PUT …/tools/grants` writes
/// through — moves the grant fingerprint, so `ensure` rebuilds the roster in
/// place and the belt the next turn runs with actually has the tools.
///
/// Without this axis every other fingerprint stays stable across a grant,
/// the fast path returns the cached roster, and the operator watches the
/// connect page flip to "Connected" while no teammate receives anything
/// until the process restarts — which is the same "Connected and reaching
/// nobody" the grant was clicked to end, with a delay attached.
///
/// One pool throughout, never reconstructed (`resident_companies()` stays
/// 1), so nothing here can be smuggling in a restart.
#[tokio::test]
async fn a_tool_grant_written_through_the_store_rebuilds_the_roster_in_place() {
    use crate::ports::types::{Actor, ActorKind, ToolGrantsOverride};

    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(MockContext::default());
    // A catch-all company: `*` covers shell/code/web and confers none of the
    // five namespaces this route deals in, which is the manifest shape the
    // issue was reported against.
    let mut rec = capped_record();
    rec.manifest.tools.allow = vec!["*".to_string()];

    let live_store = Arc::new(LiveStore::default());
    live_store.save(&rec).await.unwrap();
    let mut deps = deps_with_plan(dir.path(), context.clone(), None, None);
    deps.store = live_store.clone();

    let pool = HarnessPool::new();
    pool.ensure(&rec, &deps).await.expect("ensure before");
    let before = pool
        .grants_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");

    // An admin grants `chargebee` from the connect page.
    let mut granted = rec.clone();
    granted.overlay_tool_grants = Some(ToolGrantsOverride {
        added: vec!["chargebee".to_string()],
        set_by: Actor {
            kind: ActorKind::User,
            id: "user-admin".to_string(),
        },
        at_millis: crate::ports::now_millis(),
    });
    granted.manifest.tools.allow = granted.effective_tool_allow();
    live_store.save(&granted).await.unwrap();

    // Deliberately re-`ensure` with the STALE record the caller is holding.
    // A boot-time snapshot is what `HarnessBrain::record` hands in, so the
    // grant must be picked up from the live store read rather than from the
    // record passed in — otherwise this works only for callers that happen
    // to have reloaded.
    pool.ensure(&rec, &deps).await.expect("ensure after");
    let after = pool
        .grants_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(
        before, after,
        "granting a namespace must move the grant fingerprint, or the cached \
         roster is reused and no teammate ever receives the tools"
    );
    assert_eq!(
        pool.resident_companies().await,
        1,
        "the same company, rebuilt in place — not a new process"
    );

    // Withdrawing it returns the fingerprint to where it started: the axis
    // tracks the effective list, so a revoked grant is as visible as a
    // granted one.
    pool.ensure(&rec, &deps).await.expect("ensure idempotent");
    assert_eq!(
        pool.grants_fingerprint_of(&rec.id).await,
        Some(after),
        "an unchanged grant set must not churn the roster"
    );
    live_store.save(&rec).await.unwrap();
    pool.ensure(&rec, &deps).await.expect("ensure cleared");
    assert_eq!(
        pool.grants_fingerprint_of(&rec.id).await,
        Some(before),
        "withdrawing the grant must move the fingerprint back"
    );
}

/// **The search-backend counterpart of the proof above.** `grants_fp`
/// (and the roster's own effective-allow-list read) already tolerate a
/// stale `company` snapshot, because both fold the live override onto
/// `company`'s base. `resolve_tenant_search` must do the same: a company
/// snapshot that predates a console `search` grant must still resolve the
/// backend once the live override is passed in, or the roster ends up
/// crediting a capability no tool was ever wired for.
#[tokio::test]
async fn resolve_tenant_search_honours_a_console_grant_a_stale_company_misses() {
    use crate::ports::types::{Actor, ActorKind, ToolGrantsOverride};

    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(MockContext::default());

    // The stale snapshot: no explicit `search` grant in its own
    // `[tools].allow`, exactly what a caller holding a boot-time
    // `CompanyRecord` still has after an admin grants `search` from the
    // console without a hot rebuild. `*` covers files/shell/code/web but
    // deliberately not `search` — the same base the grant-fingerprint
    // test above uses.
    let mut rec = record();
    rec.manifest.tools.allow = vec!["*".to_string()];
    assert!(
        !crate::company::grants_search_explicit(&rec.manifest.tools.allow),
        "the fixture must start without an explicit search grant"
    );

    let mut deps = deps_with_plan(dir.path(), context.clone(), None, None);
    // No secret store wired: the fallback path returns the last known
    // connection, standing in for a company whose provider is already on
    // file.
    deps.secrets = None;
    deps.tenant_search = Some(search_byo::TenantSearch::for_test(
        "brave",
        Some("test-key"),
        None,
    ));

    let overlay_tool_grants = ToolGrantsOverride {
        added: vec!["search".to_string()],
        set_by: Actor {
            kind: ActorKind::User,
            id: "user-admin".to_string(),
        },
        at_millis: crate::ports::now_millis(),
    };

    let pool = HarnessPool::new();
    let resolved = pool
        .resolve_tenant_search(&rec, &deps, Some(&overlay_tool_grants))
        .await;
    assert!(
        resolved.is_some(),
        "a console grant the live overlay carries must resolve the search \
         backend even when the `company` snapshot passed in predates it"
    );
}

/// **The no-restart proof.** A daily cap written through the company store —
/// the exact path `PUT …/team/{id}/budget` writes through — is enforced on
/// the company's **next dispatch**, in one process, with no restart and no
/// redeploy.
///
/// This is the whole of #343 at the layer that decides whether a teammate
/// works. Before it, `budget_usd_daily` was readable only from the manifest,
/// which is a boot snapshot baked into the tenant image — so an operator
/// whose teammate had stopped had no remedy short of us shipping a new
/// image. The four phases walk exactly that operator's day:
///
///   A. the CEO has spent its manifest $5 and is refused (issue #304, and
///      the state that motivates the issue);
///   B. an admin **raises** the cap to $50 — the stopped teammate works
///      again on its very next turn. This is the acceptance criterion;
///   C. the admin sets the cap to **$0** — a real cap of nothing, refused
///      from the first cent;
///   D. the admin **clears** the cap — an explicitly-uncapped override that
///      beats the manifest's $5 even with $5 already spent, so the teammate
///      works again.
///
/// C and D are the same route with different bodies and they must not
/// resolve alike: C refuses, D runs. That is "clearing is distinct from
/// zeroing" asserted on live behaviour rather than on a type.
///
/// Throughout, the pool holds **one** resident company and is never
/// reconstructed — `resident_companies()` stays 1 and the same `pool` binding
/// serves every phase — so the only mechanism that can be carrying these
/// changes is the budget fingerprint flipping and `ensure` rebuilding the
/// roster in place. Each phase asserts that fingerprint actually moved.
#[tokio::test]
async fn a_budget_written_through_the_store_is_enforced_on_the_next_dispatch() {
    use crate::ports::types::{Actor, ActorKind, BudgetOverride};

    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(MockContext::default());
    let meter = Arc::new(RecordingMeter::default());
    let rec = capped_record();

    // A live store, so `ensure` re-resolves the overrides the way it does in
    // production. `deps_with_plan`'s default store is inert.
    let live_store = Arc::new(LiveStore::default());
    live_store.save(&rec).await.unwrap();

    // The CEO has already spent its manifest $5 today.
    meter
        .record(
            &rec.id,
            &spend_sample("ceo", 5.00, crate::ports::now_millis()),
        )
        .await
        .unwrap();

    let mut deps = deps_with_plan(
        dir.path(),
        context.clone(),
        Some(meter.clone() as Arc<dyn UsageMeter>),
        None,
    );
    deps.store = live_store.clone();

    // ONE pool for the whole test. Nothing below reconstructs it, so nothing
    // below can be smuggling in a restart.
    let pool = HarnessPool::new();

    /// Writes an override through the store exactly as the console route
    /// does, and returns the record for the next `ensure`.
    fn with_override(base: &CompanyRecord, cap: Option<f64>) -> CompanyRecord {
        let mut next = base.clone();
        next.overlay_budgets = vec![BudgetOverride {
            agent_id: "ceo".to_string(),
            budget_usd_daily: cap,
            set_by: Actor {
                kind: ActorKind::User,
                id: "user-admin".to_string(),
            },
            at_millis: crate::ports::now_millis(),
        }];
        next
    }

    // --- A. The manifest cap is spent: the teammate is stopped. ----------
    pool.ensure(&rec, &deps).await.expect("ensure A");
    let fp_manifest = pool
        .budget_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    let refused = pool
        .run(
            &rec.id,
            "ceo",
            "should-not-echo",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("a refusal is a benign outcome")
        .reply;
    assert_eq!(
        refused,
        agent_budget_exhausted_notice("ceo", 5.0),
        "phase A: the manifest's $5 cap is spent, so dispatch is refused"
    );

    // --- B. An admin raises the cap. The teammate works again. -----------
    live_store
        .save(&with_override(&rec, Some(50.0)))
        .await
        .unwrap();
    pool.ensure(&rec, &deps).await.expect("ensure B");
    let fp_raised = pool
        .budget_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(
        fp_manifest, fp_raised,
        "phase B: setting a cap must move the budget fingerprint, or the \
         cached roster is reused and the change never reaches the gate"
    );
    assert_eq!(
        pool.resident_companies().await,
        1,
        "phase B: the same company, rebuilt in place — not a new process"
    );
    let unblocked = pool
        .run(
            &rec.id,
            "ceo",
            "hello-marker",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("the raised cap unblocks the teammate")
        .reply;
    assert!(
        unblocked.contains("hello-marker"),
        "phase B: raising the cap from the console must unblock the stopped \
         teammate on its very next dispatch, with no restart: {unblocked:?}"
    );

    // --- C. The admin sets the cap to zero. Zero is a real cap. ----------
    live_store
        .save(&with_override(&rec, Some(0.0)))
        .await
        .unwrap();
    pool.ensure(&rec, &deps).await.expect("ensure C");
    let fp_zero = pool
        .budget_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(fp_raised, fp_zero, "phase C: lowering a cap is a change");
    let zeroed = pool
        .run(
            &rec.id,
            "ceo",
            "should-not-echo",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("a refusal is a benign outcome")
        .reply;
    assert_eq!(
        zeroed,
        agent_budget_exhausted_notice("ceo", 0.0),
        "phase C: a $0 cap refuses from the first cent"
    );

    // --- D. The admin clears the cap. Cleared is not zero. ---------------
    live_store.save(&with_override(&rec, None)).await.unwrap();
    pool.ensure(&rec, &deps).await.expect("ensure D");
    let fp_cleared = pool
        .budget_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(
        fp_zero, fp_cleared,
        "phase D: 'no cap' and 'a cap of $0' must not hash alike — if they \
         did, clearing a cap would silently leave the teammate at zero"
    );
    let cleared = pool
        .run(
            &rec.id,
            "ceo",
            "hello-marker",
            &deps,
            crate::runtime::delegation::ChatTarget::default(),
        )
        .await
        .expect("an explicitly-uncapped teammate runs")
        .reply;
    assert!(
        cleared.contains("hello-marker"),
        "phase D: an explicitly-uncapped override beats the manifest's $5 \
         even with $5 already spent today: {cleared:?}"
    );

    // Nothing above restarted anything.
    assert_eq!(
        pool.resident_companies().await,
        1,
        "one company, rebuilt in place across all four phases"
    );

    // A further `ensure` with no change is a no-op: the axis is not thrashing
    // the roster (and dropping live agent sessions) on every turn.
    pool.ensure(&rec, &deps).await.expect("ensure idempotent");
    assert_eq!(pool.budget_fingerprint_of(&rec.id).await, Some(fp_cleared));
}

/// `override_fingerprint` detects a persona edit, keeps `Some("")` distinct
/// from `None` (the reset-to-blueprint distinction), and does not depend on
/// the stored order — the `HashMap` the overrides come from has none, so an
/// order-sensitive hash would rebuild the roster (dropping live sessions) on
/// a save that changed nothing (issue #1530).
#[test]
fn override_fingerprint_detects_edits_and_ignores_order() {
    use crate::ports::types::AgentOverride;
    let entry = |id: &str, text: Option<&str>| AgentOverride {
        agent_id: id.to_string(),
        instructions: text.map(str::to_string),
        ..Default::default()
    };

    assert_ne!(
        override_fingerprint(&[]),
        override_fingerprint(&[entry("ceo", Some("x"))]),
        "adding an override must move the fingerprint"
    );
    assert_ne!(
        override_fingerprint(&[entry("ceo", Some("a"))]),
        override_fingerprint(&[entry("ceo", Some("b"))]),
        "editing the text must move the fingerprint"
    );
    assert_ne!(
        override_fingerprint(&[entry("ceo", Some(""))]),
        override_fingerprint(&[entry("ceo", None)]),
        "an empty-string override and a cleared one must not hash alike"
    );
    let ab = [entry("ceo", Some("a")), entry("eng", Some("b"))];
    let ba = [entry("eng", Some("b")), entry("ceo", Some("a"))];
    assert_eq!(
        override_fingerprint(&ab),
        override_fingerprint(&ba),
        "the fingerprint must not depend on the stored order"
    );
}

/// A routing edit — a model or harness re-bind — has to move the persona
/// fingerprint too: the roster the harness builds reads those fields, so a
/// re-bind that moved nothing would be silently ignored until the process
/// restarted (issue #1676 review note). The `Some("")` "cleared" form stays
/// distinct from `None` ("never edited"), the same discriminant the
/// reset-to-blueprint contract depends on.
#[test]
fn override_fingerprint_moves_on_a_model_or_harness_change() {
    use crate::ports::types::AgentOverride;
    let entry = |model: Option<&str>, harness: Option<&str>| AgentOverride {
        agent_id: "ceo".into(),
        model: model.map(str::to_string),
        harness: harness.map(str::to_string),
        ..Default::default()
    };

    assert_ne!(
        override_fingerprint(&[]),
        override_fingerprint(&[entry(Some("chat-v2"), None)]),
        "a model override must move the fingerprint or the re-bind is ignored until restart"
    );
    assert_ne!(
        override_fingerprint(&[entry(Some("chat-v2"), None)]),
        override_fingerprint(&[entry(None, Some("acp"))]),
        "a harness override must move the fingerprint too"
    );
    assert_ne!(
        override_fingerprint(&[]),
        override_fingerprint(&[entry(Some(""), None)]),
        "an explicit model clear must not hash like an untouched teammate"
    );
    // The same override twice → the same fingerprint (no spurious rebuild).
    assert_eq!(
        override_fingerprint(&[entry(Some("chat-v2"), None)]),
        override_fingerprint(&[entry(Some("chat-v2"), None)])
    );
}

/// The persona-override fingerprint is filtered the same way as the overlay
/// one: a row carrying only a face has no persona text to hash, so choosing
/// or clearing an avatar for a teammate with no other override must not
/// rebuild the roster (issue #1676 review note).
#[test]
fn override_fingerprint_ignores_an_avatar_only_row() {
    use crate::ports::types::AgentOverride;
    let avatar_only = AgentOverride {
        agent_id: "ceo".into(),
        avatar: Some("tiny:robot".into()),
        ..Default::default()
    };
    let persona = AgentOverride {
        agent_id: "ceo".into(),
        instructions: Some("speak plainly".into()),
        avatar: Some("tiny:robot".into()),
        ..Default::default()
    };

    // A row carrying only a face hashes like no row at all.
    assert_eq!(
        override_fingerprint(&[]),
        override_fingerprint(std::slice::from_ref(&avatar_only)),
        "an avatar-only row must not move the fingerprint"
    );
    // A persona edit still moves it, with or without a face riding along.
    assert_ne!(
        override_fingerprint(&[]),
        override_fingerprint(std::slice::from_ref(&persona)),
        "a persona edit must still move the fingerprint"
    );
    // Two rows differing only in their face hash alike — no spurious rebuild
    // when an operator changes one teammate's avatar.
    assert_eq!(
        override_fingerprint(std::slice::from_ref(&persona)),
        override_fingerprint(std::slice::from_ref(&AgentOverride {
            avatar: Some("tiny:fox".into()),
            ..persona
        })),
        "the face must not be part of the fingerprint"
    );
}

/// A persona override written through the store reaches the roster on the
/// next dispatch — the cache-invalidation the whole feature turns on (#1530).
/// The pool is never reconstructed (`resident_companies()` stays 1), so the
/// only thing carrying the edit is `override_fingerprint` flipping and
/// `ensure` rebuilding the roster in place. Reset-to-blueprint returns the
/// fingerprint to its pre-edit value, proving the clear is a real change the
/// roster picks up too.
#[tokio::test]
async fn a_persona_override_written_through_the_store_rebuilds_the_roster() {
    use crate::ports::types::AgentOverride;

    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(MockContext::default());
    let rec = capped_record();

    // A live store, so `ensure` re-resolves the overrides as production does.
    let live_store = Arc::new(LiveStore::default());
    live_store.save(&rec).await.unwrap();

    let mut deps = deps_with_plan(dir.path(), context.clone(), None, None);
    deps.store = live_store.clone();

    // ONE pool for the whole test — nothing below reconstructs it, so nothing
    // below can smuggle in a restart.
    let pool = HarnessPool::new();

    fn with_instructions(base: &CompanyRecord, text: Option<&str>) -> CompanyRecord {
        let mut next = base.clone();
        next.overlay_agent_edits = match text {
            Some(text) => vec![AgentOverride {
                agent_id: "ceo".to_string(),
                instructions: Some(text.to_string()),
                ..Default::default()
            }],
            None => Vec::new(),
        };
        next
    }

    // A. Blueprint — the CEO runs on its manifest persona, no override.
    pool.ensure(&rec, &deps).await.expect("ensure A");
    let fp_blueprint = pool
        .override_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");

    // B. An operator edits the CEO's persona from the console.
    live_store
        .save(&with_instructions(&rec, Some("Answer only in haiku.")))
        .await
        .unwrap();
    pool.ensure(&rec, &deps).await.expect("ensure B");
    let fp_edited = pool
        .override_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(
        fp_blueprint, fp_edited,
        "editing a persona must move the override fingerprint, or the cached \
         roster is reused and the edit never reaches the next turn"
    );
    assert_eq!(
        pool.resident_companies().await,
        1,
        "the same company, rebuilt in place — not a new process"
    );

    // C. Reset-to-blueprint clears the override; the persona returns to seed.
    live_store
        .save(&with_instructions(&rec, None))
        .await
        .unwrap();
    pool.ensure(&rec, &deps).await.expect("ensure C");
    let fp_reset = pool
        .override_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");
    assert_ne!(fp_edited, fp_reset, "clearing the override is a change");
    assert_eq!(
        fp_reset, fp_blueprint,
        "reset-to-blueprint must return to the pre-edit fingerprint"
    );

    // An unchanged `ensure` is a no-op: the axis is not thrashing the roster.
    pool.ensure(&rec, &deps).await.expect("ensure idempotent");
    assert_eq!(pool.override_fingerprint_of(&rec.id).await, Some(fp_reset));
}

/// A `PATCH {scope}` rename reaches the roster on the next dispatch (PR
/// #1875 review finding), mirroring
/// `a_persona_override_written_through_the_store_rebuilds_the_roster`
/// immediately above for the company-name axis. Before this fix, no
/// fingerprint moved on a rename, so the cached roster — and every
/// agent's persona, which embeds `manifest.company.name` — kept
/// answering to the old name until an unrelated axis happened to change
/// or the process restarted.
#[tokio::test]
async fn a_company_rename_written_through_the_store_rebuilds_the_roster() {
    let dir = tempfile::tempdir().unwrap();
    let context = Arc::new(MockContext::default());
    let rec = capped_record();
    assert_eq!(rec.manifest.company.name, "Acme");

    // A live store, so `ensure` re-resolves the name as production does —
    // exactly the pattern the persona-override test above uses.
    let live_store = Arc::new(LiveStore::default());
    live_store.save(&rec).await.unwrap();

    let mut deps = deps_with_plan(dir.path(), context.clone(), None, None);
    deps.store = live_store.clone();

    // ONE pool for the whole test — nothing below reconstructs it, so
    // nothing below can smuggle in a restart.
    let pool = HarnessPool::new();

    // A. Blueprint name.
    pool.ensure(&rec, &deps).await.expect("ensure A");
    let fp_before = pool
        .company_name_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");

    // B. `PATCH {scope}` renames the company (`server::ops::company_profile`
    // writes straight into `manifest.company.name` and saves).
    let mut renamed = rec.clone();
    renamed.manifest.company.name = "New Name Inc.".to_string();
    live_store.save(&renamed).await.unwrap();
    pool.ensure(&rec, &deps).await.expect("ensure B");
    let fp_after = pool
        .company_name_fingerprint_of(&rec.id)
        .await
        .expect("fingerprinted");

    assert_ne!(
        fp_before, fp_after,
        "renaming the company must move the company-name fingerprint, or the \
         cached roster is reused and every agent's persona keeps the old name \
         until an unrelated axis changes or the process restarts"
    );
    assert_eq!(
        pool.resident_companies().await,
        1,
        "the same company, rebuilt in place — not a new process"
    );

    // An unchanged `ensure` is a no-op: the axis is not thrashing the roster.
    pool.ensure(&rec, &deps).await.expect("ensure idempotent");
    assert_eq!(
        pool.company_name_fingerprint_of(&rec.id).await,
        Some(fp_after)
    );
}
