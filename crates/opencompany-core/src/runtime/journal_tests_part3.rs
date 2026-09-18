use super::tests_core::*;

/// Revoked, expired, and *silently lapsed* standing grants all stay gone.
///
/// The third case is the one only replay can catch: the sweep runs while the
/// process is up, so a host that was down across a deadline never wrote a
/// `StandingGrantExpired` line. Replaying on the record alone would hand the
/// permission back, making a restart a way to resurrect one.
#[tokio::test]
async fn revoked_expired_and_lapsed_standing_grants_are_not_rehydrated() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    for g in [
        standing("revoked", "shell", 100_000),
        standing("expired", "workspace_write", 100_000),
        standing("lapsed", "web_fetch", 3_000),
        standing("live", "shell", 100_000),
    ] {
        journal.record_standing_granted(&g).await.unwrap();
    }

    journal
        .record_standing_revoked(
            &GrantId::new("revoked"),
            Actor {
                kind: crate::ports::types::ActorKind::User,
                id: "user-42".into(),
            },
            5_000,
        )
        .await
        .unwrap();
    journal
        .record_standing_expired(&GrantId::new("expired"), 5_000)
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    // `lapsed` has no terminal record at all — only its deadline stops it.
    let replayed = reloaded.replayed_standing_grants(10_000);
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].id, GrantId::new("live"));

    let raw = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(raw.contains("StandingGrantMinted"));
    assert!(raw.contains("StandingGrantRevoked"));
    assert!(raw.contains("StandingGrantExpired"));
}

/// A journal written before #374 decodes unchanged, and replays no standing
/// grants. The forward-only half — an old binary cannot read a new journal —
/// is the same contract every prior variant addition made.
#[tokio::test]
async fn a_pre_374_journal_decodes_and_yields_no_standing_grants() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    journal
        .record_parked(
            &ApprovalId::new("appr-old"),
            &effect(),
            500,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    journal
        .record_granted(&grant("appr-old", 1_000))
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert_eq!(reloaded.pending().len(), 1);
    assert_eq!(
        reloaded.replayed_grants().len(),
        1,
        "the single-use path replays byte-identically"
    );
    assert!(reloaded.replayed_standing_grants(2_000).is_empty());
}

/// Issue #1805: an `ApprovalExtended` line moves the deadline anchor, and the
/// move replays on reload — so an operator's extension survives a redeploy
/// rather than reverting to the original park window. The payload timestamp
/// (`at_millis`, issue #1024) is deliberately left where it was: extending a
/// deadline does not make the content fresher.
#[tokio::test]
async fn an_extension_replays_and_moves_only_the_deadline_anchor() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    let id = ApprovalId::new("appr-extend");
    journal
        .record_parked(
            &id,
            &effect(),
            1_000,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    // A fresh park's anchor is the park instant.
    assert_eq!(journal.pending()[0].deadline_anchor_millis, 1_000);

    journal
        .record_extended(
            &id,
            9_000,
            crate::ports::types::Actor {
                kind: crate::ports::types::ActorKind::User,
                id: "operator".into(),
            },
        )
        .await
        .unwrap();
    // The live queue moved immediately.
    assert_eq!(journal.pending()[0].deadline_anchor_millis, 9_000);

    // And a reload replays the move rather than the bare park.
    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    let pending = reloaded.pending();
    assert_eq!(
        pending.len(),
        1,
        "the approval is still parked after reload"
    );
    assert_eq!(
        pending[0].deadline_anchor_millis, 9_000,
        "the extension survived the reload"
    );
    assert_eq!(
        pending[0].at_millis, 1_000,
        "the payload timestamp is untouched by an extension"
    );
}

