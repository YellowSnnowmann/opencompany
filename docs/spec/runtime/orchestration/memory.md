# One memory contract

> **Superseded.** This page planned binding OpenCompany's own memory ports
> (`MemoryStore`, `ContextStore`, `FactStore`) to TinyMemory behind a
> host-side tenant decorator. That design was retired in the hard cutover to
> OpenHuman memory v2:
>
> - OpenCompany no longer has memory ports or an engine of its own.
> - Every teammate is an OpenHuman agent bound to its company's root.
> - The host reaches memory only through
>   `openhuman_embed::memory`, wrapped as `crate::memory::CompanyMemory`.
>
> See [../memory-engine.md](../memory-engine.md).

What survives from this plan, and where it now lives:

- **Tenant isolation is structural, not a filter.** The isolation boundary is
  a namespace root per company (`team:<company>`). Every read the facade makes
  carries `Reach::subtree(root)`, and `forget` only removes ids found there.
- **Scrubbing before storage** is OpenHuman's `ScrubbingEngine`, applied to
  every write path.
- **Externally sourced content stays distinguishable.** A brain `context_put`
  from a cycle that outside content triggered is tagged `inbound`
  (`crate::memory::INBOUND_TAG`, issue #1113).
