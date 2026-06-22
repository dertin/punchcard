//! Local documentary retrieval using structural chunks, `LanceDB`, and `SQLite` FTS5.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use arrow_array::{FixedSizeListArray, RecordBatch, StringArray, types::Float32Type};
use arrow_schema::{DataType, Field, Schema};
use chrono::Utc;
use fastembed::{
    InitOptionsUserDefined, OutputKey, Pooling, QuantizationMode, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use futures::TryStreamExt;
use ignore::WalkBuilder;
use lancedb::{
    DistanceType, connect,
    database::CreateTableMode,
    query::{ExecutableQuery, QueryBase, Select},
};
use punchcard_core::{
    DocumentChunk, DocumentStatus, ProjectConfig, ProjectId, RagSearchHit, RagSourceConfig,
    SourceAuthority,
};
use punchcard_security::{
    create_private_dir, ensure_project_path, harden_private_tree, prepare_private_file,
    redact_secret_like_lines, write_private_file,
};
use punchcard_store::{Store, StoreError};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Recommended embedding model for source-code repositories.
pub const CODE_EMBEDDING_MODEL: &str = "nomic-ai/CodeRankEmbed";

/// Low-resource multilingual embedding model.
pub const FAST_EMBEDDING_MODEL: &str = "intfloat/multilingual-e5-small";

/// Default embedding model for newly initialized projects.
pub const DEFAULT_EMBEDDING_MODEL: &str = CODE_EMBEDDING_MODEL;

const VECTOR_TABLE: &str = "chunks";
const MODEL_MARKER: &str = "model-ready";
const CODE_MODEL_REVISION: &str = "e74f446dc6e67e29fcee77213472c142f73a6bbb";
const CODE_MODEL_REPOSITORY: &str = "mrsladoje/CodeRankEmbed-onnx-int8";
const CODE_MODEL_FILES: [(&str, &str); 5] = [
    (
        "model.onnx",
        "4eae31d09b1843103a1ebd5e2b2e24b5a5cad441a33906b35b12b1e2ed91d1db",
    ),
    (
        "tokenizer.json",
        "91f1def9b9391fdabe028cd3f3fcc4efd34e5d1f08c3bf2de513ebb5911a1854",
    ),
    (
        "config.json",
        "d9cac600c987632c33d61a7ed7f7b3ff0a52e7eb41440b661266a9c88f75ebcd",
    ),
    (
        "special_tokens_map.json",
        "5d5b662e421ea9fac075174bb0688ee0d9431699900b90662acd44b2a350503a",
    ),
    (
        "tokenizer_config.json",
        "8de9ab7560d7fb63338c00736faac7bb9d85d49da44c774c607e7766b59614ea",
    ),
];
const FAST_MODEL_REVISION: &str = "761b726dd34fb83930e26aab4e9ac3899aa1fa78";
const FAST_MODEL_REPOSITORY: &str = "Xenova/multilingual-e5-small";
const FAST_MODEL_FILES: [(&str, &str); 5] = [
    (
        "onnx/model_int8.onnx",
        "4d24e2bc01a447951524466ef533e52944bf48509e6552810bcee1a2711cb02c",
    ),
    (
        "tokenizer.json",
        "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
    ),
    (
        "config.json",
        "cb99455288675345e1a4f411438d5d0adbba5fbd3a67ea4fb03c015433b996c1",
    ),
    (
        "special_tokens_map.json",
        "d05497f1da52c5e09554c0cd874037a083e1dc1b9cfd48034d1c717f1afc07a7",
    ),
    (
        "tokenizer_config.json",
        "a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmbeddingBackend {
    UserDefinedE5,
    UserDefinedCodeRank,
}

#[derive(Debug, Clone, Copy)]
struct EmbeddingModelSpec {
    profile: &'static str,
    model_id: &'static str,
    description: &'static str,
    repository: &'static str,
    revision: &'static str,
    files: &'static [(&'static str, &'static str)],
    dimensions: usize,
    max_length: usize,
    query_prefix: &'static str,
    document_prefix: &'static str,
    backend: EmbeddingBackend,
}

const CODE_MODEL: EmbeddingModelSpec = EmbeddingModelSpec {
    profile: "code",
    model_id: CODE_EMBEDDING_MODEL,
    description: "recommended code retrieval: CodeRankEmbed INT8 plus BM25",
    repository: CODE_MODEL_REPOSITORY,
    revision: CODE_MODEL_REVISION,
    files: &CODE_MODEL_FILES,
    dimensions: 768,
    max_length: 8_192,
    query_prefix: "Represent this query for searching relevant code: ",
    document_prefix: "",
    backend: EmbeddingBackend::UserDefinedCodeRank,
};

const FAST_MODEL: EmbeddingModelSpec = EmbeddingModelSpec {
    profile: "fast",
    model_id: FAST_EMBEDDING_MODEL,
    description: "minimum download, memory, and latency with multilingual retrieval",
    repository: FAST_MODEL_REPOSITORY,
    revision: FAST_MODEL_REVISION,
    files: &FAST_MODEL_FILES,
    dimensions: 384,
    max_length: 512,
    query_prefix: "query: ",
    document_prefix: "passage: ",
    backend: EmbeddingBackend::UserDefinedE5,
};

/// User-facing information about one supported RAG embedding profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct EmbeddingProfile {
    /// Stable profile name accepted by the CLI.
    pub profile: &'static str,
    /// Semantic model identifier stored in project configuration.
    pub model: &'static str,
    /// Intended tradeoff.
    pub description: &'static str,
    /// Output vector dimensions.
    pub dimensions: usize,
}

/// Read-only summary of the persisted documentary and vector indexes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RagStatus {
    /// Selected embedding profile, if recognized by this release.
    pub profile: Option<EmbeddingProfile>,
    /// Model identifier stored in project configuration.
    pub configured_model: String,
    /// Revision-sensitive marker required for the configured model.
    pub expected_model_marker: Option<String>,
    /// Marker currently stored beside the vector index.
    pub actual_model_marker: Option<String>,
    /// Number of indexed documentary sources in `SQLite`.
    pub documents: usize,
    /// Number of authoritative chunks in `SQLite`.
    pub chunks: usize,
    /// Number of vectors in `LanceDB`, when readable.
    pub vectors: Option<usize>,
    /// Whether lexical retrieval has indexed chunks.
    pub lexical_ready: bool,
    /// Whether the vector index matches the configured model and chunk count.
    pub vector_ready: bool,
    /// Repository-relative sources whose persisted content has drifted.
    pub stale_sources: Vec<PathBuf>,
    /// Configured source paths that do not currently exist.
    pub missing_sources: Vec<PathBuf>,
    /// Diagnostic produced when the vector index exists but cannot be read.
    pub vector_error: Option<String>,
    /// Recommended command when synchronization is required.
    pub next_action: Option<&'static str>,
}

/// Returns all embedding profiles supported by this release.
#[must_use]
pub fn embedding_profiles() -> [EmbeddingProfile; 2] {
    [profile_info(CODE_MODEL), profile_info(FAST_MODEL)]
}

/// Resolves a profile name to the model identifier persisted in configuration.
#[must_use]
pub fn model_for_profile(profile: &str) -> Option<&'static str> {
    model_spec_by_profile(profile).map(|spec| spec.model_id)
}

