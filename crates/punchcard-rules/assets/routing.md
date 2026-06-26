## Classify (before any tool)

Pick the **shallowest** tier that stays correct.

1. **Trivial → Direct edit** — one named file, literal edit under ~5 lines, no observable contract (API/error/log shape, persistence, auth, flags, migrations, public config, validation). Read the named file; **no Punchcard MCP, no broad Grep**.
2. **Focused → Discover** — target file(s) named but a hypothesis must be validated, blast radius is unclear, or a review asks to prove/disprove. **`get_context` once** before Read/Grep.
3. **Enriched → Discover** — any Enriched signal below. **`get_context` before broad Read/Grep.**
4. **Implement** — material validated code or docs change: Discover (unless Trivial) → govern change.
5. **Ambiguous** — `get_context({ task, hints? })` once, then source.

**Not Trivial** even with a named path: review hypotheses; logging/errors; cross-module edits; behavioral proof.

**Enriched signals** (any): refactor / feature / integrate / retrocompat / architecture; contracts or flags; more than 3 modules or unknown blast radius; active cards; debug / plan; analysis beyond a one-line swap.

Unsure Focused vs Enriched → Enriched. Subagents: parent sets tier once; no duplicate retrieval. Routes describe **how much Punchcard to use**, not where code runs.

## Discovery precedence (before Read/Grep)

When Discover applies, **do not** open with repo-wide Grep:

1. `get_context({ task, hints? })` when Focused or Enriched
2. `read_doc` / `read_memory` on deck refs to plan what/where/which files
3. **CodeGraph** when `.codegraph/` exists (`codegraph_explore`, `codegraph_node`)
4. **Read** only files from that plan
5. **Grep** one concrete gap only after 1–4

If you need multiple files and lack a deck-informed file list, return to step 1.

Punchcard deck precedes CodeGraph; CodeGraph precedes Grep.

## Tier reference

| Tier | When | Punchcard |
|---|---|---|
| **Trivial** | Single-file literal edit, no observable contract | None |
| **Focused** | Named targets but hypothesis/blast radius open | `get_context` once |
| **Enriched** | Any Enriched signal | `get_context` first |

## Route reference

| Route | Tier | Actions |
|---|---|---|
| **Direct edit** | Trivial | Read the named file |
| **Discover** | Focused / Enriched | precedence above; if a deck ref answers a docs question, stop |
| **Implement** | Not Trivial | Discover → govern change |

**Retrieval budget:** one `get_context` per task; prefer `read_doc` / `read_memory` on known refs before new searches; no repeat `get_context` or rephrase-pad searches.

**Governed:** `start_change` **Evidence** cites deck/memory when Discover ran; `run_validation` each required name from `.punchcard/config.toml` before `save_memory`.

**Micro-change** (one file, ~15 lines or fewer, no new API/invariant): still `start_change` → validations → `save_memory`; skip `open_task` and heavy session ceremony unless promoting durable memory or the user asked for it.