/// The grant records must not disturb the approval-queue fold they share a
/// file with — including #309's origin index, which the Task Detail
/// waiting-time read joins against.
#[tokio::test]
async fn grant_records_leave_the_parked_queue_and_origins_intact() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    let parked_id = ApprovalId::new("appr-parked");
    journal
        .record_parked(
            &parked_id,
            &effect(),
            500,
            TaskLink::Unlinked,
            ApprovalConversation::default(),
            None,
        )
        .await
        .unwrap();
    journal
        .record_granted(&grant("appr-granted", 1_000))
        .await
        .unwrap();
    journal
        .record_grant_consumed(&ApprovalId::new("appr-granted"), None)
        .await
        .unwrap();

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    assert_eq!(
        reloaded.pending().len(),
        1,
        "the parked approval is untouched"
    );
    assert_eq!(reloaded.pending()[0].id, parked_id);
    assert_eq!(
        reloaded
            .approval_origins()
            .get(&parked_id)
            .map(|o| o.at_millis),
        Some(500)
    );
    assert!(reloaded.replayed_grants().is_empty());
}

/// **Issue #386**: rapid appends through a *single* journal must not tear a
/// line.
///
/// This is the shape CI actually hit. `append` used to leave its trailing
/// newline in a `tokio::fs::File` whose background write nobody awaited,
/// then drop the handle and release the lock — so the next append's opening
/// bytes could reach the file before the previous record's terminator, and
/// two records landed on one line. One writer was enough; concurrency
/// across instances was never required.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rapid_appends_through_one_journal_never_tear_a_line() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    const N: usize = 256;
    for i in 0..N {
        journal
            .record_executed(&format!("cyc:{i}"), executed(i as u64))
            .await
            .unwrap();
    }

    let records = parse_every_line(&path).await;
    assert_eq!(records.len(), N, "every append is its own line");

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    for i in 0..N {
        assert!(
            reloaded.is_executed(&format!("cyc:{i}")),
            "cyc:{i} must survive the reload",
        );
    }
}

/// **Issue #386**: a line an old host merged replays in full.
///
/// This is the shape already sitting in journals written before the write
/// fix, and the shape CI tripped over. It must not be *skipped*: dropping a
/// merged line would un-commit an `EffectExecuted` key and let an
/// at-most-once effect fire again, which is a worse outcome than the parse
/// error it replaces.
#[tokio::test]
async fn a_merged_line_replays_every_record_it_holds() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");

    let merged = format!(
        "{}{}",
        serde_json::to_string(&JournalRecord::EffectExecuted {
            key: "cyc:0".into(),
            effect: Some(executed(0)),
        })
        .unwrap(),
        serde_json::to_string(&JournalRecord::EffectExecuted {
            key: "cyc:1".into(),
            effect: Some(executed(1)),
        })
        .unwrap(),
    );
    let intact = serde_json::to_string(&JournalRecord::EffectExecuted {
        key: "cyc:2".into(),
        effect: Some(executed(2)),
    })
    .unwrap();
    tokio::fs::write(&path, format!("{merged}\n{intact}\n"))
        .await
        .unwrap();

    let journal = RuntimeJournal::new(&path);
    journal
        .load()
        .await
        .expect("a merged line must not fail the load");
    for key in ["cyc:0", "cyc:1", "cyc:2"] {
        assert!(journal.is_executed(key), "{key} must replay");
    }
    assert!(
        journal.corruption().is_empty(),
        "a merged line is recovered, not lost, so it is not corruption",
    );
}

