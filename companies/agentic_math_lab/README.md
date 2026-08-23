# Agentic Math Lab

A company that answers computational mathematics problems and can show its
work. Its acceptance test is Project Euler: problems stated in a paragraph,
answered by one exact integer, hard enough that the answer has to be computed.

- **The roster** is a split, not a hierarchy: the theorist decides the approach
  and never reports the answer, the programmer writes and runs and reports only
  what printed, the verifier writes a second route without reading the first,
  the scribe records what survived. The lead does none of it.
- **The rule** is that an answer is two independent routes agreeing. One
  program's output is a result; the difference is that somebody tried to break
  it.
- **No `web`, no `search`.** Withheld deliberately: a lab that can look the
  answer up proves nothing by producing it. This is a nudge, not a boundary —
  `shell` is granted and there is no network sandbox
  (`docs/spec/security/agent-isolation.md`), so what it removes is the tool an
  agent reaches for first. The claim is carried by the program on disk.

Run it:

```bash
cargo run --bin opencompany -- serve --company companies/agentic_math_lab
```

The end-to-end proof is `frontend/test/e2e/euler-live.spec.ts`, which states a
problem in the main line against a real model and checks the integer the lab
reaches against the published one.
