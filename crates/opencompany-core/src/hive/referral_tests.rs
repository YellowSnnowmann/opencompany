//! Tests for the cross-desk referral seam: the pure decision, the journal
//! queue's idempotency, and the crossing record's round trip.

use super::*;
use crate::hive::routing::desk_routing;
use crate::hive::test_support::{MemoryLog, TWO_DESKS, record};
use crate::ports::events::EventLog;
use tinyhivemind::dispatch::{DispatchConversation, DispatchKey};
use tinyhivemind_core::mention::{Mention, MentionAuthor};

fn input(record: &CompanyRecord, author: &str, text: &str, hop: u32) -> ReferralInput {
    let members = crate::runtime::delegation_tools::tinyhivemind_roster(record);
    let desks = crate::runtime::delegation_tools::tinyhivemind_desks(record);
    let roster = Roster::new(&members, &[], &[]);
    let mentions = tinyhivemind_core::mention::resolve(
        text,
        None,
        &MentionAuthor::Agent { id: author.into() },
        &roster,
        &desks.set(),
    );
    ReferralInput {
        key: DispatchKey {
            trigger_sequence: 7,
        },
        conversation: DispatchConversation {
            desk_id: "engineering".into(),
            thread_root: None,
        },
        author_id: author.into(),
        content: text.into(),
        mentions,
        hop,
        origin: None,
    }
}

fn mentions_of(text: &str) -> Vec<Mention> {
    let record = record(TWO_DESKS);
    input(&record, "engineer", text, 0).mentions
}

#[test]
fn a_desk_mention_crosses_and_a_local_one_does_not() {
    let record = record(TWO_DESKS);
    let policy = desk_routing(&record, "engineering").referral_policy();
    assert!(policy.enabled);
    let members = crate::runtime::delegation_tools::tinyhivemind_roster(&record);
    let desks = crate::runtime::delegation_tools::tinyhivemind_desks(&record);
    let roster = Roster::new(&members, &[], &[]);
    let crossing = input(
        &record,
        "engineer",
        "@#content can you draft the copy for the login page?",
        0,
    );
    assert!(
        !mentions_of("@#content please").is_empty(),
        "desk mention resolves"
    );
    let referral = decide(policy, &crossing, &roster, &desks.set()).expect("crosses");
    assert_eq!(referral.to.desk_id, "content");
    assert_eq!(referral.target_id, "writer");
    assert_eq!(referral.child_hop, 1);
    assert!(referral.crosses());
    let desk_referral = DeskReferral::from_referral(&referral, &record, "ep-1", None, true);
    assert_eq!(desk_referral.from_desk_name, "Engineering desk");
    assert_eq!(desk_referral.asker_label, "engineer");
    assert!(
        desk_referral
            .seed_text()
            .starts_with("@engineer on #Engineering desk asks: ")
    );
    assert_eq!(
        asked_message(&desk_referral.seed_text()),
        "@#content can you draft the copy for the login page?"
    );
    let address = desk_referral
        .return_address(EventSeq::new(11))
        .expect("returns");
    assert_eq!(address.episode_id, "ep-1");
    assert_eq!(address.asker, "engineer");
    assert_eq!(address.forward_seq, 11);
    // A teammate on this desk stays here.
    let local = input(&record, "engineer", "@ceo what do you think?", 0);
    assert!(decide(policy, &local, &roster, &desks.set()).is_none());
    // The hop budget is spent.
    let spent = input(&record, "engineer", "@#content again", 1);
    assert!(decide(policy, &spent, &roster, &desks.set()).is_none());
    // A desk with referral off never crosses.
    let mut off = record.clone();
    off.manifest.group_chats[0].hive.referral = None;
    let policy = desk_routing(&off, "engineering").referral_policy();
    assert!(decide(policy, &crossing, &roster, &desks.set()).is_none());
}

#[tokio::test]
async fn the_journal_queue_enqueues_once_per_trigger() {
    let record = record(TWO_DESKS);
    let log = MemoryLog::default();
    let company = MemoryLog::company();
    let policy = desk_routing(&record, "engineering").referral_policy();
    let members = crate::runtime::delegation_tools::tinyhivemind_roster(&record);
    let desks = crate::runtime::delegation_tools::tinyhivemind_desks(&record);
    let roster = Roster::new(&members, &[], &[]);
    let crossing = input(&record, "engineer", "@#content draft it", 0);
    let queue = JournalReferralQueue::new(&log, &company);
    let outcome =
        tinyhivemind::referral::dispatch_referral(&queue, policy, &crossing, &roster, &desks.set())
            .await
            .unwrap();
    assert_eq!(
        outcome,
        tinyhivemind::referral::ReferralOutcome::Referred { crossed: true }
    );
    let accepted = queue.drain();
    assert_eq!(accepted.len(), 1);
    // The driver journals the forward marker once it has opened the far
    // episode; a second decision on the same trigger then finds it.
    let desk_referral = DeskReferral::from_referral(&accepted[0], &record, "ep-1", None, true);
    let forward = log
        .append(&company, desk_referral.forward_event(Some("ep-2")))
        .await
        .unwrap();
    let again =
        tinyhivemind::referral::dispatch_referral(&queue, policy, &crossing, &roster, &desks.set())
            .await
            .unwrap();
    assert_eq!(again, tinyhivemind::referral::ReferralOutcome::Already);
    assert!(queue.drain().is_empty());

    let stored = log.rows().pop().unwrap();
    let CompanyEvent::ReferralEnqueued {
        from_desk,
        to_desk,
        target,
        asker,
        episode_id,
        to_episode_id,
        hop,
        returning,
        trigger_sequence,
        ..
    } = stored.event
    else {
        panic!("a forward marker");
    };
    assert_eq!(
        (from_desk.as_str(), to_desk.as_str()),
        ("engineering", "content")
    );
    assert_eq!((target.as_str(), asker.as_str()), ("writer", "engineer"));
    assert_eq!(episode_id.as_deref(), Some("ep-1"));
    assert_eq!(to_episode_id.as_deref(), Some("ep-2"));
    assert_eq!((hop, returning, trigger_sequence), (1, false, 7));

    let origin = desk_referral.return_address(forward).unwrap();
    let back = return_event(
        &record,
        &origin,
        "content",
        "writer",
        EventSeq::new(20),
        "ep-2",
    );
    let CompanyEvent::ReferralEnqueued {
        returning,
        answers,
        to_desk,
        target,
        episode_id,
        ..
    } = back
    else {
        panic!("a return marker");
    };
    assert!(returning);
    assert_eq!(answers, Some(forward.value()));
    assert_eq!(
        (to_desk.as_str(), target.as_str()),
        ("engineering", "engineer")
    );
    assert_eq!(episode_id.as_deref(), Some("ep-1"));
}

#[test]
fn answer_rows_attribute_and_unattribute_symmetrically() {
    let note = returned_note("writer", "Content desk", "  Use the short form.  ");
    assert_eq!(
        note,
        "@writer on #Content desk answered: Use the short form."
    );
    assert_eq!(
        unattributed("writer", "Content desk", &note),
        "Use the short form."
    );
    assert_eq!(unattributed("writer", "Content desk", "plain"), "plain");
    assert_eq!(asked_message("no head here"), "no head here");
    assert_eq!(pair_conversation("zed", "amy"), "dm:amy+zed");
    assert!(is_hive_author(HIVE_REFERRAL_AUTHOR));
    assert!(is_legacy_report_author("hive-report"));
    assert!(!is_hive_author("writer"));
}
