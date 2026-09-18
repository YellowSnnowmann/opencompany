use super::tests_core::*;

// -----------------------------------------------------------------------
// What is, and is not, a mention
// -----------------------------------------------------------------------

#[test]
fn a_roster_id_after_whitespace_is_a_mention() {
    let found = resolve_text("hey @engineer can you look?");
    assert_eq!(targets(&found), vec![&agent("engineer")]);
    assert_eq!(found[0].text, "@engineer");
    assert_eq!(found[0].offset, 4);
    assert!(!found[0].quiet);
}

#[test]
fn a_mention_at_the_very_start_resolves() {
    let found = resolve_text("@engineer ping");
    assert_eq!(targets(&found), vec![&agent("engineer")]);
    assert_eq!(found[0].offset, 0);
}

/// The single most important negative case: an email address contains an
/// `@` followed by something that can look exactly like a roster id.
#[test]
fn an_email_address_is_not_a_mention() {
    assert!(resolve_text("write to jane@engineer.com about it").is_empty());
    assert!(resolve_text("engineer@acme.test").is_empty());
}

#[test]
fn trailing_punctuation_does_not_break_a_mention() {
    for text in ["@engineer, thoughts?", "ask @engineer.", "(@engineer)"] {
        let found = resolve_text(text);
        assert_eq!(targets(&found), vec![&agent("engineer")], "text: {text}");
        assert_eq!(found[0].text, "@engineer", "text: {text}");
    }
}

