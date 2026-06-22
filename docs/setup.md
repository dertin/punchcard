# Setup

Punchcard targets Debian and Ubuntu and was validated on Ubuntu 24.04.4 LTS,
which is the current system used for this repository.

The repository includes a setup script for host dependencies:

```bash
./scripts/setup.sh
```

Use `./scripts/setup.sh --check` when you only want to verify that the host is
already ready.

The setup script covers the system packages required for runtime and RAG PDF
extraction:

- `ca-certificates`
- `curl`
- `git`
- `build-essential`
- `pkg-config`
- `libssl-dev`
- `poppler-utils`

`pdftotext` comes from `poppler-utils` and is required for PDF extraction.

Rust 1.91 or newer is still required separately. If `cargo`, `rustc`, `cargo
fmt`, or `cargo clippy` are missing, install Rust with `rustup` and add the
formatter and clippy components.

For the repository workflow after host setup:

```bash
./scripts/install-local.sh
punchcard init
./scripts/validate.sh
```

See [Configuration](configuration.md) for every `.punchcard/config.toml` option.

`protoc` is not listed here because the workspace uses vendored build-time
support for the Lance crates.
