# Validation

This runbook describes the complete repository validation flow for Punchcard v1.

The project-level validation policy lives in
[`.punchcard/config.toml`](../.punchcard/config.toml). See
[Configuration](configuration.md) for the full option reference and for which
commands write the file.
The allowlisted validation commands are defined under
`[validation.commands.<name>]`; `[validation].required` names the checks that
must pass before promotion:

- `fmt`
- `check`
- `test`
- `clippy`

For the normal repository-wide validation pass, run:

```bash
./scripts/validate.sh
```

That script executes the allowlisted commands in the same order used by the
project configuration.

It then runs the runtime smoke checks that matter for Punchcard itself:

- `./scripts/agent-assets.sh check`
- `cargo test -p punchcard-mcp stdio_protocol_lists_vertical_slice_tools`
- `punchcard plugin status`
- `punchcard rag sync`
- `punchcard doctor`

For a single change, the CLI also exposes the same checks individually:

```bash
punchcard validate fmt --change-id <change-id>
punchcard validate check --change-id <change-id>
punchcard validate test --change-id <change-id>
punchcard validate clippy --change-id <change-id>
```

The validation suite is intentionally narrow. It verifies formatting, compile
health, tests, and lints. It does not re-run the full documentation retrieval
or release-install workflow on every change.

Supplemental checks that are useful during release review but are not part of
the required validation set include:

```bash
cargo test --manifest-path fixtures/rust-project/Cargo.toml
```

That fixture confirms the repository still handles a separate Rust project
layout correctly.
