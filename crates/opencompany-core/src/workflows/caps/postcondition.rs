//! The deterministic tier of issue #1866's sufficiency gate.
//!
//! The engine advances the moment a node returns `Ok`; nothing checks whether
//! the output is actually enough to hand downstream. The extreme case is
//! already fixed independently for the iteration-cap signal (#1865): a node
//! that stops at `max_tool_iterations` settles `Failed` rather than flowing a
//! truncated reply on as if it were a finished answer. This module is the
//! general form of that same idea, expressed as an author-declared,
//! **mechanical** check rather than a signal the engine happens to expose —
//! "the output has this shape, or it isn't good enough to advance."
//!
//! Deliberately narrow: three predicates, no LLM call, no network, no state.
//! The semantic judge tier the issue also describes (a tool-less model call
//! for nodes whose sufficiency cannot be expressed as a predicate) is Wave 3,
//! gated on #1861's blocker-park plumbing landing first — this module only
//! ever returns `Ok` or a plain-English gap sentence.
//!
//! # A gate that cannot be evaluated fails the node
//!
//! [`evaluate_postcondition`] is validated at author time
//! ([`crate::company::workflow_file::validate`] rejects an unknown `require`
//! before a graph is ever saved), so an unrecognized `require` reaching this
//! function at runtime can only mean a graph saved by an older or newer
//! version of the validator disagreeing with this binary.
//!
//! A postcondition is not observability — it is the author saying "this
//! output is not good enough to hand downstream unless it has this shape".
//! Advancing on a `require` this build cannot check does not skip a
//! measurement, it silently deletes the check: the node flows its output on
//! with its declared quality gate having done nothing, and the run reads as
//! healthy. So an unrecognized `require` FAILS the node, with a gap sentence
//! naming the predicate and the version disagreement behind it. An author who
//! meant the check to be optional expresses that by not declaring it.
//!
//! `field_present` losing its own `field` fails closed for a related reason.
//! `validate` requires every `field_present` postcondition to carry a
//! non-empty `field`, so this function is never handed one the validator
//! approved without it — a `field_present` spec reaching here with a
//! missing/non-string `field` means something rewrote a validated value
//! between save and this call (issue #1937/#1866: the whole `postcondition`
//! rides inside the engine-resolved node config, so an authored `field =
//! "=item.missing"` — a plausible mistake, since `=`-expressions are the
//! normal syntax everywhere else in config — gets evaluated by the SAME
//! generic config resolution as any other value, and a miss resolves to
//! `null` indistinguishably from "no field was ever authored"). Unlike an
//! unrecognized `require`, this is not "a predicate this binary cannot
//! evaluate" — `field_present` is fully understood here, it just has nothing
//! left to check. Passing it through anyway would silently switch the whole
//! gate off for exactly the graphs that most need it caught, so this one
//! case fails CLOSED instead: see the `field_present` arm below.

use serde_json::Value;