/// Resolves a configured model to its profile information.
#[must_use]
pub fn embedding_profile(model_name: &str) -> Option<EmbeddingProfile> {
    model_spec(model_name).map(profile_info)
}

/// Returns the revision-sensitive marker expected by a ready vector index.
///
/// # Errors
///
/// Returns [`RagError::UnsupportedEmbeddingModel`] for an unknown model.
pub fn model_marker(model_name: &str) -> Result<String, RagError> {
    let spec = ensure_supported_model(model_name)?;
    Ok(format!("{}@{}", spec.model_id, spec.revision))
}

/// Inspects RAG state without downloading models or mutating either index.
///
/// # Errors
///
/// Returns [`RagError`] when project paths, source inspection, or the
/// authoritative `SQLite` store cannot be read.
pub async fn status(
    root: &Path,
    project_id: &ProjectId,
    config: &ProjectConfig,
    store: &Store,
) -> Result<RagStatus, RagError> {
    let profile = embedding_profile(&config.rag.embedding_model);
    let expected_model_marker = model_marker(&config.rag.embedding_model).ok();
    let marker_path = model_marker_path(root);
    ensure_project_path(root, &marker_path)?;
    let actual_model_marker = std::fs::read_to_string(&marker_path).ok();
    let (documents, chunks) = store.document_index_counts(project_id)?;
    let stale_sources = source_drift(root, project_id, config, store)?;
    let missing_sources = config
        .rag
        .sources
        .iter()
        .filter_map(|source| {
            let path = if source.path.is_absolute() {
                source.path.clone()
            } else {
                root.join(&source.path)
            };
            (!path.exists()).then_some(source.path.clone())
        })
        .collect::<Vec<_>>();
    let vector_directory = rag_dir(root).join("lancedb");
    ensure_project_path(root, &vector_directory)?;
    let (vectors, vector_error) = vector_row_count(root, &vector_directory).await;
    let vector_ready = if chunks == 0 {
        !vector_directory.exists() && actual_model_marker.is_none()
    } else {
        expected_model_marker == actual_model_marker
            && vectors == Some(chunks)
            && vector_error.is_none()
    };
    let synchronized = stale_sources.is_empty() && missing_sources.is_empty() && vector_ready;

    Ok(RagStatus {
        profile,
        configured_model: config.rag.embedding_model.clone(),
        expected_model_marker,
        actual_model_marker,
        documents,
        chunks,
        vectors,
        lexical_ready: chunks > 0,
        vector_ready,
        stale_sources,
        missing_sources,
        vector_error,
        next_action: (!synchronized).then_some("punchcard rag sync"),
    })
}

async fn vector_row_count(root: &Path, vector_directory: &Path) -> (Option<usize>, Option<String>) {
    if !vector_directory.exists() {
        return (None, None);
    }
    let result = async {
        harden_private_tree(root, vector_directory)?;
        let database = connect(&vector_database_uri(root)).execute().await?;
        let table = database.open_table(VECTOR_TABLE).execute().await?;
        Ok::<usize, RagError>(table.count_rows(None).await?)
    }
    .await;
    match result {
        Ok(count) => (Some(count), None),
        Err(error) => (None, Some(truncate_chars(&error.to_string(), 500))),
    }
}

fn profile_info(spec: EmbeddingModelSpec) -> EmbeddingProfile {
    EmbeddingProfile {
        profile: spec.profile,
        model: spec.model_id,
        description: spec.description,
        dimensions: spec.dimensions,
    }
}

fn model_spec_by_profile(profile: &str) -> Option<EmbeddingModelSpec> {
    match profile {
        "code" => Some(CODE_MODEL),
        "fast" => Some(FAST_MODEL),
        _ => None,
    }
}

fn model_spec(model_name: &str) -> Option<EmbeddingModelSpec> {
    match model_name {
        CODE_EMBEDDING_MODEL => Some(CODE_MODEL),
        FAST_EMBEDDING_MODEL => Some(FAST_MODEL),
        _ => None,
    }
}

/// Summary of one indexing run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexReport {
    /// Documents successfully indexed.
    pub documents_indexed: usize,
    /// Chunks written.
    pub chunks_indexed: usize,
    /// Documents skipped because their indexed state was unchanged.
    pub documents_unchanged: usize,
    /// Removed source records deleted from the index.
    pub documents_deleted: usize,
    /// Files skipped by type, size, or security policy.
    pub documents_skipped: usize,
    /// Human-readable warnings.
    pub warnings: Vec<String>,
}

/// SQLite-backed inputs captured before asynchronous semantic retrieval.
pub struct PreparedSearch {
    lexical_ids: Vec<String>,
    chunks_by_id: HashMap<String, DocumentChunk>,
}

/// Indexes all configured documentary sources.
///
/// # Errors
///
/// Returns [`RagError`] when a configured source cannot be read or persisted.
pub async fn index_project(
    root: &Path,
    project_id: &ProjectId,
    config: &ProjectConfig,
    store: &Store,
) -> Result<IndexReport, RagError> {
    let model = ensure_supported_model(&config.rag.embedding_model)?;
    let mut report = IndexReport::default();
    let mut seen_paths = HashSet::new();
    let mut changed_chunks = Vec::new();
    let mut removed_vector_ids = Vec::new();
    for source in &config.rag.sources {
        for path in discover_source_files(root, source)? {
            match index_file(root, project_id, source, &path, config, store) {
                Ok(IndexFileResult::Changed {
                    chunks,
                    removed_ids,
                    path,
                }) => {
                    seen_paths.insert(path);
                    report.documents_indexed += 1;
                    report.chunks_indexed += chunks.len();
                    changed_chunks.extend(chunks);
                    removed_vector_ids.extend(removed_ids);
                }
                Ok(IndexFileResult::Unchanged { path }) => {
                    seen_paths.insert(path);
                    report.documents_unchanged += 1;
                }
                Err(RagError::Skipped(reason)) => {
                    report.documents_skipped += 1;
                    report
                        .warnings
                        .push(format!("{}: {reason}", path.display()));
                }
                Err(error) => return Err(error),
            }
        }
    }
    for indexed_path in store.document_source_paths(project_id)? {
        if !seen_paths.contains(&indexed_path) {
            removed_vector_ids.extend(store.document_chunk_ids(project_id, &indexed_path)?);
            report.documents_deleted += store.delete_document_source(project_id, &indexed_path)?;
        }
    }
    let vector_ready = vector_index_ready(root, model);
    if !vector_ready {
        rebuild_vector_index(root, project_id, config, store).await?;
    } else if report.documents_indexed > 0 || report.documents_deleted > 0 {
        sync_vector_index(
            root,
            config,
            &changed_chunks,
            &removed_vector_ids,
            store.all_document_chunks(project_id)?.is_empty(),
        )
        .await?;
    }
    Ok(report)
}

enum IndexFileResult {
    Changed {
        chunks: Vec<DocumentChunk>,
        removed_ids: Vec<String>,
        path: PathBuf,
    },
    Unchanged {
        path: PathBuf,
    },
}

