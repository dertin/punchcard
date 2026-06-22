Classify each user request before tools. Pick the cheapest route that preserves correctness.

Routes describe **how much Punchcard to use**, not where code runs. None of these mean local machine vs remote environment.

| Route | Meaning | Punchcard tools |
|---|---|---|
| **Source-only** | You already know which files or symbols answer the request; scope is closed | None — open and read source |
| **Discover** | Scope, cause, requirements, or blast radius are still open | `context_prepare`, then `rag_get` / `memory_search` only for deck gaps |
| **Implement** | Discover path plus a material code or doc change that must be recorded as validated project memory | Discover tools, then `change_begin` → `validation_run` for each required name → `change_promote` |

| Request | Signals | Route |
|---|---|---|
| Code or behavior question | Named symbol, file, or closed scope | Source-only if the files are already known; otherwise Discover |
| Small scoped edit | Few files, clear edit target | Source-only if files are known; Implement if the result must be recorded |
| Refactor or multi-file work | Cross-module scope or unclear blast radius | Discover, then Implement |
| Plan or design | User asks for options, phases, or tradeoffs before code | Discover; concise plan only — do not implement until asked |
| Debug or investigate | Symptom, regression, or unknown cause | Discover |
| Review or audit | Explain, review, or compare existing code or docs | Source-only if targets are named; otherwise Discover |
| Subagent delegation | Parent spawns focused workers | Parent classifies once; each subagent gets one bounded goal, route, and stop rules; parent synthesizes; no duplicate retrieval for the same gap |

Decision rules: unsure source-only vs discover → discover; material change that must outlive the session → implement; plan only when the user asks or scope needs multiple decisions; open `change_begin` at implementation start; record every required name with `validation_run` before `change_promote`.