/// `@engineering` must not resolve to the `engineer` teammate — a mention
/// has to end on a boundary, or every longer name becomes a misroute.
#[test]
fn a_longer_word_does_not_resolve_to_a_shorter_id() {
    let found = resolve_text("the @engineerish thing");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_unknown_name_resolves_to_nothing() {
    assert!(resolve_text("@nobody are you there").is_empty());
}

// -----------------------------------------------------------------------
// Code regions
// -----------------------------------------------------------------------

#[test]
fn a_mention_inside_an_inline_code_span_is_not_a_mention() {
    assert!(resolve_text("run `@engineer --help` first").is_empty());
}

#[test]
fn a_mention_inside_a_fenced_block_is_not_a_mention() {
    let text = "before\n```\n@engineer\n```\nafter";
    assert!(resolve_text(text).is_empty());
}

/// A line like ```not-a-close is code, not a closing fence: CommonMark
/// only lets a fence be followed by spaces or tabs. Closing the mask there
/// would unmask a later `@engineer` the renderer still shows as code.
#[test]
fn a_false_closing_fence_does_not_unmask_a_later_mention() {
    let text = "before\n```\ncode\n```not-a-close\n@engineer\n```\nafter";
    assert!(resolve_text(text).is_empty());
}

/// Trailing whitespace on a closing fence is still a valid close
/// (CommonMark allows spaces or tabs), so the block keeps masking.
#[test]
fn a_fence_closed_with_trailing_whitespace_still_masks() {
    let text = "before\n```\n@engineer\n```  \nafter";
    assert!(resolve_text(text).is_empty());
}

/// A CRLF line ending is still a close — the `\r` is part of the ending,
/// not fence text, and must keep closing the block as it did before the
/// suffix was restricted.
#[test]
fn a_fence_closed_over_crlf_still_masks() {
    let text = "before\n```\n@engineer\n```\r\nafter";
    assert!(resolve_text(text).is_empty());
}

/// The reason [`strip_code_regions`] blanks rather than removes: a mention
/// *after* a code span must still land on its real byte offset.
#[test]
fn offsets_survive_a_masked_code_span() {
    let text = "`@ceo` but really @engineer";
    let found = resolve_text(text);
    assert_eq!(targets(&found), vec![&agent("engineer")]);
    let m = &found[0];
    assert_eq!(&text[m.offset..m.offset + m.text.len()], "@engineer");
}

#[test]
fn masking_preserves_length_and_newlines() {
    let text = "a\n```\nxx\n```\nb `y` c";
    let masked = strip_code_regions(text);
    assert_eq!(masked.len(), text.len());
    assert_eq!(
        masked.matches('\n').count(),
        text.matches('\n').count(),
        "newlines are kept so line structure survives"
    );
    assert!(!masked.contains("xx"));
    assert!(masked.starts_with("a\n"));
}

/// An unbalanced backtick is not a code span, and must not swallow the rest
/// of the message.
#[test]
fn an_unclosed_backtick_does_not_mask_the_rest() {
    let found = resolve_text("weird ` tick then @engineer");
    assert_eq!(targets(&found), vec![&agent("engineer")]);
}

/// A longer closing run cannot close a shorter opener: CommonMark only lets
/// a *whole* run of exactly the opening length close a span, so
/// `` `code @engineer here`` `` (one opener, two trailing) is not code and
/// the mention — which opens after a space and closes before one — must
/// resolve. The console's mask has to agree with this or it would suppress
/// a mention the renderer still shows. (`` `@engineer`` `` does *not*
/// resolve either side: an `@` right after a backtick is not a
/// mention-opening position.)
#[test]
fn a_longer_backtick_run_does_not_close_a_shorter_opener() {
    let found = resolve_text("`code @engineer here``");
    assert_eq!(targets(&found), vec![&agent("engineer")]);
}

// -----------------------------------------------------------------------
// Longest-alias-wins, and ambiguity
// -----------------------------------------------------------------------

/// `@Jane Doe` has a space in it and is only reachable because the matcher
/// tries the whole display name, longest first.
#[test]
fn a_two_word_display_name_resolves() {
    let found = resolve_text("thanks @Jane Doe!");
    assert_eq!(
        targets(&found),
        vec![&MentionTarget::User {
            id: "u1".to_string()
        }]
    );
    assert_eq!(found[0].text, "@Jane Doe");
}

#[test]
fn a_slug_also_reaches_a_person() {
    let found = resolve_text("thanks @jane-doe");
    assert_eq!(
        targets(&found),
        vec![&MentionTarget::User {
            id: "u1".to_string()
        }]
    );
}

#[test]
fn a_person_with_no_display_name_is_reachable_by_their_local_part() {
    let found = resolve_text("@sam what do you think");
    assert_eq!(
        targets(&found),
        vec![&MentionTarget::User {
            id: "u2".to_string()
        }]
    );
}

/// One member's name prefixing another's is the case that silently
/// misroutes if the matcher takes the first match rather than the longest.
#[test]
fn the_longest_alias_wins() {
    let users = vec![
        user("u1", "ann@acme.test", Some("Ann")),
        user("u2", "annlee@acme.test", Some("Ann Lee")),
    ];
    let found = resolve("ping @Ann Lee now", None, None, &acme(), &users);
    assert_eq!(
        targets(&found),
        vec![&MentionTarget::User {
            id: "u2".to_string()
        }],
        "the longer name must win, or Ann Lee can never be mentioned"
    );
}

/// Never guess a ping.
#[test]
fn an_ambiguous_name_resolves_to_nobody() {
    let users = vec![
        user("u1", "sam.a@acme.test", Some("Sam")),
        user("u2", "sam.b@acme.test", Some("Sam")),
    ];
    let found = resolve("hey @Sam", None, None, &acme(), &users);
    assert!(
        found.is_empty(),
        "two people share this name, so it must stay literal text: {found:?}"
    );
}

/// …and says so (B-101).
///
/// The refusal above is right and stays. What was missing is that it was
/// announced only by the absence of a chip — invisible in a wall of text,
/// and for an API poster, who renders no chips at all, not a signal in any
/// sense. A founder's `@Priya` reached neither the teammate nor the person
/// of that name and nothing told them; the channel catch-all answered and
/// spoke about her in the third person.
#[test]
fn an_ambiguous_name_is_reported_with_what_it_collided_with() {
    // The reported shape exactly: a roster teammate and a human member who
    // share one spelling.
    let record = record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"priya\"\nrole = \"Merchandiser\"\n",
    );
    let users = vec![user("u1", "priya@acme.test", Some("Priya"))];
    let found = resolve_reporting(
        "@Priya which scent sold best last month?",
        None,
        None,
        &record,
        &users,
    );

    assert!(found.mentions.is_empty(), "still pings nobody");
    assert_eq!(found.ambiguous.len(), 1, "and the refusal is reported once");
    let refused = &found.ambiguous[0];
    assert_eq!(refused.text, "@Priya", "the literal the sender typed");
    assert_eq!(refused.offset, 0);
    assert_eq!(
        refused.targets,
        vec![
            MentionTarget::Agent {
                id: "priya".to_string()
            },
            MentionTarget::User {
                id: "u1".to_string()
            },
        ],
        "both claimants, so the note can say what the collision was"
    );

    let note = ambiguity_note(&found.ambiguous).expect("a refusal produces a note");
    assert!(note.contains("@Priya"), "names the literal: {note}");
    assert!(note.contains("a teammate"), "names the kinds: {note}");
    assert!(note.contains("a person"), "names the kinds: {note}");
    assert!(
        note.contains("pinged nobody"),
        "states the consequence: {note}"
    );
}