/// Reports indexed documentary sources whose current file state has drifted.
///
/// This performs no model initialization and does not mutate either index.
///
/// # Errors
///
/// Returns [`RagError`] when source discovery, extraction, or store access
/// fails.
pub fn source_drift(
    root: &Path,
    project_id: &ProjectId,
    config: &ProjectConfig,
    store: &Store,
) -> Result<Vec<PathBuf>, RagError> {
    let mut seen = HashSet::new();
    let mut drift = Vec::new();
    for source in &config.rag.sources {
        for path in discover_source_files(root, source)? {
            if is_denied(root, &path, config) {
                continue;
            }
            let metadata = std::fs::metadata(&path).map_err(|source| RagError::Read {
                path: path.clone(),
                source,
            })?;
            let citation_path = path
                .strip_prefix(root)
                .map_or_else(|_| path.clone(), Path::to_path_buf);
            seen.insert(citation_path.clone());
            let Some(source_kind) = source_kind(&path) else {
                continue;
            };
            if metadata.len() > config.security.max_document_bytes {
                drift.push(citation_path);
                continue;
            }
            let content = read_document(&path, source_kind)?;
            let content_hash = digest(redact_secrets(&content).as_bytes());
            if !store.document_source_matches(
                project_id,
                &citation_path,
                source_kind,
                source.authority,
                source.status,
                &content_hash,
            )? {
                drift.push(citation_path);
            }
        }
    }
    for indexed_path in store.document_source_paths(project_id)? {
        if !seen.contains(&indexed_path) {
            drift.push(indexed_path);
        }
    }
    drift.sort();
    drift.dedup();
    Ok(drift)
}

/// Captures the lexical ranking and authoritative chunk metadata.
///
/// # Errors
///
/// Returns [`RagError`] when the store query fails.
pub fn prepare_search(
    store: &Store,
    project_id: &ProjectId,
    query: &str,
    lexical_limit: usize,
) -> Result<PreparedSearch, RagError> {
    let lexical = store.search_documents(project_id, query, lexical_limit)?;
    let lexical_ids = lexical.iter().map(|hit| hit.id.clone()).collect::<Vec<_>>();
    let chunks_by_id = store
        .all_document_chunks(project_id)?
        .into_iter()
        .map(|chunk| (chunk.id.clone(), chunk))
        .collect::<HashMap<_, _>>();
    Ok(PreparedSearch {
        lexical_ids,
        chunks_by_id,
    })
}

/// Searches the semantic branch and fuses it with prepared lexical results.
///
/// # Errors
///
/// Returns [`RagError`] when embedding or vector retrieval fails.
pub async fn search(
    root: &Path,
    config: &ProjectConfig,
    query: &str,
    limit: usize,
    prepared: PreparedSearch,
) -> Result<Vec<RagSearchHit>, RagError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let model = ensure_supported_model(&config.rag.embedding_model)?;
    let mut chunks_by_id = prepared.chunks_by_id;
    let lexical_ids = prepared.lexical_ids;
    let semantic_ids = if vector_index_ready(root, model) {
        semantic_search(root, model, query, config.rag.top_k_semantic).await?
    } else {
        Vec::new()
    };
    let rankings = [lexical_ids, semantic_ids];
    let fused = reciprocal_rank_fusion(&rankings, config.rag.rrf_k);
    let reranked = rerank_by_query_terms(fused, &chunks_by_id, query);

    Ok(reranked
        .into_iter()
        .take(limit)
        .filter_map(|(id, score)| {
            chunks_by_id
                .remove(&id)
                .map(|chunk| chunk_to_hit(chunk, score))
        })
        .collect())
}

fn rerank_by_query_terms(
    fused: Vec<(String, f64)>,
    chunks: &HashMap<String, DocumentChunk>,
    query: &str,
) -> Vec<(String, f64)> {
    let query_terms = meaningful_terms(query);
    if query_terms.is_empty() {
        return fused;
    }
    let mut document_frequency = HashMap::<String, usize>::new();
    for chunk in chunks.values() {
        let terms = meaningful_terms(&format!(
            "{} {} {}",
            chunk.source_path.display(),
            chunk.title_path,
            chunk.content
        ));
        for term in query_terms.intersection(&terms) {
            *document_frequency.entry(term.clone()).or_default() += 1;
        }
    }
    let chunk_count = u32::try_from(chunks.len()).unwrap_or(u32::MAX);
    let relevance = fused
        .into_iter()
        .map(|(id, rrf_score)| {
            let terms = chunks.get(&id).map_or_else(HashSet::new, |chunk| {
                meaningful_terms(&format!(
                    "{} {} {}",
                    chunk.source_path.display(),
                    chunk.title_path,
                    chunk.content
                ))
            });
            let matched_terms = query_terms.intersection(&terms).collect::<Vec<_>>();
            let lexical_score = matched_terms
                .iter()
                .map(|term| {
                    let frequency =
                        u32::try_from(document_frequency.get(*term).copied().unwrap_or_default())
                            .unwrap_or(u32::MAX);
                    ((f64::from(chunk_count) + 1.0) / (f64::from(frequency) + 1.0)).ln() + 1.0
                })
                .sum::<f64>();
            (id, rrf_score, matched_terms.len(), lexical_score)
        })
        .collect::<Vec<_>>();
    let maximum_overlap = relevance
        .iter()
        .map(|(_, _, overlap, _)| *overlap)
        .max()
        .unwrap_or_default();
    let minimum_overlap = maximum_overlap;
    let mut reranked = relevance
        .into_iter()
        .filter(|(_, _, overlap, _)| maximum_overlap == 0 || *overlap >= minimum_overlap)
        .collect::<Vec<_>>();
    reranked.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then_with(|| right.3.total_cmp(&left.3))
            .then_with(|| right.1.total_cmp(&left.1))
            .then_with(|| left.0.cmp(&right.0))
    });
    reranked
        .into_iter()
        .map(|(id, rrf_score, overlap, lexical_score)| {
            let overlap = u32::try_from(overlap).unwrap_or(u32::MAX);
            (id, rrf_score + lexical_score + f64::from(overlap) * 10.0)
        })
        .collect()
}

fn meaningful_terms(value: &str) -> HashSet<String> {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter_map(normalize_term)
        .collect()
}

fn normalize_term(value: &str) -> Option<String> {
    let mut term = value.to_ascii_lowercase();
    if term.len() < 3 || is_stopword(&term) {
        return None;
    }
    if term.len() > 5 && (term.ends_with("ied") || term.ends_with("ies")) {
        term.truncate(term.len() - 3);
        term.push('y');
    } else if term.len() > 5 && term.ends_with("ing") {
        term.truncate(term.len() - 3);
    } else if term.len() > 4 && term.ends_with("ed") {
        term.truncate(term.len() - 2);
    } else if term.len() > 4 && term.ends_with('s') {
        term.truncate(term.len() - 1);
    }
    Some(term)
}

fn is_stopword(term: &str) -> bool {
    matches!(
        term,
        "about"
            | "after"
            | "before"
            | "como"
            | "con"
            | "cuando"
            | "debe"
            | "desde"
            | "does"
            | "esta"
            | "este"
            | "from"
            | "have"
            | "local"
            | "para"
            | "project"
            | "punchcard"
            | "repository"
            | "should"
            | "that"
            | "the"
            | "this"
            | "what"
            | "when"
            | "where"
            | "which"
            | "with"
    )
}

