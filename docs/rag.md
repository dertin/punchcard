# Documentary retrieval

Configured sources are discovered without following symlinks. Markdown,
MDX, text, reStructuredText, AsciiDoc, JSON, YAML, TOML, protobuf, and PDF are
supported. PDF extraction uses `pdftotext` without a shell.

Indexing performs:

1. path and size policy checks;
2. credential labels, common token formats, credential-bearing URLs, and
   private-key block redaction;
3. source hashing and unchanged-source detection;
4. heading-aware chunks with line citations and bounded overlap;
5. SQLite FTS5 indexing;
6. profile-selected document embeddings in LanceDB.

Changed files replace only their SQLite chunks and LanceDB vector rows. Removed
files delete only their prior chunk/vector IDs. An unchanged sync skips
embedding work; a missing or interrupted vector marker triggers a safe full
rebuild.

Search runs SQLite BM25 and LanceDB cosine retrieval, then fuses chunk IDs with
RRF. Results include source path, heading path, line range, authority, status,
score, excerpt, and `untrusted_content: true`.

Inspect the persisted state without changing it:

```bash
punchcard rag status
```

The status reports the configured profile and revision marker, indexed
document/chunk/vector counts, lexical and vector readiness, missing or stale
sources, vector read errors, and the next synchronization command when needed.

## Embedding profiles

Punchcard always combines semantic retrieval with SQLite BM25. New projects
default to the `code` profile:

| Profile | Model | Use |
| --- | --- | --- |
| `code` | `nomic-ai/CodeRankEmbed` INT8, 768 dimensions | Recommended for code and technical repositories |
| `fast` | `intfloat/multilingual-e5-small` INT8, 384 dimensions | Minimum download, memory, and latency; multilingual documents |

Choose during initialization:

```bash
punchcard init --rag-profile code
punchcard init --rag-profile fast
```

An interactive terminal offers the same choice and defaults to `code`.
Non-interactive initialization also selects `code` deterministically. Existing
configuration is never overwritten by `init`.

Inspect or change the profile later:

```bash
punchcard rag model list
punchcard rag model set fast
punchcard rag sync
```

Changing the profile preserves the SQLite/BM25 index. The next synchronization
rebuilds only the vector index because dimensions and embedding spaces are not
interchangeable.

Chunk sizes, source roots, retrieval `top_k` values, and security deny paths
are configured in [Configuration](configuration.md).

## Model integrity

Punchcard does not execute Hugging Face remote model code. It downloads only a
fixed ONNX model and tokenizer data over HTTPS, verifies every SHA-256, and
loads the artifacts locally with FastEmbed.

The `code` profile uses the MIT-licensed `nomic-ai/CodeRankEmbed` model through
the pinned dynamic INT8 ONNX conversion
`mrsladoje/CodeRankEmbed-onnx-int8` revision
`e74f446dc6e67e29fcee77213472c142f73a6bbb`. Its ONNX SHA-256 is
`4eae31d09b1843103a1ebd5e2b2e24b5a5cad441a33906b35b12b1e2ed91d1db`.

The `fast` profile uses the base `intfloat/multilingual-e5-small` model through
the pinned Hugging Face staff conversion `Xenova/multilingual-e5-small`
revision `761b726dd34fb83930e26aab4e9ac3899aa1fa78`. Its INT8 ONNX SHA-256 is
`4d24e2bc01a447951524466ef533e52944bf48509e6552810bcee1a2711cb02c`.