/// The other two wordings [`ambiguity_note`] can produce, both reachable
/// from ordinary company state and neither exercised by the test above
/// (coderabbit on #2161).
///
/// **Two people called Sam** is the commonest collision there is, and it is
/// the one case where naming the kinds says nothing: both claimants are
/// `a person`, so "matches a person and a person here" would be a sentence
/// that reads like a bug. The equal-noun arm collapses it to "two of
/// these" instead, and that arm is only reachable when the nouns match.
///
/// **Two refused spans in one message** switches both the head and the
/// subject — "each match", "they" rather than "it" — because the sentence
/// is now about a list. A regression in either is invisible until someone
/// reads a live note.
#[test]
fn the_collapsed_and_plural_wordings_are_produced_too() {
    let record = record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"priya\"\nrole = \"Merchandiser\"\n",
    );
    // Two Sams for the equal-noun arm, and a Priya who collides with the
    // `priya` teammate so the message carries a *second* refusal.
    let users = vec![
        user("u1", "sam.a@acme.test", Some("Sam")),
        user("u2", "sam.b@acme.test", Some("Sam")),
        user("u3", "priya@acme.test", Some("Priya")),
    ];
    let found = resolve_reporting(
        "@Sam and @Priya — who owns this?",
        None,
        None,
        &record,
        &users,
    );

    assert!(found.mentions.is_empty(), "both spans ping nobody");
    assert_eq!(found.ambiguous.len(), 2, "{:?}", found.ambiguous);

    // The plural head, over both spans.
    let both = ambiguity_note(&found.ambiguous).expect("a refusal produces a note");
    assert!(both.contains("@Sam"), "names both literals: {both}");
    assert!(both.contains("@Priya"), "names both literals: {both}");
    assert!(
        both.contains("each match more than one thing here"),
        "the plural head: {both}"
    );
    assert!(
        both.contains("they pinged nobody"),
        "a list takes the plural subject: {both}"
    );

    // The same first span alone takes the equal-noun arm, because two
    // people share one noun.
    let sam = ambiguity_note(&found.ambiguous[..1]).expect("note");
    assert!(
        sam.contains("matches two of these here"),
        "two claimants of one kind collapse rather than repeating the noun: {sam}"
    );
    assert!(
        !sam.contains("a person and a person"),
        "which is the whole point of the arm: {sam}"
    );
    assert!(
        sam.contains("it pinged nobody"),
        "one span takes the singular subject: {sam}"
    );
}

/// The negative half. Nothing is reported for a message whose names all
/// resolve, or for one that names nobody at all — otherwise the notice
/// would fire on every message and be worth nothing.
#[test]
fn an_unambiguous_message_reports_no_refusal() {
    for body in [
        "hey @engineer, can you look?",
        "no names in this one at all",
    ] {
        let found = resolve_reporting(body, None, None, &acme(), &people());
        assert!(
            found.ambiguous.is_empty(),
            "{body:?} refused nothing: {:?}",
            found.ambiguous
        );
    }
    assert_eq!(ambiguity_note(&[]), None, "and an empty list posts nothing");
}

/// A caller that ran a picker has already disambiguated — its two rows are
/// the two colliding targets, and the click said which. A span the picker
/// resolved is not reported: the operator was shown the two colliding rows
/// and clicked one, so telling them it reached nobody would contradict what
/// they just did.
#[test]
fn a_span_the_picker_resolved_is_not_reported() {
    let record = record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"priya\"\nrole = \"Merchandiser\"\n",
    );
    let users = vec![user("u1", "priya@acme.test", Some("Priya"))];
    let supplied = vec![Mention {
        target: MentionTarget::User {
            id: "u1".to_string(),
        },
        text: "@Priya".to_string(),
        offset: 0,
        quiet: false,
    }];
    let found = resolve_reporting("@Priya which scent?", Some(supplied), None, &record, &users);
    assert!(
        found.ambiguous.is_empty(),
        "the click settled it: {:?}",
        found.ambiguous
    );
    assert_eq!(found.mentions.len(), 1, "the pick still resolves");
}