/// **Issue #386**: a truncated line is reported, and the records around it
/// still replay.
///
/// The old `load` returned `Err` here, which failed the company's boot: one
/// unreadable line cost every readable one after it, plus the console an
/// operator would need to repair the file.
#[tokio::test]
async fn a_truncated_line_is_reported_and_the_rest_still_replays() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");

    let record = |key: &str, at| {
        serde_json::to_string(&JournalRecord::EffectExecuted {
            key: key.into(),
            effect: Some(executed(at)),
        })
        .unwrap()
    };
    let whole = record("cyc:1", 1);
    let truncated = &whole[..whole.len() / 2];
    tokio::fs::write(
        &path,
        format!(
            "{}\n{truncated}\n{}\n",
            record("cyc:0", 0),
            record("cyc:2", 2)
        ),
    )
    .await
    .unwrap();

    let journal = RuntimeJournal::new(&path);
    journal
        .load()
        .await
        .expect("one bad line must not fail the boot");

    assert!(journal.is_executed("cyc:0"), "the line before must replay");
    assert!(
        journal.is_executed("cyc:2"),
        "the lines after the damage are the ones the old load lost",
    );
    assert!(
        !journal.is_executed("cyc:1"),
        "the truncated record is gone"
    );

    let corruption = journal.corruption();
    assert_eq!(corruption.len(), 1, "exactly one line was unreadable");
    assert_eq!(corruption[0].line, 2, "the report must locate the line");
    assert_eq!(corruption[0].bytes, truncated.len());
    assert!(
        !corruption[0].message.contains("filing.submit"),
        "a corruption report must not quote the line's contents",
    );
}

/// **Issue #386**: a torn write can split a multi-byte codepoint, so the
/// damaged line is not merely bad JSON — it is not valid UTF-8 at all.
///
/// `load` used to `read_to_string`, which fails on the first invalid byte
/// anywhere in the file. That turned exactly the damage this recovery path
/// exists for into the whole-boot failure it exists to prevent, and no
/// amount of per-line JSON handling downstream could have saved it. Raised
/// in review of PR #389.
#[tokio::test]
async fn a_line_that_is_not_valid_utf8_is_skipped_like_any_other_damage() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");

    let record = |key: &str, at| {
        serde_json::to_string(&JournalRecord::EffectExecuted {
            key: key.into(),
            effect: Some(executed(at)),
        })
        .unwrap()
    };

    // A lone continuation byte: never valid on its own, which is what the
    // tail of a split codepoint looks like.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(record("cyc:0", 0).as_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(&[0x7b, 0x9f, 0x8d]);
    bytes.push(b'\n');
    bytes.extend_from_slice(record("cyc:2", 2).as_bytes());
    bytes.push(b'\n');
    tokio::fs::write(&path, &bytes).await.unwrap();

    let journal = RuntimeJournal::new(&path);
    journal
        .load()
        .await
        .expect("invalid UTF-8 on one line must not fail the boot");

    assert!(journal.is_executed("cyc:0"), "the line before must replay");
    assert!(
        journal.is_executed("cyc:2"),
        "the lines after the damage must still replay",
    );

    let corruption = journal.corruption();
    assert_eq!(corruption.len(), 1, "exactly one line was unreadable");
    assert_eq!(corruption[0].line, 2, "the report must locate the line");
}

/// **Issue #386**: when `append` returns, the record is on the file.
///
/// The deterministic half of the bug, and the more serious one. The
/// at-most-once guarantee is that an effect's key is durable *before* the
/// side effect runs; the old write path returned once the write was queued
/// on tokio's blocking pool, so `record_executed` reported a commit that a
/// crash could still lose and an `ENOSPC` on the real write reached nobody.
/// Measured against that path, 199 of 200 appends failed this assertion —
/// the torn line was the rare, visible symptom of a window that was open
/// almost always.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_append_has_reached_the_file_before_it_returns() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");
    let journal = RuntimeJournal::new(&path);

    let mut expected = 0usize;
    for i in 0..64u64 {
        let key = format!("cyc:{i}");
        expected += serde_json::to_string(&JournalRecord::EffectExecuted {
            key: key.clone(),
            effect: Some(executed(i)),
        })
        .unwrap()
        .len()
            + 1;
        journal.record_executed(&key, executed(i)).await.unwrap();
        // A synchronous stat, so the assertion cannot be satisfied by the
        // very blocking pool that would still be running a queued write.
        let on_disk = std::fs::metadata(&path).expect("journal file").len() as usize;
        assert_eq!(
            on_disk,
            expected,
            "append #{} returned with {} of {expected} bytes on the file",
            i + 1,
            on_disk,
        );
    }
}