async fn rebuild_vector_index(
    root: &Path,
    project_id: &ProjectId,
    config: &ProjectConfig,
    store: &Store,
) -> Result<(), RagError> {
    let chunks = store.all_document_chunks(project_id)?;
    if chunks.is_empty() {
        remove_vector_index(root)?;
        return Ok(());
    }
    let embeddings = embed_chunks(root, config, &chunks).await?;

    let rag_dir = rag_dir(root);
    create_private_dir(root, &rag_dir)?;
    let model = ensure_supported_model(&config.rag.embedding_model)?;
    let batch = vector_batch(&chunks, &embeddings, model.dimensions)?;
    let database_uri = vector_database_uri(root);
    ensure_project_path(root, &rag_dir.join("lancedb"))?;
    let database = connect(&database_uri).execute().await?;
    database
        .create_table(VECTOR_TABLE, batch)
        .mode(CreateTableMode::Overwrite)
        .execute()
        .await?;
    harden_private_tree(root, &rag_dir.join("lancedb"))?;
    write_model_marker(root, model)
}

async fn sync_vector_index(
    root: &Path,
    config: &ProjectConfig,
    changed_chunks: &[DocumentChunk],
    removed_ids: &[String],
    index_empty: bool,
) -> Result<(), RagError> {
    if index_empty {
        remove_vector_index(root)?;
        return Ok(());
    }
    remove_model_marker(root)?;
    let embeddings = if changed_chunks.is_empty() {
        Vec::new()
    } else {
        embed_chunks(root, config, changed_chunks).await?
    };
    let database_uri = vector_database_uri(root);
    ensure_project_path(root, &rag_dir(root).join("lancedb"))?;
    let database = connect(&database_uri).execute().await?;
    let table = database.open_table(VECTOR_TABLE).execute().await?;
    for ids in removed_ids.chunks(500) {
        if ids.is_empty() {
            continue;
        }
        let predicate = ids
            .iter()
            .map(|id| format!("'{}'", id.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ");
        table.delete(&format!("id IN ({predicate})")).await?;
    }
    if !changed_chunks.is_empty() {
        let model = ensure_supported_model(&config.rag.embedding_model)?;
        table
            .add(vector_batch(changed_chunks, &embeddings, model.dimensions)?)
            .execute()
            .await?;
    }
    harden_private_tree(root, &rag_dir(root).join("lancedb"))?;
    write_model_marker(root, ensure_supported_model(&config.rag.embedding_model)?)
}

async fn embed_chunks(
    root: &Path,
    config: &ProjectConfig,
    chunks: &[DocumentChunk],
) -> Result<Vec<Vec<f32>>, RagError> {
    let model = ensure_supported_model(&config.rag.embedding_model)?;
    let texts = chunks
        .iter()
        .map(|chunk| embedding_text(chunk, model))
        .collect::<Vec<_>>();
    let cache_dir = model_cache_dir(root);
    create_private_dir(root, &cache_dir)?;
    let model_name = config.rag.embedding_model.clone();
    let root = root.to_path_buf();
    let embeddings =
        tokio::task::spawn_blocking(move || embed_texts(&model_name, &root, &cache_dir, texts))
            .await
            .map_err(RagError::EmbeddingTask)??;
    if embeddings.len() != chunks.len()
        || embeddings
            .iter()
            .any(|embedding| embedding.len() != model.dimensions)
    {
        return Err(RagError::UnexpectedEmbeddingShape {
            expected_rows: chunks.len(),
            actual_rows: embeddings.len(),
            expected_dimensions: model.dimensions,
        });
    }
    Ok(embeddings)
}

fn vector_batch(
    chunks: &[DocumentChunk],
    embeddings: &[Vec<f32>],
    dimensions: usize,
) -> Result<RecordBatch, RagError> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                i32::try_from(dimensions).unwrap_or(i32::MAX),
            ),
            false,
        ),
    ]));
    let vector_array = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
        embeddings
            .iter()
            .map(|embedding| Some(embedding.iter().copied().map(Some).collect::<Vec<_>>())),
        i32::try_from(dimensions).unwrap_or(i32::MAX),
    );
    Ok(RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(
                chunks.iter().map(|chunk| chunk.id.as_str()),
            )),
            Arc::new(vector_array),
        ],
    )?)
}

fn write_model_marker(root: &Path, model: EmbeddingModelSpec) -> Result<(), RagError> {
    write_private_file(
        root,
        &model_marker_path(root),
        format!("{}@{}", model.model_id, model.revision).as_bytes(),
    )?;
    Ok(())
}

async fn semantic_search(
    root: &Path,
    model: EmbeddingModelSpec,
    query: &str,
    limit: usize,
) -> Result<Vec<String>, RagError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let cache_dir = model_cache_dir(root);
    create_private_dir(root, &cache_dir)?;
    let model_name = model.model_id.to_owned();
    let text = format!("{}{query}", model.query_prefix);
    let owned_root = root.to_path_buf();
    let mut embeddings = tokio::task::spawn_blocking(move || {
        embed_texts(&model_name, &owned_root, &cache_dir, vec![text])
    })
    .await
    .map_err(RagError::EmbeddingTask)??;
    let query_vector = embeddings.pop().ok_or_else(|| {
        RagError::Embedding("embedding model returned no query vector".to_owned())
    })?;

    let database_uri = vector_database_uri(root);
    harden_private_tree(root, &rag_dir(root).join("lancedb"))?;
    let database = connect(&database_uri).execute().await?;
    let table = database.open_table(VECTOR_TABLE).execute().await?;
    let batches = table
        .query()
        .select(Select::columns(&["id"]))
        .limit(limit)
        .nearest_to(query_vector)?
        .distance_type(DistanceType::Cosine)
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let mut ids = Vec::new();
    for batch in batches {
        let id_column = batch
            .column_by_name("id")
            .and_then(|column| column.as_any().downcast_ref::<StringArray>())
            .ok_or(RagError::InvalidVectorResult)?;
        ids.extend(id_column.iter().flatten().map(ToOwned::to_owned));
    }
    Ok(ids)
}

fn embed_texts(
    model_name: &str,
    root: &Path,
    cache_dir: &Path,
    texts: Vec<String>,
) -> Result<Vec<Vec<f32>>, RagError> {
    let spec = ensure_supported_model(model_name)?;
    let snapshot = populate_verified_model_cache(root, cache_dir, spec)?;
    let mut model = match spec.backend {
        EmbeddingBackend::UserDefinedE5 => {
            let tokenizer_files = tokenizer_files(&snapshot)?;
            let user_model = UserDefinedEmbeddingModel::new(
                read_model_file(&snapshot.join("onnx/model_int8.onnx"))?,
                tokenizer_files,
            )
            .with_quantization(QuantizationMode::Dynamic)
            .with_pooling(Pooling::Mean);
            TextEmbedding::try_new_from_user_defined(
                user_model,
                InitOptionsUserDefined::new().with_max_length(spec.max_length),
            )
        }
        EmbeddingBackend::UserDefinedCodeRank => {
            let tokenizer_files = tokenizer_files(&snapshot)?;
            let mut user_model = UserDefinedEmbeddingModel::new(
                read_model_file(&snapshot.join("model.onnx"))?,
                tokenizer_files,
            )
            .with_quantization(QuantizationMode::Dynamic);
            user_model.output_key = Some(OutputKey::ByName("sentence_embedding"));
            TextEmbedding::try_new_from_user_defined(
                user_model,
                InitOptionsUserDefined::new().with_max_length(spec.max_length),
            )
        }
    }
    .map_err(|error| RagError::Embedding(format!("{error:#}")))?;
    model
        .embed(texts, None)
        .map_err(|error| RagError::Embedding(format!("{error:#}")))
}