/// Evaluates a node's declared `postcondition` against its output envelope.
///
/// `spec` is the raw `postcondition` config node ({ "require": ..., "field":
/// ... (optional) }); `output` is the node's output value — for an agent node
/// today, the `{ "text", "agent_ref" }` envelope [`super::HarnessAgentRunner::run_turn`]
/// builds. `Ok(())` means the output clears the gate; `Err(gap)` carries a
/// plain-English sentence naming what is missing, suitable to surface as the
/// halting attempt's error message.
///
/// Three predicates:
/// - `non_empty` — the envelope's `text` is present and non-empty after
///   trimming whitespace. Catches the truncation class this issue opens
///   with: a capped or refused turn that still produced *some* prose.
/// - `field_present` — the dotted `field` path resolves to a present,
///   non-null value in `output`. If `field` itself did not resolve to a
///   usable name (a validated postcondition's own `field` going missing/
///   non-string between save and this call), this fails CLOSED rather than
///   passing the node through — see the module doc's second section.
/// - `non_empty_list` — the target (the whole `output`, or the dotted
///   `field` path within it when given) is a JSON array with at least one
///   element.
///
/// Any OTHER `require` value (one this function does not recognize at all)
/// fails CLOSED: the node is failed with a gap naming the predicate this
/// build cannot check. See the module doc for why advancing instead would
/// silently delete an authored gate.
pub(crate) fn evaluate_postcondition(spec: &Value, output: &Value) -> Result<(), String> {
    let require = spec.get("require").and_then(Value::as_str).unwrap_or("");
    let field = spec
        .get("field")
        .and_then(Value::as_str)
        .filter(|f| !f.is_empty());

    match require {
        "non_empty" => {
            let text = output.get("text").and_then(Value::as_str).unwrap_or("");
            if text.trim().is_empty() {
                Err("the node's output was empty — nothing was produced to advance on.".to_string())
            } else {
                Ok(())
            }
        }
        "field_present" => {
            let Some(path) = field else {
                // `workflow_file::validate` REQUIRES a `field` on every
                // `field_present` postcondition it ever saves, so a spec
                // reaching here with no usable `field` cannot be a graph the
                // validator approved as written — it means something
                // rewrote a validated `field` into null/non-string between
                // save and this call (an authored `=`-expression resolved
                // away by config resolution is the concrete case that
                // motivated this; see `workflows::caps::tests_field_present::
                // a_field_resolved_away_by_an_authored_expression_fails_closed_at_run_turn`).
                // `field_present`'s entire job is checking that one named
                // field exists — evaluating it with no field to check is
                // failing the very thing it exists to verify, not an
                // ambiguous no-op. Fail CLOSED: an authored safety gate must
                // never be silently switched off by a resolution quirk this
                // binary did not foresee.
                tracing::warn!(
                    require,
                    "workflow postcondition: `field_present` declared but its `field` did \
                     not resolve to a name to check — failing the node rather than silently \
                     passing an unverifiable gate"
                );
                return Err(
                    "the node's postcondition declares `field_present` but its `field` did \
                     not resolve to a name to check — refusing to advance rather than \
                     silently pass an unverifiable gate."
                        .to_string(),
                );
            };
            match resolve_path(output, path) {
                // Codex #3894162757 on #1937 — a bare scalar under the exact
                // `json` root is the one shape `field_present` must refuse
                // to certify even though it genuinely resolves. `path ==
                // "json"` means the author is checking the WHOLE parsed
                // reply, which is exactly what a downstream `=item.json`
                // binding reads too — but tinyflows' own envelope
                // construction (`finish_agent_run`/`envelope::structured_of`
                // in the vendored engine) normalizes anything that is not an
                // `Object`/`Array` to `Value::Null` on the way to that
                // binding. Certifying a scalar here would pass a gate whose
                // value the workflow can never actually read — the same
                // "gate certifies X, item never gets X" defect this whole
                // module exists to close, just reached through a shape that
                // resolves cleanly rather than one that resolves to null.
                // A dotted path UNDER `json` (`json.count`) is unaffected:
                // reaching a scalar there means the reply was already an
                // object, which merges into the emitted value intact (see
                // `HarnessAgentRunner::run_turn`'s `Value::Object` arm), so
                // `item.json.count` really does resolve downstream.
                Some(value) if path == "json" && is_bare_scalar(value) => Err(format!(
                    "the node's output under `json` is a bare scalar ({value}) — a \
                     downstream `=item.json` binding can never see it (tinyflows only \
                     carries an object or array through `json`; anything else \
                     normalizes to null), so this postcondition can never certify a \
                     value the workflow can actually read. Have the agent reply with \
                     an object instead naming it (e.g. `{{\"value\": ...}}`) and target \
                     the dotted path (`field = \"json.value\"`)."
                )),
                Some(value) if !value.is_null() => Ok(()),
                _ => Err(format!(
                    "the node's output is missing `{path}` — the expected field never landed."
                )),
            }
        }
        "non_empty_list" => {
            let target = match field {
                Some(path) => resolve_path(output, path),
                // No `field` given: for the standard `{ json, text, raw }`
                // envelope, "the output" means the structured payload under
                // `json`, not the envelope wrapper itself — the wrapper is
                // always an object (it also carries `text`/`agent_ref`), so
                // checking it directly could never see a `Value::Array` even
                // when the underlying result genuinely is a list. Falls back
                // to the raw value for any caller not using that envelope
                // shape (no `json` key at all), which is the pre-existing
                // behavior.
                None => output.get("json").or(Some(output)),
            };
            let described = field
                .map(|path| format!("`{path}`"))
                .unwrap_or_else(|| "the output".to_string());
            match target {
                Some(Value::Array(items)) if !items.is_empty() => Ok(()),
                Some(Value::Array(_)) => Err(format!(
                    "{described} is an empty list — nothing came back to advance on."
                )),
                Some(_) => Err(format!(
                    "{described} is not a list — the shape does not match."
                )),
                None => Err(format!(
                    "{described} is missing — nothing came back to advance on."
                )),
            }
        }
        other => {
            tracing::error!(
                require = other,
                "workflow postcondition: unrecognized `require` — failing the node rather than \
                 advancing on a declared gate this binary cannot evaluate"
            );
            Err(format!(
                "the node's postcondition requires `{other}`, which this build does not know how \
                 to check — refusing to advance rather than silently pass an unevaluated gate. \
                 The graph was saved by a build whose validator recognizes predicates this one \
                 does not."
            ))
        }
    }
}

/// Resolves a dot-separated path (`"a.b.c"`) through nested JSON objects.
/// Does not index into arrays — every hop is an object-field lookup, which is
/// all the two field-aware predicates above need.
fn resolve_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(value, |acc, key| acc.get(key))
}

/// True for a JSON value with no structure of its own — a bool, number, or
/// string. Used by the `field_present`-on-bare-`json` check: these are the
/// shapes tinyflows' own envelope construction discards (normalizes to
/// `Value::Null`) rather than carries through to a downstream `=item.json`
/// binding. `Null` is deliberately excluded — it is handled by the ordinary
/// "missing" branch above this call, not this one.
fn is_bare_scalar(value: &Value) -> bool {
    matches!(value, Value::Bool(_) | Value::Number(_) | Value::String(_))
}

#[cfg(test)]
#[path = "postcondition_tests.rs"]
mod tests;
