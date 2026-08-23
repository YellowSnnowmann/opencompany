# Solve loop

How a stated problem becomes a recorded answer. The saved `euler_solve`
workflow is this, as a graph.

1. **Restate.** The theorist says what is being asked, with the boundary cases
   spelled out and the ambiguous readings named. Cheap, and it is where the
   most expensive mistakes are caught.
2. **Cost the naive method.** As a count of operations. If it finishes, skip to
   step 4 — an approach note for a problem a loop settles is make-work.
3. **Reduce.** Name the structure exploited and the cost after it, and produce
   the small-case table. The table is the deliverable, not the prose.
4. **Program.** Reproduce the small cases first, then run to completion. Report
   the number, the wall-clock time, and the exact command.
5. **Check.** The verifier writes a second route from the statement, without
   reading the first program, and brute-forces a reduced bound. It reports the
   method, the command, the number and what it ruled out.
6. **Agree, or find the smallest disagreement.** A disagreement goes back to
   step 3 with the smallest input where the two differ — not to step 4, where
   the repair is to patch until the numbers match and both can end up wrong.
7. **Record.** The scribe writes the note under `Solutions/` and the row on
   [[Answers]]. Failed approaches go on [[Attempts]] with what killed them.

The operator's part is stating the problem and accepting the answer. Everything
between is the lab's.