fn tokenizer_files(snapshot: &Path) -> Result<TokenizerFiles, RagError> {
    Ok(TokenizerFiles {
        tokenizer_file: read_model_file(&snapshot.join("tokenizer.json"))?,
        config_file: read_model_file(&snapshot.join("config.json"))?,
        special_tokens_map_file: read_model_file(&snapshot.join("special_tokens_map.json"))?,
        tokenizer_config_file: read_model_file(&snapshot.join("tokenizer_config.json"))?,
    })
}

fn populate_verified_model_cache(
    root: &Path,
    cache_dir: &Path,
    model: EmbeddingModelSpec,
) -> Result<PathBuf, RagError> {
    create_private_dir(root, cache_dir)?;
    let repository = cache_dir.join(format!("models--{}", model.repository.replace('/', "--")));
    let snapshot = repository.join("snapshots").join(model.revision);
    create_private_dir(root, &snapshot)?;
    for &(relative, expected_hash) in model.files {
        let destination = snapshot.join(relative);
        ensure_project_path(root, &destination)?;
        if destination.is_file() && digest_file(&destination)? == expected_hash {
            prepare_private_file(root, &destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            create_private_dir(root, parent)?;
        }
        let temporary = destination.with_extension("download");
        prepare_private_file(root, &temporary)?;
        let url = format!(
            "https://huggingface.co/{}/resolve/{}/{relative}",
            model.repository, model.revision
        );
        let output = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--proto",
                "=https",
                "--proto-redir",
                "=https",
                "--retry",
                "3",
                "--silent",
                "--show-error",
                "--output",
            ])
            .arg(&temporary)
            .arg(&url)
            .output()
            .map_err(|source| RagError::ModelDownload {
                url: url.clone(),
                source,
            })?;
        if !output.status.success() {
            let stderr = redact_secret_like_lines(&String::from_utf8_lossy(&output.stderr));
            return Err(RagError::ModelDownloadFailed {
                url,
                stderr: truncate_chars(&stderr, 2_000),
            });
        }
        let actual_hash = digest_file(&temporary)?;
        if actual_hash != expected_hash {
            let _ = std::fs::remove_file(&temporary);
            return Err(RagError::ModelHashMismatch {
                path: destination,
                expected: expected_hash.to_owned(),
                actual: actual_hash,
            });
        }
        std::fs::rename(&temporary, &destination).map_err(|source| RagError::Write {
            path: destination.clone(),
            source,
        })?;
        prepare_private_file(root, &destination)?;
    }
    let reference = repository.join("refs/main");
    write_private_file(root, &reference, model.revision.as_bytes())?;
    harden_private_tree(root, cache_dir)?;
    Ok(snapshot)
}