/// A picker-supplied mention `revalidate` demoted to `quiet` (its target
/// renamed, removed, or reassigned since the picker ran) must not suppress
/// an ambiguity the live scan finds at the very same span (Codex review,
/// PR #2052).
///
/// `revalidate` keeps a stale mention in `found` rather than dropping it —
/// `quiet` is what says "typed, but delivered to nobody" — so `claimed`
/// built from every entry of `found` counted this span as settled even
/// though nothing was actually pinged. If the old alias now names two live
/// targets, that produced the exact silent failure B-101 exists to catch:
/// the picker's answer reaches nobody (quiet), and the ambiguity the scan
/// found at the same span goes unreported (falsely claimed) — an operator
/// watching the transcript sees nothing say their `@name` went nowhere.
#[test]
fn a_demoted_picker_answer_does_not_suppress_the_scan_s_own_ambiguity() {
    // Two live targets share the alias "Priya" — the same collision
    // `an_ambiguous_name_is_reported_with_what_it_collided_with` uses.
    let record = record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"priya\"\nrole = \"Merchandiser\"\n",
    );
    let users = vec![user("u1", "priya@acme.test", Some("Priya"))];
    // The picker supplied a THIRD, now-gone user at this exact span — not
    // one of the two live claimants above. `revalidate` cannot find it on
    // the roster, so it survives only as `quiet: true`.
    let supplied = vec![Mention {
        target: MentionTarget::User {
            id: "u-departed".to_string(),
        },
        text: "@Priya".to_string(),
        offset: 0,
        quiet: false,
    }];
    let found = resolve_reporting("@Priya which scent?", Some(supplied), None, &record, &users);

    assert_eq!(found.mentions.len(), 1, "the stale pick is kept, demoted");
    assert!(found.mentions[0].quiet, "and pings nobody");
    assert_eq!(
        found.ambiguous.len(),
        1,
        "the live scan's own collision at the same span must still be reported: {:?}",
        found.ambiguous
    );
    assert_eq!(found.ambiguous[0].text, "@Priya");
}

/// The case the console actually produces, and the one that made this bug
/// invisible from the surface it was reported on.
///
/// `resolvableMentions` in the console runs the same refusal rule, and a
/// loaded directory then sends `[]` explicitly to suppress host extraction
/// ("Preserve absent-versus-empty", `MessageComposer`). So a free-typed
/// `@Priya` arrives here as "the picker resolved nothing" — indistinguishable
/// from a message that named nobody — and reporting only on the extraction
/// path left the console exactly as silent as before.
#[test]
fn an_empty_picker_answer_still_reports_what_the_scan_refuses() {
    let record = record(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"priya\"\nrole = \"Merchandiser\"\n",
    );
    let users = vec![user("u1", "priya@acme.test", Some("Priya"))];
    let found = resolve_reporting(
        "@Priya which scent sold best last month?",
        Some(Vec::new()),
        None,
        &record,
        &users,
    );
    assert!(found.mentions.is_empty(), "routing is unchanged: nobody");
    assert_eq!(
        found.ambiguous.len(),
        1,
        "but the refusal is the host's to report, whoever asked"
    );
    assert_eq!(found.ambiguous[0].text, "@Priya");
}

/// …and an empty picker answer over a message that names nobody at all
/// still reports nothing, so the notice cannot become background noise.
#[test]
fn an_empty_picker_answer_over_a_plain_message_reports_nothing() {
    let found = resolve_reporting(
        "which scent sold best last month?",
        Some(Vec::new()),
        None,
        &acme(),
        &people(),
    );
    assert!(found.mentions.is_empty());
    assert!(found.ambiguous.is_empty());
}

#[test]
fn colliding_slugs_are_disambiguated_in_order() {
    let users = vec![
        user("u1", "a@acme.test", Some("Sam")),
        user("u2", "b@acme.test", Some("Sam")),
        user("u3", "c@acme.test", Some("Sam")),
    ];
    assert_eq!(user_slugs(&users), vec!["sam", "sam-2", "sam-3"]);
}

/// A natural display name can already look like a generated
/// disambiguation (`"Sam-2"` is a real name someone can type at signup),
/// and the counter must still notice it collided with the second `Sam`
/// rather than handing both people the same slug.
#[test]
fn a_natural_label_matching_a_generated_suffix_does_not_collide() {
    let users = vec![
        user("u1", "a@acme.test", Some("Sam")),
        user("u2", "b@acme.test", Some("Sam")),
        user("u3", "c@acme.test", Some("Sam-2")),
    ];
    let slugs = user_slugs(&users);
    assert_eq!(
        slugs.len(),
        slugs.iter().collect::<std::collections::HashSet<_>>().len(),
        "every emitted slug must be unique: {slugs:?}"
    );
}

