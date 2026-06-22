# Security

Punchcard is local-first. It must not send telemetry, call an LLM, or execute
arbitrary MCP-provided commands.

Security controls include:

- validation commands are named project allowlist entries and execute as argv,
  never through a shell; arguments and captured output are redacted before
  evidence is returned or persisted;
- associated files must resolve inside the project;
- project runtime and agent configuration paths reject symlinked components
  before reads, writes, or recursive removal;
- document traversal does not follow symlinks;
- denylisted paths, `.env`, credentials, and private-key-like files are
  excluded;
- common credential labels, token formats, credential-bearing URLs, and full
  private-key blocks are redacted before persistence;
- retrieved chunks are labeled `untrusted_content: true`;
- memory events reject updates and deletes at the SQLite layer;
- model artifacts are fetched from a pinned Hugging Face revision and verified
  with SHA-256 before FastEmbed can load them;
- project runtime directories use mode `0700` and sensitive files use mode
  `0600`;
- no telemetry or internal LLM call exists.

`.punchcard/state.db`, vector data, logs, model files, and backups are local
runtime data and are ignored by Git. Exported memory JSONL may contain project
knowledge, is written with mode `0600`, and must be handled as sensitive.

Report vulnerabilities privately to the repository maintainers. Do not include
live credentials or private repository contents in a report.