fn read_model_file(path: &Path) -> Result<Vec<u8>, RagError> {
    std::fs::read(path).map_err(|source| RagError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn embedding_text(chunk: &DocumentChunk, model: EmbeddingModelSpec) -> String {
    format!(
        "{}{}\n{}\n{}",
        model.document_prefix,
        chunk.source_path.display(),
        chunk.title_path,
        chunk.content
    )
}

fn chunk_to_hit(chunk: DocumentChunk, score: f64) -> RagSearchHit {
    RagSearchHit {
        id: chunk.id,
        source_path: chunk.source_path,
        title_path: chunk.title_path,
        line_start: chunk.line_start,
        line_end: chunk.line_end,
        excerpt: truncate_chars(&chunk.content, 1_200),
        score,
        authority: chunk.authority,
        status: chunk.status,
        untrusted_content: true,
    }
}

fn ensure_supported_model(model_name: &str) -> Result<EmbeddingModelSpec, RagError> {
    model_spec(model_name).ok_or_else(|| RagError::UnsupportedEmbeddingModel(model_name.to_owned()))
}

fn rag_dir(root: &Path) -> PathBuf {
    root.join(".punchcard/rag")
}

fn model_cache_dir(root: &Path) -> PathBuf {
    rag_dir(root).join("models")
}

fn model_marker_path(root: &Path) -> PathBuf {
    rag_dir(root).join(MODEL_MARKER)
}

fn vector_database_uri(root: &Path) -> String {
    rag_dir(root).join("lancedb").to_string_lossy().into_owned()
}

fn vector_index_ready(root: &Path, model: EmbeddingModelSpec) -> bool {
    let marker = model_marker_path(root);
    let vectors = rag_dir(root).join("lancedb");
    ensure_project_path(root, &marker).is_ok()
        && ensure_project_path(root, &vectors).is_ok()
        && std::fs::read_to_string(marker)
            .is_ok_and(|contents| contents == format!("{}@{}", model.model_id, model.revision))
        && vectors.exists()
}

fn remove_model_marker(root: &Path) -> Result<(), RagError> {
    let marker = model_marker_path(root);
    ensure_project_path(root, &marker)?;
    if marker.exists() {
        std::fs::remove_file(&marker).map_err(|source| RagError::RemoveFile {
            path: marker,
            source,
        })?;
    }
    Ok(())
}

fn remove_vector_index(root: &Path) -> Result<(), RagError> {
    let directory = rag_dir(root).join("lancedb");
    ensure_project_path(root, &directory)?;
    if directory.exists() {
        harden_private_tree(root, &directory)?;
        std::fs::remove_dir_all(&directory).map_err(|source| RagError::RemoveDirectory {
            path: directory,
            source,
        })?;
    }
    remove_model_marker(root)
}

/// Fuses ranked result IDs with Reciprocal Rank Fusion.
#[must_use]
pub fn reciprocal_rank_fusion(rankings: &[Vec<String>], rrf_k: usize) -> Vec<(String, f64)> {
    let mut scores: HashMap<String, f64> = HashMap::new();
    for ranking in rankings {
        for (position, id) in ranking.iter().enumerate() {
            let denominator = rrf_k.saturating_add(position).saturating_add(1);
            let denominator = u32::try_from(denominator).unwrap_or(u32::MAX);
            *scores.entry(id.clone()).or_default() += 1.0 / f64::from(denominator);
        }
    }
    let mut scores: Vec<_> = scores.into_iter().collect();
    scores.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    scores
}

fn discover_source_files(root: &Path, source: &RagSourceConfig) -> Result<Vec<PathBuf>, RagError> {
    let configured = if source.path.is_absolute() {
        source.path.clone()
    } else {
        root.join(&source.path)
    };
    if !configured.exists() {
        return Ok(Vec::new());
    }
    if configured.is_file() {
        return Ok(vec![configured]);
    }

    let mut files = Vec::new();
    for entry in WalkBuilder::new(&configured)
        .standard_filters(true)
        .follow_links(false)
        .build()
    {
        let entry = entry.map_err(RagError::Walk)?;
        if entry.file_type().is_some_and(|kind| kind.is_file()) {
            files.push(entry.into_path());
        }
    }
    files.sort();
    Ok(files)
}

fn index_file(
    root: &Path,
    project_id: &ProjectId,
    source: &RagSourceConfig,
    path: &Path,
    config: &ProjectConfig,
    store: &Store,
) -> Result<IndexFileResult, RagError> {
    if is_denied(root, path, config) {
        return Err(RagError::Skipped(
            "path is denied by security policy".to_owned(),
        ));
    }
    let metadata = std::fs::metadata(path).map_err(|source| RagError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.len() > config.security.max_document_bytes {
        return Err(RagError::Skipped(format!(
            "document exceeds {} bytes",
            config.security.max_document_bytes
        )));
    }
    let source_kind = source_kind(path)
        .ok_or_else(|| RagError::Skipped("unsupported document type".to_owned()))?;
    let raw = read_document(path, source_kind)?;
    if raw.len() > usize::try_from(config.security.max_document_bytes).unwrap_or(usize::MAX) {
        return Err(RagError::Skipped(format!(
            "extracted document exceeds {} bytes",
            config.security.max_document_bytes
        )));
    }
    let normalized = redact_secrets(&raw);
    let source_hash = digest(normalized.as_bytes());
    let citation_path = path
        .strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), Path::to_path_buf);
    let source_id = digest(citation_path.to_string_lossy().as_bytes());
    if store.document_source_matches(
        project_id,
        &citation_path,
        source_kind,
        source.authority,
        source.status,
        &source_hash,
    )? {
        return Ok(IndexFileResult::Unchanged {
            path: citation_path,
        });
    }
    let removed_ids = store.document_chunk_ids(project_id, &citation_path)?;
    let chunks = chunk_document(
        &source_id,
        &citation_path,
        source_kind,
        source.authority,
        source.status,
        &normalized,
        config.rag.chunk_target_tokens,
        config.rag.chunk_overlap_tokens,
        &source_hash,
    );
    store.replace_document(
        project_id,
        &source_id,
        &citation_path,
        source_kind,
        source.authority,
        source.status,
        &source_hash,
        &chunks,
    )?;
    Ok(IndexFileResult::Changed {
        chunks,
        removed_ids,
        path: citation_path,
    })
}

fn read_document(path: &Path, source_kind: &str) -> Result<String, RagError> {
    if source_kind == "pdf" {
        let output = Command::new("pdftotext")
            .arg("-layout")
            .arg(path)
            .arg("-")
            .output()
            .map_err(|source| RagError::ExecutePdf {
                path: path.to_path_buf(),
                source,
            })?;
        if !output.status.success() {
            return Err(RagError::PdfExtraction {
                path: path.to_path_buf(),
                stderr: String::from_utf8_lossy(&output.stderr)
                    .chars()
                    .take(2_000)
                    .collect(),
            });
        }
        String::from_utf8(output.stdout).map_err(|source| RagError::PdfUtf8 {
            path: path.to_path_buf(),
            source,
        })
    } else {
        std::fs::read_to_string(path).map_err(|source| RagError::Read {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "chunk metadata is explicit to prevent citation/source mixing"
)]
fn chunk_document(
    source_id: &str,
    path: &Path,
    source_kind: &str,
    authority: SourceAuthority,
    status: DocumentStatus,
    content: &str,
    target_tokens: usize,
    overlap_tokens: usize,
    source_revision: &str,
) -> Vec<DocumentChunk> {
    let target_words = target_tokens.max(50);
    let overlap_lines = (overlap_tokens / 12).max(1);
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let mut headings = BTreeMap::<usize, String>::new();
    let mut heading_stack: Vec<String> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some((level, title)) = markdown_heading(line) {
            heading_stack.truncate(level.saturating_sub(1));
            heading_stack.push(title.to_owned());
        }
        headings.insert(index, heading_stack.join(" > "));
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start;
        let mut words: usize = 0;
        while end < lines.len() {
            let line_words = lines[end].split_whitespace().count();
            if end > start && words.saturating_add(line_words) > target_words {
                break;
            }
            words = words.saturating_add(line_words);
            end += 1;
        }
        if end == start {
            end += 1;
        }
        let chunk_content = lines[start..end].join("\n").trim().to_owned();
        if !chunk_content.is_empty() {
            let content_hash = digest(chunk_content.as_bytes());
            let id = digest(format!("{source_id}:{start}:{end}:{content_hash}").as_bytes());
            chunks.push(DocumentChunk {
                id,
                source_id: source_id.to_owned(),
                source_path: path.to_path_buf(),
                source_kind: source_kind.to_owned(),
                authority,
                status,
                title_path: headings.get(&start).cloned().unwrap_or_default(),
                line_start: start + 1,
                line_end: end,
                content: chunk_content,
                content_hash,
                source_revision: source_revision.to_owned(),
                indexed_at: Utc::now(),
            });
        }
        if end >= lines.len() {
            break;
        }
        start = end.saturating_sub(overlap_lines).max(start + 1);
    }
    chunks
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let title = line.get(hashes..)?.trim();
    (!title.is_empty()).then_some((hashes, title))
}

fn source_kind(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "md" => Some("markdown"),
        "mdx" => Some("mdx"),
        "txt" => Some("text"),
        "rst" => Some("restructured_text"),
        "adoc" | "asciidoc" => Some("asciidoc"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "proto" => Some("protobuf"),
        "pdf" => Some("pdf"),
        _ => None,
    }
}

fn is_denied(root: &Path, path: &Path, config: &ProjectConfig) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    config
        .security
        .deny_paths
        .iter()
        .any(|denied| relative == denied || relative.starts_with(denied))
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                let lower = name.to_ascii_lowercase();
                lower == ".env"
                    || lower.starts_with(".env.")
                    || lower.contains("credentials")
                    || lower.contains("private_key")
                    || lower.contains("private-key")
                    || matches!(
                        lower.as_str(),
                        ".netrc"
                            | ".npmrc"
                            | ".pypirc"
                            | "id_dsa"
                            | "id_ecdsa"
                            | "id_ed25519"
                            | "id_rsa"
                    )
            })
}

fn redact_secrets(content: &str) -> String {
    redact_secret_like_lines(content)
}