#[test]
fn a_label_with_nothing_typable_yields_an_empty_slug() {
    assert_eq!(mention_slug("!!!"), "");
    assert_eq!(mention_slug("Ana  M. Ruiz"), "ana-m-ruiz");
}

/// A symbol-only display name ("🙂") slugs to nothing, which would leave
/// the person unmentionable while the picker still advertises a row. The
/// fallback must hand such a user a real, typable alias — the email local
/// part, then the id — so the picker can insert a spelling the host's
/// `opens_mention` accepts and the directory can resolve.
#[test]
fn a_symbol_only_display_name_still_gets_a_typable_slug() {
    let users = vec![
        user("u1", "smiley@acme.test", Some("🙂")),
        user("u2", "no_name@acme.test", Some("!!!")),
        user("u3", "plain@acme.test", Some("Ada")),
    ];
    assert_eq!(user_slugs(&users), vec!["smiley", "no-name", "ada"]);
}

// -----------------------------------------------------------------------
// Desks and everyone
// -----------------------------------------------------------------------

#[test]
fn a_desk_resolves_by_id_and_by_name() {
    for text in ["@engineering please", "@Engineering please"] {
        let found = resolve_text(text);
        assert_eq!(
            targets(&found),
            vec![&MentionTarget::Desk {
                id: "engineering".to_string()
            }],
            "text: {text}"
        );
    }
}

/// `MentionTarget::Desk`'s own doc comment advertises `@#engineering` as
/// the desk spelling; extraction must actually accept it, and a
/// client-supplied mention using it must revalidate as live rather than
/// being demoted for a text/target mismatch.
#[test]
fn a_hash_prefixed_desk_mention_resolves() {
    let found = resolve_text("@#engineering please");
    assert_eq!(
        targets(&found),
        vec![&MentionTarget::Desk {
            id: "engineering".to_string()
        }]
    );
    assert_eq!(found[0].text, "@#engineering");

    let supplied = vec![Mention {
        target: MentionTarget::Desk {
            id: "engineering".to_string(),
        },
        text: "@#engineering".to_string(),
        offset: 0,
        quiet: false,
    }];
    let out = resolve(
        "@#engineering please",
        Some(supplied),
        None,
        &acme(),
        &people(),
    );
    assert_eq!(out.len(), 1);
    assert!(!out[0].quiet, "{out:?}");
}

/// `@#` naming a non-desk alias must not resolve — the `#` is desk-only,
/// so `@#engineer` (an agent id) must stay literal text rather than
/// silently falling back to the unprefixed match.
#[test]
fn a_hash_prefixed_non_desk_alias_does_not_resolve() {
    let found = resolve_text("@#engineer please");
    assert!(found.is_empty(), "{found:?}");
}

/// The same desk-only rule applies when a structured caller supplies the
/// span: `@#engineer` paired with the agent target must not ping them.
/// `is_valid_alias_for` strips the hash for the alias comparison, so
/// without the kind check here the `#`-shaped mention would validate.
#[test]
fn a_hash_prefixed_non_desk_target_is_demoted_in_revalidation() {
    let supplied = vec![Mention {
        target: agent("engineer"),
        text: "@#engineer".to_string(),
        offset: 0,
        quiet: false,
    }];
    let out = resolve(
        "@#engineer please",
        Some(supplied),
        None,
        &acme(),
        &people(),
    );
    assert_eq!(out.len(), 1);
    assert!(out[0].quiet, "{out:?}");
}

#[test]
fn every_everyone_alias_reaches_the_same_target() {
    for alias in EVERYONE_ALIASES {
        let found = resolve_text(&format!("@{alias} heads up"));
        assert_eq!(
            targets(&found),
            vec![&MentionTarget::Everyone],
            "alias: {alias}"
        );
    }
}

// -----------------------------------------------------------------------
// normalize: self, duplicates, the cap
// -----------------------------------------------------------------------

#[test]
fn the_sender_is_not_mentioned_in_their_own_message() {
    let sender = Actor {
        kind: ActorKind::User,
        id: "u1".to_string(),
    };
    let found = resolve(
        "@Jane Doe and @sam",
        None,
        Some(&sender),
        &acme(),
        &people(),
    );
    assert_eq!(
        targets(&found),
        vec![&MentionTarget::User {
            id: "u2".to_string()
        }],
        "a message must not notify its own author"
    );
}
