#!/usr/bin/env bash
# Report Punchcard MCP discover vs govern usage from local tracing logs.
# Use after agent sessions to see whether Enriched routing is being followed.
set -euo pipefail

ROOT="${1:-.}"
LOG="$ROOT/.punchcard/logs/punchcard.jsonl"

if [[ ! -f "$LOG" ]]; then
  echo "No MCP log at $LOG (start the Punchcard MCP server in this repo first)." >&2
  exit 1
fi

python3 - "$LOG" <<'PY'
import json, sys
from collections import Counter

path = sys.argv[1]
counts = Counter()
by_day: dict[str, Counter] = {}

with open(path) as f:
    for line in f:
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        fields = rec.get("fields", {})
        if fields.get("message") != "Punchcard MCP tool completed":
            continue
        tool = fields.get("tool")
        if not tool:
            continue
        counts[tool] += 1
        day = rec.get("timestamp", "")[:10]
        by_day.setdefault(day, Counter())[tool] += 1

discover = {
    "get_context", "search_docs", "read_doc", "check_docs_index",
    "search_memory", "read_memory", "list_memory", "review_memory",
}
govern = {"start_change", "run_validation", "save_memory", "record_change_failure"}
session = {
    "start_session", "end_session", "get_session_context",
    "open_task", "close_task", "save_task_note", "search_task_notes", "summarize_task",
}

total = sum(counts.values()) or 1
d = sum(counts[t] for t in discover)
g = sum(counts[t] for t in govern)
s = sum(counts[t] for t in session)

print(f"MCP tool events: {total}")
print(f"  discover/memory/rag: {d} ({100 * d / total:.0f}%)")
print(f"  govern:              {g} ({100 * g / total:.0f}%)")
print(f"  session/task:        {s} ({100 * s / total:.0f}%)")
print()
print("Top tools:")
for tool, n in counts.most_common(12):
    print(f"  {n:4d}  {tool}")

if d == 0 and g > 0:
    print()
    print("WARN: govern without any discover — Enriched routing likely skipped.")
    print("      Expect get_context before start_change on refactor/feature work.")
elif counts["get_context"] > 0 and counts["start_change"] > 0:
    # crude ordering proxy: not perfect, but flags obvious inversion
    print()
    print("OK: both get_context and govern recorded — check ordering in transcripts.")

print()
print("By day (discover / govern):")
for day in sorted(by_day):
    dc = sum(by_day[day][t] for t in discover)
    gc = sum(by_day[day][t] for t in govern)
    print(f"  {day}: discover={dc} govern={gc}")
PY