fn digest(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

fn digest_file(path: &Path) -> Result<String, RagError> {
    let mut file = std::fs::File::open(path).map_err(|source| RagError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| RagError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    let mut chars = value.chars();
    let excerpt: String = chars.by_ref().take(maximum).collect();
    if chars.next().is_some() {
        format!("{excerpt}…")
    } else {
        excerpt
    }
}

/// Documentary indexing and retrieval failures.
#[derive(Debug, Error)]
pub enum RagError {
    /// A protected runtime path or sensitive artifact was unsafe.
    #[error(transparent)]
    Security(#[from] punchcard_security::SecurityError),
    /// A derived index or model directory could not be created.
    #[error("failed to create directory {path}: {source}")]
    CreateDirectory {
        /// Directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// Filesystem traversal failed.
    #[error("failed to walk documentary source: {0}")]
    Walk(#[source] ignore::Error),
    /// A document could not be read.
    #[error("failed to read document {path}: {source}")]
    Read {
        /// Document path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The external PDF extractor could not be launched.
    #[error("failed to run `pdftotext` for {path}: {source}")]
    ExecutePdf {
        /// PDF path.
        path: PathBuf,
        /// Underlying process error.
        source: std::io::Error,
    },
    /// The external PDF extractor rejected a document.
    #[error("failed to extract PDF {path}: {stderr}")]
    PdfExtraction {
        /// PDF path.
        path: PathBuf,
        /// Bounded extractor stderr.
        stderr: String,
    },
    /// Extracted PDF text was not UTF-8.
    #[error("PDF extraction for {path} returned invalid UTF-8: {source}")]
    PdfUtf8 {
        /// PDF path.
        path: PathBuf,
        /// UTF-8 decoding error.
        source: std::string::FromUtf8Error,
    },
    /// The verified model downloader could not launch `curl`.
    #[error("failed to download model file from {url}: {source}")]
    ModelDownload {
        /// Pinned model URL.
        url: String,
        /// Process launch error.
        source: std::io::Error,
    },
    /// The verified model fallback received an unsuccessful response.
    #[error("model download failed for {url}: {stderr}")]
    ModelDownloadFailed {
        /// Pinned model URL.
        url: String,
        /// Bounded downloader stderr.
        stderr: String,
    },
    /// A downloaded model artifact did not match its pinned checksum.
    #[error("model artifact hash mismatch for {path}: expected {expected}, received {actual}")]
    ModelHashMismatch {
        /// Downloaded artifact path.
        path: PathBuf,
        /// Pinned SHA-256.
        expected: String,
        /// Actual SHA-256.
        actual: String,
    },
    /// A file was intentionally skipped.
    #[error("{0}")]
    Skipped(String),
    /// A derived index marker could not be written.
    #[error("failed to write {path}: {source}")]
    Write {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A derived vector directory could not be removed.
    #[error("failed to remove directory {path}: {source}")]
    RemoveDirectory {
        /// Directory path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A derived index marker could not be removed.
    #[error("failed to remove file {path}: {source}")]
    RemoveFile {
        /// File path.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// The configured embedding model is not supported by this release.
    #[error(
        "unsupported embedding model `{0}`; use `{CODE_EMBEDDING_MODEL}` or `{FAST_EMBEDDING_MODEL}`"
    )]
    UnsupportedEmbeddingModel(String),
    /// `FastEmbed` initialization or inference failed.
    #[error("embedding failed: {0}")]
    Embedding(String),
    /// The blocking embedding worker failed.
    #[error("embedding worker failed: {0}")]
    EmbeddingTask(#[source] tokio::task::JoinError),
    /// `FastEmbed` returned an invalid row count or vector dimension.
    #[error(
        "embedding output shape mismatch: expected {expected_rows} rows of \
         {expected_dimensions} dimensions, received {actual_rows} rows"
    )]
    UnexpectedEmbeddingShape {
        /// Expected number of vectors.
        expected_rows: usize,
        /// Actual number of vectors.
        actual_rows: usize,
        /// Required vector dimension.
        expected_dimensions: usize,
    },
    /// A vector query returned an unexpected Arrow schema.
    #[error("vector search returned no UTF-8 `id` column")]
    InvalidVectorResult,
    /// Arrow record construction failed.
    #[error(transparent)]
    Arrow(#[from] arrow_schema::ArrowError),
    /// `LanceDB` indexing or retrieval failed.
    #[error(transparent)]
    LanceDb(#[from] lancedb::Error),
    /// Store operation failed.
    #[error(transparent)]
    Store(#[from] StoreError),
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::sync::Arc;

    use arrow_array::{FixedSizeListArray, RecordBatch, StringArray, types::Float32Type};
    use arrow_schema::{DataType, Field, Schema};
    use futures::TryStreamExt;
    use lancedb::query::{ExecutableQuery, QueryBase};
    use punchcard_core::{
        DocumentStatus, ProjectConfig, ProjectId, RagSourceConfig, SourceAuthority,
    };
    use punchcard_store::Store;
    use tempfile::tempdir;

    use super::{
        CODE_EMBEDDING_MODEL, FAST_EMBEDDING_MODEL, IndexFileResult, chunk_document, embed_texts,
        embedding_profile, embedding_profiles, index_file, model_for_profile, model_marker,
        reciprocal_rank_fusion, redact_secrets, remove_vector_index, rerank_by_query_terms,
        source_drift, status,
    };

    #[test]
    fn chunking_preserves_heading_and_line_citations() {
        let chunks = chunk_document(
            "source",
            Path::new("docs/test.md"),
            "markdown",
            SourceAuthority::ProjectDocs,
            DocumentStatus::Current,
            "# Contract\nThe service must remain local.\n\n## Validation\nRun tests.",
            50,
            10,
            "revision",
        );

        assert_eq!(chunks[0].title_path, "Contract");
    }

    #[test]
    fn rrf_rewards_results_present_in_both_rankings() {
        let rankings = vec![
            vec!["a".to_owned(), "b".to_owned()],
            vec!["b".to_owned(), "c".to_owned()],
        ];

        let fused = reciprocal_rank_fusion(&rankings, 60);

        assert_eq!(fused[0].0, "b");
    }

    #[test]
    fn supported_profiles_are_stable_and_revision_sensitive() {
        let profiles = embedding_profiles();

        assert_eq!(profiles[0].profile, "code");
        assert_eq!(profiles[0].model, CODE_EMBEDDING_MODEL);
        assert_eq!(profiles[0].dimensions, 768);
        assert_eq!(profiles[1].profile, "fast");
        assert_eq!(profiles[1].model, FAST_EMBEDDING_MODEL);
        assert_eq!(profiles[1].dimensions, 384);
        assert_eq!(model_for_profile("code"), Some(CODE_EMBEDDING_MODEL));
        assert_eq!(
            embedding_profile(FAST_EMBEDDING_MODEL).map(|profile| profile.profile),
            Some("fast")
        );
        assert!(
            model_marker(CODE_EMBEDDING_MODEL)
                .expect("code model should be supported")
                .starts_with("nomic-ai/CodeRankEmbed@")
        );
    }

    #[tokio::test]
    async fn status_reports_clean_empty_index_without_creating_vectors() {
        let temporary = tempdir().expect("temporary directory should exist");
        fs::create_dir_all(temporary.path().join(".punchcard"))
            .expect("Punchcard directory should exist");
        let project_id = ProjectId::from_persisted("p".to_owned());
        let store = Store::in_memory().expect("store should initialize");
        store
            .register_project(&project_id, temporary.path(), "fixture")
            .expect("project should register");
        let mut config = ProjectConfig::for_project("fixture", false);
        config.rag.sources.clear();

        let result = status(temporary.path(), &project_id, &config, &store)
            .await
            .expect("status should inspect an empty index");

        assert!(result.vector_ready);
        assert_eq!(result.next_action, None);
    }

    #[test]
    fn redaction_removes_secret_like_lines() {
        let redacted = redact_secrets("name = test\nghp_FAKEPUNCHCARDTOKEN12345678901234567890");

        assert!(!redacted.contains("FAKEPUNCHCARDTOKEN"));
    }

    #[test]
    fn vector_cleanup_rejects_symlinked_index_directory() {
        let temporary = tempdir().expect("temporary directory should exist");
        let rag = temporary.path().join(".punchcard/rag");
        let outside = tempdir().expect("outside directory should exist");
        fs::create_dir_all(&rag).expect("RAG directory should exist");
        symlink(outside.path(), rag.join("lancedb")).expect("fixture symlink should be created");

        let error =
            remove_vector_index(temporary.path()).expect_err("symlinked vector index should fail");

        assert!(error.to_string().contains("symlink"));
    }

    #[test]
    fn term_reranker_filters_semantically_adjacent_but_unrelated_chunks() {
        let relevant = chunk_document(
            "relevant",
            Path::new("docs/memory.md"),
            "markdown",
            SourceAuthority::ProjectDocs,
            DocumentStatus::Current,
            "# Promotion\nValidation evidence is required before promotion.",
            50,
            10,
            "revision",
        )
        .remove(0);
        let unrelated = chunk_document(
            "unrelated",
            Path::new("docs/environment.md"),
            "markdown",
            SourceAuthority::ProjectDocs,
            DocumentStatus::Current,
            "# Environment\nUbuntu and Cargo versions are recorded.",
            50,
            10,
            "revision",
        )
        .remove(0);
        let relevant_id = relevant.id.clone();
        let unrelated_id = unrelated.id.clone();
        let chunks = HashMap::from([
            (relevant_id.clone(), relevant),
            (unrelated_id.clone(), unrelated),
        ]);

        let reranked = rerank_by_query_terms(
            vec![(unrelated_id, 0.04), (relevant_id.clone(), 0.03)],
            &chunks,
            "What validation evidence is required before promotion?",
        );

        assert_eq!(reranked.len(), 1);
        assert_eq!(reranked[0].0, relevant_id);
    }

    #[test]
    fn incremental_index_skips_unchanged_document() {
        let temporary = tempdir().expect("temporary directory should exist");
        let document = temporary.path().join("guide.md");
        std::fs::write(&document, "# Guide\nUse governed memory.")
            .expect("fixture should be written");
        let project_id = ProjectId::from_persisted("p".to_owned());
        let store = Store::in_memory().expect("store should initialize");
        store
            .register_project(&project_id, temporary.path(), "fixture")
            .expect("project should register");
        let source = RagSourceConfig {
            path: document.clone(),
            authority: SourceAuthority::ProjectDocs,
            status: DocumentStatus::Current,
        };
        let mut config = ProjectConfig::for_project("fixture", false);
        config.rag.sources = vec![source.clone()];

        let first = index_file(
            temporary.path(),
            &project_id,
            &source,
            &document,
            &config,
            &store,
        )
        .expect("first index should succeed");
        let second = index_file(
            temporary.path(),
            &project_id,
            &source,
            &document,
            &config,
            &store,
        )
        .expect("second index should succeed");

        assert!(matches!(first, IndexFileResult::Changed { .. }));
        assert!(matches!(second, IndexFileResult::Unchanged { .. }));
        std::fs::write(&document, "# Guide\nThe governed contract changed.")
            .expect("fixture should change");
        let drift = source_drift(temporary.path(), &project_id, &config, &store)
            .expect("source drift should be computed");
        assert_eq!(drift, vec![Path::new("guide.md").to_path_buf()]);
    }

    #[test]
    fn indexing_does_not_persist_common_token_formats() {
        let temporary = tempdir().expect("temporary directory should exist");
        let document = temporary.path().join("guide.md");
        std::fs::write(
            &document,
            "# Guide\nghp_FAKEPUNCHCARDTOKEN12345678901234567890",
        )
        .expect("fixture should be written");
        let project_id = ProjectId::from_persisted("p".to_owned());
        let store = Store::in_memory().expect("store should initialize");
        store
            .register_project(&project_id, temporary.path(), "fixture")
            .expect("project should register");
        let source = RagSourceConfig {
            path: document.clone(),
            authority: SourceAuthority::ProjectDocs,
            status: DocumentStatus::Current,
        };
        let mut config = ProjectConfig::for_project("fixture", false);
        config.rag.sources = vec![source.clone()];

        index_file(
            temporary.path(),
            &project_id,
            &source,
            &document,
            &config,
            &store,
        )
        .expect("document should index");

        let chunks = store
            .all_document_chunks(&project_id)
            .expect("chunks should load");
        assert!(
            chunks
                .iter()
                .all(|chunk| !chunk.content.contains("FAKEPUNCHCARDTOKEN"))
        );
    }

    #[tokio::test]
    async fn lancedb_round_trip_returns_nearest_id() {
        let temporary = tempdir().expect("temporary directory should exist");
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), 2),
                false,
            ),
        ]));
        let vectors = FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
            [
                Some(vec![Some(1.0), Some(0.0)]),
                Some(vec![Some(0.0), Some(1.0)]),
            ],
            2,
        );
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["a", "b"])),
                Arc::new(vectors),
            ],
        )
        .expect("record batch should be valid");
        let uri = temporary.path().to_string_lossy();
        let database = lancedb::connect(&uri)
            .execute()
            .await
            .expect("database should open");
        let table = database
            .create_table("chunks", batch)
            .execute()
            .await
            .expect("table should be created");
        let batches = table
            .query()
            .limit(1)
            .nearest_to(&[1.0_f32, 0.0])
            .expect("query vector should be valid")
            .execute()
            .await
            .expect("query should execute")
            .try_collect::<Vec<_>>()
            .await
            .expect("results should collect");
        let ids = batches[0]
            .column_by_name("id")
            .expect("id column should exist")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("id column should be UTF-8");
        assert_eq!(ids.value(0), "a");
    }

    #[test]
    #[ignore = "downloads and executes both pinned embedding models"]
    fn verified_embedding_models_smoke_test() {
        let temporary = tempdir().expect("temporary directory should exist");
        let cache = temporary.path().join(".punchcard/rag/models");
        let code_embeddings = embed_texts(
            CODE_EMBEDDING_MODEL,
            temporary.path(),
            &cache,
            vec![
                "Represent this query for searching relevant code: parse JSON in Rust".to_owned(),
                "use serde_json::from_str to deserialize JSON into a Rust struct".to_owned(),
                "CREATE INDEX users_email_idx ON users(email)".to_owned(),
            ],
        )
        .expect("code model should embed text");
        assert_eq!(code_embeddings.len(), 3);
        assert!(code_embeddings.iter().all(|embedding| {
            embedding.len() == 768 && embedding.iter().all(|value| value.is_finite())
        }));
        assert!(
            cosine_similarity(&code_embeddings[0], &code_embeddings[1])
                > cosine_similarity(&code_embeddings[0], &code_embeddings[2])
        );

        let fast_embeddings = embed_texts(
            FAST_EMBEDDING_MODEL,
            temporary.path(),
            &cache,
            vec!["query: governed memory".to_owned()],
        )
        .expect("fast model should embed text");
        assert_eq!(fast_embeddings[0].len(), 384);
    }

    fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
        let dot = left
            .iter()
            .zip(right)
            .map(|(left, right)| left * right)
            .sum::<f32>();
        let left_norm = left.iter().map(|value| value * value).sum::<f32>().sqrt();
        let right_norm = right.iter().map(|value| value * value).sum::<f32>().sqrt();
        dot / (left_norm * right_norm)
    }
}