/// **Issue #386**: two journals over one path must not interleave.
///
/// `write_lock` is per-instance, so it serialises nothing between two
/// `RuntimeJournal` values sharing a file. Nothing in the type stops a
/// caller building two, and the test suite builds them routinely. The
/// defence is the process-wide per-path lock plus the single whole-line
/// write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_appends_from_two_journals_over_one_path_lose_nothing() {
    let dir = tmp_dir();
    let path = dir.path().join("journal.jsonl");

    const N: usize = 128;
    let one = Arc::new(RuntimeJournal::new(&path));
    let two = Arc::new(RuntimeJournal::new(&path));

    let a = tokio::spawn({
        let one = Arc::clone(&one);
        async move {
            for i in 0..N {
                one.record_executed(&format!("a:{i}"), executed(i as u64))
                    .await
                    .unwrap();
            }
        }
    });
    let b = tokio::spawn({
        let two = Arc::clone(&two);
        async move {
            for i in 0..N {
                two.record_executed(&format!("b:{i}"), executed(i as u64))
                    .await
                    .unwrap();
            }
        }
    });
    a.await.unwrap();
    b.await.unwrap();

    let records = parse_every_line(&path).await;
    assert_eq!(records.len(), N * 2, "no record may be lost or merged");

    let reloaded = RuntimeJournal::new(&path);
    reloaded.load().await.unwrap();
    for i in 0..N {
        assert!(reloaded.is_executed(&format!("a:{i}")), "a:{i} lost");
        assert!(reloaded.is_executed(&format!("b:{i}")), "b:{i} lost");
    }
}

/// **Old journal lines replay unchanged (issue #971).**
///
/// Every `ApprovalExpired` written before the field existed was a TTL
/// expiry, so the serde default is the truth about them. A missing default
/// here would not be a cosmetic regression: replay is how the parked queue
/// is rebuilt at boot, and a line that fails to parse leaves an approval
/// resurrected that the host had already retired.
#[test]
fn a_pre_reason_expiry_line_replays_as_a_ttl_expiry() {
    let old_line = r#"{"record":"ApprovalExpired","id":"ap-old","at_millis":42}"#;
    let parsed: JournalRecord = serde_json::from_str(old_line).expect("old line must replay");
    match parsed {
        JournalRecord::ApprovalExpired {
            id,
            at_millis,
            reason,
        } => {
            assert_eq!(id.as_ref(), "ap-old");
            assert_eq!(at_millis, 42);
            assert_eq!(reason, ExpiryReason::Ttl);
        }
        other => panic!("expected ApprovalExpired, got {other:?}"),
    }

    // And a line written today carries the reason explicitly, so the two
    // are told apart by what is on the wire rather than by inference.
    let written = serde_json::to_string(&JournalRecord::ApprovalExpired {
        id: ApprovalId::new("ap-new"),
        at_millis: 43,
        reason: ExpiryReason::Ttl,
    })
    .expect("serialize");
    assert!(written.contains(r#""reason":"ttl""#), "{written}");
}

#[test]
fn expired_and_amended_records_round_trip_under_record_tag() {
    for record in [
        JournalRecord::ApprovalExpired {
            id: ApprovalId::new("x"),
            at_millis: 42,
            reason: ExpiryReason::Ttl,
        },
        JournalRecord::ApprovalAmended {
            id: ApprovalId::new("y"),
            amended_effect: effect(),
            at_millis: 7,
        },
        JournalRecord::ApprovalGranted {
            grant: grant("z", 11),
        },
        JournalRecord::GrantConsumed {
            id: ApprovalId::new("z"),
            effect: None,
        },
        JournalRecord::GrantConsumed {
            id: ApprovalId::new("z2"),
            effect: Some(executed(21)),
        },
        JournalRecord::GrantExpired {
            id: ApprovalId::new("z"),
            at_millis: 13,
        },
    ] {
        let json = serde_json::to_value(&record).unwrap();
        assert!(json.get("record").is_some());
        let back: JournalRecord = serde_json::from_value(json).unwrap();
        // Re-serialize to compare (JournalRecord has no PartialEq).
        assert_eq!(
            serde_json::to_string(&back).unwrap(),
            serde_json::to_string(&record).unwrap()
        );
    }
}

/// **Issue #392**: the host-durable set is a policy, and this pins it.
///
/// The wildcard-free match in [`JournalRecord::durability`] already makes
/// *completeness* a compile error — a new variant will not build until it is
/// classified. What no compiler can catch is an existing kind being moved
/// across the line: flipping `EffectExecuted` to `Process` compiles, passes
/// every other test in this file, and silently gives up the one guarantee
/// the journal exists for. This is the test that notices.
///
/// `every_record_kind`'s `ApprovalParked` sample carries an ordinary effect,
/// so it belongs on the `Process` side here. Its workflow-gate arm is the
/// one kind whose level depends on contents rather than tag, and it is
/// pinned separately below (issue #1145) — deliberately not by loosening
/// this list, which is the assertion that would have stopped noticing.
#[test]
fn host_durable_kinds_are_exactly_the_ten_that_protect_approval_work() {
    let all = every_record_kind();
    let tags: HashSet<String> = all.iter().map(record_tag).collect();
    assert_eq!(
        tags.len(),
        22,
        "every JournalRecord variant must appear once in every_record_kind"
    );

    let mut host: Vec<String> = all
        .iter()
        .filter(|record| record.durability() == Durability::Host)
        .map(record_tag)
        .collect();
    host.sort();
    assert_eq!(
        host,
        vec![
            "ApprovalContinuationDispatched".to_string(),
            "ApprovalContinuationQueued".to_string(),
            "BlockedNodeApproved".to_string(),
            "BlockedNodeDispatched".to_string(),
            "BlockedNodeReleased".to_string(),
            "BlockedNodeStashed".to_string(),
            "EffectExecuted".to_string(),
            "GrantConsumed".to_string(),
            "GrantDispatched".to_string(),
            "StandingGrantRevoked".to_string()
        ],
        "the host-durable set is these ten kinds and nothing else; \
         widening it taxes the hot path, narrowing it lets an effect duplicate, \
         a spent grant re-arm, an explicit follow-up repeat, or a blocked node's \
         stash/approval/dispatch survive a process restart but not the host crash it also \
         promises to survive"
    );
}

/// **Issue #1145.** A workflow gate's park is host-durable; every other park
/// is not.
///
/// Both arms in one test because the assertion *is* the distinction. The
/// `Host` half alone would pass if every park were flushed — taxing the
/// journal's approval path to fix one caller — and the `Process` half alone
/// would pass on the code this replaces.
///
/// Why the gate is different: a paused workflow run is *settled*, not
/// suspended, so nothing re-enters the gate and the parked effect is the
/// run's only continuation. A chat turn re-parks on its next attempt, which
/// is why its park stays `Process` — and why the volume this record is
/// written at is untouched.
#[test]
fn only_a_workflow_gate_park_is_host_durable() {
    assert_eq!(
        parked_with_kind(crate::runtime::WORKFLOW_APPROVE_KIND).durability(),
        Durability::Host,
        "a workflow gate's park is the run's only continuation — losing it \
         strands the run behind a question that exists nowhere, and nothing \
         re-parks it"
    );

    // The callers that do re-park, and the volume this record is written at.
    for kind in ["shell", "http.request", "message.send", "composio_execute"] {
        assert_eq!(
            parked_with_kind(kind).durability(),
            Durability::Process,
            "{kind} parks are re-asked on the next attempt; flushing them \
             taxes the approval path for a question that comes back on its own"
        );
    }
}
