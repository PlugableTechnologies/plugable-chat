//! The CPU search model's files: where they live, how they are verified, and where they come from.
//!
//! fastembed's own loader fetches BGE-Base-EN-v1.5 from huggingface.co `main` with no checksum, and
//! a network that resets TLS to Hugging Face (rc16, Bob) left search off for ever. So the app pins
//! one upstream revision in `embedding_model_manifest.json` (commit, size and SHA-256 of every file)
//! and loads those files itself through fastembed's "user defined model" API.
//!
//! Each file is taken from the first place that can supply bytes with the pinned hash:
//!   1. the model folder already on disk (re-verified on every start),
//!   2. a folder the user placed (Settings > Network) and the old fastembed/hf-hub cache,
//!   3. the project's own GitHub release (`embedding-models-<name>-r<N>`, published by
//!      `scripts/ci/publish-embedding-model.sh`),
//!   4. Hugging Face at the pinned revision (honours `HF_ENDPOINT`, so a mirror works).
//! Downloads resume with HTTP Range and are SHA-256 verified; a file whose hash is wrong is deleted
//! and the next source is tried. Bytes that fail verification are never loaded.

use crate::toolbox_install::{download_verified_with_policy, sha256_hex_of_file, DownloadPolicy};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Where the project's release assets live. The tag comes from the manifest.
pub const EMBEDDING_MODEL_RELEASE_BASE_URL: &str = "https://github.com/PlugableTechnologies/plugable-chat/releases/download";
pub const DEFAULT_HF_ENDPOINT: &str = "https://huggingface.co";
/// Test seam (honoured only when `PLUGABLE_CHAT_TEST_STATE` is set): a comma list of the sources the
/// clean-host scenarios allow, from `local`, `release`, `huggingface`.
pub const TEST_SOURCES_ENV: &str = "PLUGABLE_CHAT_TEST_EMBEDDING_SOURCES";
/// Test seam (same condition): replaces `EMBEDDING_MODEL_RELEASE_BASE_URL`.
pub const TEST_RELEASE_BASE_ENV: &str = "PLUGABLE_CHAT_TEST_EMBEDDING_RELEASE_BASE_URL";

const MANIFEST_JSON: &str = include_str!("embedding_model_manifest.json");

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ManifestFile {
    /// Path inside the upstream repository (and inside the local model folder).
    pub path: String,
    /// File name of the release asset.
    pub asset: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ModelManifest {
    pub name: String,
    pub upstream_repo: String,
    pub revision: String,
    pub license: String,
    pub release_tag: String,
    pub files: Vec<ManifestFile>,
}

impl ModelManifest {
    pub fn parse(json: &str) -> Result<ModelManifest, String> {
        let m: ModelManifest = serde_json::from_str(json).map_err(|e| format!("embedding model manifest: {e}"))?;
        m.check()?;
        Ok(m)
    }

    /// The manifest compiled into the app.
    pub fn pinned() -> &'static ModelManifest {
        static M: OnceLock<ModelManifest> = OnceLock::new();
        M.get_or_init(|| ModelManifest::parse(MANIFEST_JSON).expect("embedded embedding_model_manifest.json is valid"))
    }

    fn check(&self) -> Result<(), String> {
        let hex = |s: &str, n: usize| s.len() == n && s.chars().all(|c| c.is_ascii_hexdigit());
        if !hex(&self.revision, 40) {
            return Err(format!("revision '{}' is not a full commit hash", self.revision));
        }
        for f in &self.files {
            if !hex(&f.sha256, 64) || f.size == 0 {
                return Err(format!("{}: needs a 64-character sha256 and a size", f.path));
            }
        }
        if self.files.is_empty() {
            return Err("no files".to_string());
        }
        Ok(())
    }

    pub fn file(&self, path: &str) -> Option<&ManifestFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Folder name under the embedding cache: pinned per revision, so a new revision never mixes with an old one.
    pub fn dir_name(&self) -> String {
        format!("{}-{}", self.name, &self.revision[..12])
    }

    pub fn release_url(&self, base: &str, file: &ManifestFile) -> String {
        format!("{}/{}/{}", base.trim_end_matches('/'), self.release_tag, file.asset)
    }

    pub fn huggingface_url(&self, endpoint: &str, file: &ManifestFile) -> String {
        format!("{}/{}/resolve/{}/{}", endpoint.trim_end_matches('/'), self.upstream_repo, self.revision, file.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    /// A folder that may already hold the files.
    Folder(PathBuf),
    Release,
    HuggingFace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSource {
    pub label: String,
    pub kind: SourceKind,
}

/// Parse the test seam: `Some(["release"])`. Unknown names are ignored.
fn parse_restriction(value: Option<&str>) -> Option<Vec<String>> {
    let v = value?.trim();
    if v.is_empty() {
        return None;
    }
    Some(v.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect())
}

/// Sources in the order they are tried. `restriction` is the test seam (see `TEST_SOURCES_ENV`).
pub fn plan_sources(offline_folder: Option<&Path>, legacy_cache_dirs: &[PathBuf], restriction: Option<&str>) -> Vec<ModelSource> {
    let mut out = Vec::new();
    if let Some(dir) = offline_folder {
        out.push(ModelSource { label: format!("the folder you chose ({})", dir.display()), kind: SourceKind::Folder(dir.to_path_buf()) });
    }
    for dir in legacy_cache_dirs {
        out.push(ModelSource { label: format!("the earlier download ({})", dir.display()), kind: SourceKind::Folder(dir.clone()) });
    }
    out.push(ModelSource { label: "the Plugable Chat release on GitHub".to_string(), kind: SourceKind::Release });
    out.push(ModelSource { label: "Hugging Face".to_string(), kind: SourceKind::HuggingFace });
    if let Some(allowed) = parse_restriction(restriction) {
        out.retain(|s| {
            let name = match s.kind {
                SourceKind::Folder(_) => "local",
                SourceKind::Release => "release",
                SourceKind::HuggingFace => "huggingface",
            };
            allowed.iter().any(|a| a == name)
        });
    }
    out
}

/// Where fastembed/hf-hub of earlier versions left the model: `<cache>/models--<org>--<repo>/snapshots/<commit>/`
/// (also under `<cache>/hub/`, where `HF_HOME` points). Verified by hash like any other source.
pub fn legacy_snapshot_dirs(cache_root: &Path, manifest: &ModelManifest) -> Vec<PathBuf> {
    let repo_dir = format!("models--{}", manifest.upstream_repo.replace('/', "--"));
    let mut out = Vec::new();
    for base in [cache_root.join(&repo_dir), cache_root.join("hub").join(&repo_dir), cache_root.join("hf").join("hub").join(&repo_dir)] {
        if let Ok(entries) = std::fs::read_dir(base.join("snapshots")) {
            let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            dirs.sort();
            out.extend(dirs);
        }
    }
    out
}

/// True when `path` holds exactly the pinned bytes. Size first (cheap), then SHA-256.
pub async fn file_matches(path: &Path, file: &ManifestFile) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(m) if m.is_file() && m.len() == file.size => {}
        _ => return false,
    }
    matches!(sha256_hex_of_file(path).await, Ok(h) if h.eq_ignore_ascii_case(&file.sha256))
}

/// Where each file of the model came from, for the log and the test record.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FetchReport {
    pub sources: Vec<(String, String)>,
}

impl FetchReport {
    pub fn summary(&self) -> String {
        let mut labels: Vec<&str> = self.sources.iter().map(|(_, l)| l.as_str()).collect();
        labels.sort();
        labels.dedup();
        labels.join(", ")
    }
}

/// Network endpoints used by `ensure_model_files`.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub release_base: String,
    pub hf_endpoint: String,
}

impl Endpoints {
    /// The real addresses, with `HF_ENDPOINT` honoured and the test seam for the release address.
    pub fn from_env() -> Endpoints {
        let automated = std::env::var_os(crate::test_state::ENV_VAR).is_some();
        let release_base = automated
            .then(|| std::env::var(TEST_RELEASE_BASE_ENV).ok())
            .flatten()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| EMBEDDING_MODEL_RELEASE_BASE_URL.to_string());
        let hf_endpoint = std::env::var(crate::network_settings::HF_ENDPOINT_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_HF_ENDPOINT.to_string());
        Endpoints { release_base, hf_endpoint }
    }
}

/// The source restriction from the test seam, only when the app runs under the test harness.
pub fn test_restriction_from_env() -> Option<String> {
    std::env::var_os(crate::test_state::ENV_VAR)?;
    std::env::var(TEST_SOURCES_ENV).ok()
}

/// Policy for the model's files: a few quick tries per source (the caller retries the whole step too).
pub fn model_download_policy() -> DownloadPolicy {
    DownloadPolicy {
        max_attempts: 3,
        base_delay: std::time::Duration::from_secs(2),
        connect_timeout: std::time::Duration::from_secs(20),
        read_timeout: std::time::Duration::from_secs(30),
    }
}

fn folder_candidates(folder: &Path, file: &ManifestFile) -> Vec<PathBuf> {
    let base = Path::new(&file.path).file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let mut v = vec![folder.join(&file.path), folder.join(&file.asset), folder.join(base)];
    v.dedup();
    v
}

async fn copy_into_place(from: &Path, dest: &Path) -> Result<(), String> {
    let partial = dest.with_extension("copying");
    tokio::fs::copy(from, &partial).await.map_err(|e| format!("could not copy {}: {e}", from.display()))?;
    let _ = tokio::fs::remove_file(dest).await;
    tokio::fs::rename(&partial, dest).await.map_err(|e| format!("could not move the copy into place: {e}"))
}

/// Make sure every pinned file is in `model_dir` with the pinned bytes, taking each from the first
/// source in `sources` that has it. Errors name every source that was tried and why it failed (the
/// raw network text is kept: the caller classifies it).
pub async fn ensure_model_files(
    manifest: &ModelManifest,
    model_dir: &Path,
    sources: &[ModelSource],
    endpoints: &Endpoints,
    policy: &DownloadPolicy,
    progress: &mut dyn FnMut(&str, u64, u64),
) -> Result<FetchReport, String> {
    let mut report = FetchReport::default();
    for file in &manifest.files {
        let dest = model_dir.join(&file.path);
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        if file_matches(&dest, file).await {
            report.sources.push((file.path.clone(), "files already on this computer".to_string()));
            continue;
        }
        // Whatever is there is not the pinned file (truncated, replaced, an old revision): remove it.
        let _ = tokio::fs::remove_file(&dest).await;

        let mut failures: Vec<String> = Vec::new();
        let mut obtained = false;
        for source in sources {
            let attempt: Result<(), String> = match &source.kind {
                SourceKind::Folder(folder) => {
                    let mut found = None;
                    for candidate in folder_candidates(folder, file) {
                        if file_matches(&candidate, file).await {
                            found = Some(candidate);
                            break;
                        }
                    }
                    match found {
                        Some(candidate) => copy_into_place(&candidate, &dest).await,
                        None => Err(format!("{} has no verified {}", folder.display(), file.path)),
                    }
                }
                SourceKind::Release => {
                    let url = manifest.release_url(&endpoints.release_base, file);
                    download_verified_with_policy(&url, &file.sha256, &dest, policy, false, |d, t| progress(&file.path, d, t)).await.map(|_| ())
                }
                SourceKind::HuggingFace => {
                    let url = manifest.huggingface_url(&endpoints.hf_endpoint, file);
                    download_verified_with_policy(&url, &file.sha256, &dest, policy, false, |d, t| progress(&file.path, d, t)).await.map(|_| ())
                }
            };
            match attempt {
                Ok(()) if file_matches(&dest, file).await => {
                    crate::app_log::log("embedding", &format!("{} taken from {}", file.path, source.label));
                    report.sources.push((file.path.clone(), source.label.clone()));
                    obtained = true;
                    break;
                }
                Ok(()) => {
                    let _ = tokio::fs::remove_file(&dest).await;
                    failures.push(format!("{}: the file did not match its checksum", source.label));
                }
                Err(e) => {
                    let _ = tokio::fs::remove_file(&dest).await;
                    crate::app_log::log("embedding", &format!("{} not available from {}: {e}", file.path, source.label));
                    failures.push(format!("{}: {e}", source.label));
                }
            }
        }
        if !obtained {
            return Err(format!("Failed to retrieve {}: {}", file.path, failures.join("; ")));
        }
    }
    Ok(report)
}

/// Build the embedding model from verified files in `model_dir`. Blocking (reads ~440 MB and
/// creates the ONNX session).
pub fn load_text_embedding(manifest: &ModelManifest, model_dir: &Path) -> Result<fastembed::TextEmbedding, String> {
    let read = |path: &str| std::fs::read(model_dir.join(path)).map_err(|e| format!("could not read {path}: {e}"));
    let tokenizer_files = fastembed::TokenizerFiles {
        tokenizer_file: read("tokenizer.json")?,
        config_file: read("config.json")?,
        special_tokens_map_file: read("special_tokens_map.json")?,
        tokenizer_config_file: read("tokenizer_config.json")?,
    };
    let _ = manifest;
    // BGE-Base-EN-v1.5 uses CLS pooling and no quantization: what fastembed's own `BGEBaseENV15` entry does.
    let model = fastembed::UserDefinedEmbeddingModel::new(read("onnx/model.onnx")?, tokenizer_files).with_pooling(fastembed::Pooling::Cls);
    fastembed::TextEmbedding::try_new_from_user_defined(model, fastembed::InitOptionsUserDefined::new()).map_err(|e| format!("{e:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[derive(Clone)]
    enum Reply {
        /// 200 with the whole body (ignores Range).
        Full(Vec<u8>),
        /// Promises the whole body, sends `n` bytes, then closes: an interrupted download.
        CutAfter(Vec<u8>, usize),
        /// Honours `Range: bytes=N-` with 206.
        Range(Vec<u8>),
        Status(u16),
    }

    /// Routes by path; each path has a queue of replies, the last one repeats. Returns the base URL and
    /// the log of (path, range header) per request.
    async fn serve(routes: Vec<(&str, Vec<Reply>)>) -> (String, Arc<Mutex<Vec<(String, Option<String>)>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let queues: Arc<Mutex<HashMap<String, Vec<Reply>>>> =
            Arc::new(Mutex::new(routes.into_iter().map(|(p, r)| (p.to_string(), r)).collect()));
        let seen: Arc<Mutex<Vec<(String, Option<String>)>>> = Arc::default();
        let seen_task = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let (queues, seen) = (queues.clone(), seen_task.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    let range = req.lines().find(|l| l.to_ascii_lowercase().starts_with("range:")).map(|l| l.to_string());
                    seen.lock().unwrap().push((path.clone(), range.clone()));
                    let reply = {
                        let mut q = queues.lock().unwrap();
                        match q.get_mut(&path) {
                            Some(list) if list.len() > 1 => list.remove(0),
                            Some(list) => list[0].clone(),
                            None => Reply::Status(404),
                        }
                    };
                    match reply {
                        Reply::Full(body) => {
                            let h = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = socket.write_all(h.as_bytes()).await;
                            let _ = socket.write_all(&body).await;
                        }
                        Reply::CutAfter(body, cut) => {
                            let h = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = socket.write_all(h.as_bytes()).await;
                            let _ = socket.write_all(&body[..cut]).await;
                        }
                        Reply::Range(body) => {
                            let start: usize = range
                                .as_deref()
                                .and_then(|r| r.split("bytes=").nth(1))
                                .and_then(|r| r.trim().trim_end_matches('-').parse().ok())
                                .unwrap_or(0);
                            let rest = &body[start.min(body.len())..];
                            let h = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                                rest.len(), start, body.len() - 1, body.len()
                            );
                            let _ = socket.write_all(h.as_bytes()).await;
                            let _ = socket.write_all(rest).await;
                        }
                        Reply::Status(code) => {
                            let h = format!("HTTP/1.1 {code} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            let _ = socket.write_all(h.as_bytes()).await;
                        }
                    }
                    let _ = socket.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn fast_policy(attempts: u32) -> DownloadPolicy {
        DownloadPolicy {
            max_attempts: attempts,
            base_delay: std::time::Duration::from_millis(5),
            connect_timeout: std::time::Duration::from_secs(5),
            read_timeout: std::time::Duration::from_secs(5),
        }
    }

    const REV: &str = "0123456789abcdef0123456789abcdef01234567";

    /// Two tiny files stand in for the model: `a/one.bin` (4000 bytes) and `two.json`.
    fn test_manifest() -> (ModelManifest, Vec<u8>, Vec<u8>) {
        let one: Vec<u8> = (0..4000u32).map(|i| (i % 251) as u8).collect();
        let two = br#"{"k":1}"#.to_vec();
        let m = ModelManifest {
            name: "tiny".into(),
            upstream_repo: "org/tiny".into(),
            revision: REV.into(),
            license: "MIT".into(),
            release_tag: "embedding-models-tiny-r1".into(),
            files: vec![
                ManifestFile { path: "a/one.bin".into(), asset: "one.bin".into(), sha256: sha(&one), size: one.len() as u64 },
                ManifestFile { path: "two.json".into(), asset: "two.json".into(), sha256: sha(&two), size: two.len() as u64 },
            ],
        };
        m.check().unwrap();
        (m, one, two)
    }

    fn endpoints(base: &str) -> Endpoints {
        Endpoints { release_base: format!("{base}/rel"), hf_endpoint: format!("{base}/hf") }
    }

    fn release_path(m: &ModelManifest, asset: &str) -> String {
        format!("/rel/{}/{asset}", m.release_tag)
    }
    fn hf_path(path: &str) -> String {
        format!("/hf/org/tiny/resolve/{REV}/{path}")
    }

    fn net_sources() -> Vec<ModelSource> {
        plan_sources(None, &[], None)
    }

    async fn run(m: &ModelManifest, dir: &Path, sources: &[ModelSource], base: &str, attempts: u32) -> Result<FetchReport, String> {
        ensure_model_files(m, dir, sources, &endpoints(base), &fast_policy(attempts), &mut |_, _, _| {}).await
    }

    #[test]
    fn the_pinned_manifest_is_complete_and_names_the_release() {
        let m = ModelManifest::pinned();
        assert_eq!(m.upstream_repo, "Xenova/bge-base-en-v1.5");
        assert_eq!(m.revision.len(), 40);
        assert_eq!(m.release_tag, "embedding-models-bge-base-en-v1.5-r1");
        assert_eq!(m.license, "MIT");
        for p in ["onnx/model.onnx", "tokenizer.json", "config.json", "special_tokens_map.json", "tokenizer_config.json"] {
            let f = m.file(p).unwrap_or_else(|| panic!("{p} missing"));
            assert_eq!(f.sha256.len(), 64);
        }
        assert_eq!(m.files.len(), 5);
        assert!(m.release_url(EMBEDDING_MODEL_RELEASE_BASE_URL, m.file("onnx/model.onnx").unwrap())
            .ends_with("/embedding-models-bge-base-en-v1.5-r1/model.onnx"));
        assert!(m.huggingface_url(DEFAULT_HF_ENDPOINT, m.file("tokenizer.json").unwrap())
            .starts_with("https://huggingface.co/Xenova/bge-base-en-v1.5/resolve/4d6cd88e18e51a5e020c2c305726d76ada9c03cf/"));
        assert_eq!(m.dir_name(), "bge-base-en-v1.5-4d6cd88e18e5");
    }

    #[test]
    fn a_manifest_that_does_not_pin_a_commit_or_hash_is_refused() {
        let good = include_str!("embedding_model_manifest.json");
        assert!(ModelManifest::parse(good).is_ok());
        assert!(ModelManifest::parse(&good.replace("4d6cd88e18e51a5e020c2c305726d76ada9c03cf", "main")).is_err());
        assert!(ModelManifest::parse(&good.replace("9bc579acdba21c253c62a9bf866891355a63ffa3442b52c8a37d75b2ccb91848", "abc")).is_err());
        assert!(ModelManifest::parse("{}").is_err());
    }

    #[test]
    fn sources_are_tried_in_the_documented_order() {
        let folder = PathBuf::from("/models");
        let legacy = vec![PathBuf::from("/old/snap")];
        let kinds: Vec<SourceKind> = plan_sources(Some(&folder), &legacy, None).into_iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![SourceKind::Folder(folder.clone()), SourceKind::Folder(legacy[0].clone()), SourceKind::Release, SourceKind::HuggingFace]);
        let kinds: Vec<SourceKind> = plan_sources(None, &[], None).into_iter().map(|s| s.kind).collect();
        assert_eq!(kinds, vec![SourceKind::Release, SourceKind::HuggingFace]);
    }

    #[test]
    fn the_test_seam_can_leave_only_some_sources() {
        let folder = PathBuf::from("/m");
        let only = |r: &str| plan_sources(Some(&folder), &[], Some(r)).into_iter().map(|s| s.kind).collect::<Vec<_>>();
        assert_eq!(only("huggingface"), vec![SourceKind::HuggingFace]);
        assert_eq!(only("release,local"), vec![SourceKind::Folder(folder.clone()), SourceKind::Release]);
        assert_eq!(only(" "), vec![SourceKind::Folder(folder.clone()), SourceKind::Release, SourceKind::HuggingFace]);
    }

    #[tokio::test]
    async fn file_matches_checks_size_and_hash() {
        let (m, one, _) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("one.bin");
        std::fs::write(&p, &one).unwrap();
        assert!(file_matches(&p, &m.files[0]).await);
        let mut tampered = one.clone();
        tampered[10] ^= 1;
        std::fs::write(&p, &tampered).unwrap();
        assert!(!file_matches(&p, &m.files[0]).await, "same size, different bytes");
        std::fs::write(&p, &one[..100]).unwrap();
        assert!(!file_matches(&p, &m.files[0]).await, "truncated");
        assert!(!file_matches(&dir.path().join("missing"), &m.files[0]).await);
    }

    #[tokio::test]
    async fn verified_files_already_present_need_no_network() {
        let (m, one, two) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/one.bin"), &one).unwrap();
        std::fs::write(dir.path().join("two.json"), &two).unwrap();
        let (base, seen) = serve(vec![]).await;
        let report = run(&m, dir.path(), &net_sources(), &base, 1).await.unwrap();
        assert!(seen.lock().unwrap().is_empty(), "nothing may be requested");
        assert_eq!(report.summary(), "files already on this computer");
    }

    #[tokio::test]
    async fn the_release_comes_before_huggingface() {
        let (m, one, two) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        let (base, seen) = serve(vec![
            (&release_path(&m, "one.bin"), vec![Reply::Full(one.clone())]),
            (&release_path(&m, "two.json"), vec![Reply::Full(two.clone())]),
        ])
        .await;
        let report = run(&m, dir.path(), &net_sources(), &base, 1).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("a/one.bin")).unwrap(), one);
        assert!(seen.lock().unwrap().iter().all(|(p, _)| p.starts_with("/rel/")), "{:?}", seen.lock().unwrap());
        assert_eq!(report.summary(), "the Plugable Chat release on GitHub");
    }

    #[tokio::test]
    async fn a_hash_mismatch_deletes_the_file_and_the_next_source_is_tried() {
        let (m, one, two) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        let mut bad = one.clone();
        bad[0] ^= 0xff; // right size, wrong bytes
        let (base, seen) = serve(vec![
            (&release_path(&m, "one.bin"), vec![Reply::Full(bad)]),
            (&release_path(&m, "two.json"), vec![Reply::Full(two.clone())]),
            (&hf_path("a/one.bin"), vec![Reply::Full(one.clone())]),
        ])
        .await;
        let report = run(&m, dir.path(), &net_sources(), &base, 1).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("a/one.bin")).unwrap(), one, "the verified copy replaced the bad one");
        assert!(!dir.path().join("a/one.download").exists(), "no leftover partial");
        let used: HashMap<_, _> = report.sources.iter().cloned().collect();
        assert_eq!(used["a/one.bin"], "Hugging Face");
        assert_eq!(used["two.json"], "the Plugable Chat release on GitHub");
        let paths: Vec<String> = seen.lock().unwrap().iter().map(|(p, _)| p.clone()).collect();
        assert!(paths.contains(&hf_path("a/one.bin")) && !paths.contains(&hf_path("two.json")), "{paths:?}");
    }

    #[tokio::test]
    async fn a_missing_release_falls_through_to_huggingface() {
        let (m, one, two) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        let (base, _) = serve(vec![
            (&release_path(&m, "one.bin"), vec![Reply::Status(404)]),
            (&hf_path("a/one.bin"), vec![Reply::Full(one)]),
            (&hf_path("two.json"), vec![Reply::Full(two)]),
        ])
        .await;
        let report = run(&m, dir.path(), &net_sources(), &base, 1).await.unwrap();
        assert!(report.sources.iter().all(|(_, l)| l == "Hugging Face"));
    }

    #[tokio::test]
    async fn an_interrupted_download_resumes_with_a_range_request_and_is_verified() {
        let (m, one, two) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        let (base, seen) = serve(vec![
            (&release_path(&m, "one.bin"), vec![Reply::CutAfter(one.clone(), 1500), Reply::Range(one.clone())]),
            (&release_path(&m, "two.json"), vec![Reply::Full(two)]),
        ])
        .await;
        run(&m, dir.path(), &net_sources(), &base, 3).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("a/one.bin")).unwrap(), one);
        let log = seen.lock().unwrap();
        let ones: Vec<_> = log.iter().filter(|(p, _)| p.ends_with("one.bin")).collect();
        assert_eq!(ones.len(), 2);
        assert!(ones[0].1.is_none());
        assert!(ones[1].1.as_deref().unwrap_or("").to_ascii_lowercase().contains("bytes=1500-"), "{:?}", ones[1]);
    }

    #[tokio::test]
    async fn a_partial_file_left_by_an_earlier_run_is_continued_not_restarted() {
        let (m, one, two) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/one.download"), &one[..2000]).unwrap();
        let (base, seen) = serve(vec![
            (&release_path(&m, "one.bin"), vec![Reply::Range(one.clone())]),
            (&release_path(&m, "two.json"), vec![Reply::Full(two)]),
        ])
        .await;
        run(&m, dir.path(), &net_sources(), &base, 1).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("a/one.bin")).unwrap(), one);
        let log = seen.lock().unwrap();
        assert!(log.iter().any(|(p, r)| p.ends_with("one.bin") && r.as_deref().unwrap_or("").contains("bytes=2000-")), "{log:?}");
    }

    #[tokio::test]
    async fn a_corrupt_file_on_disk_is_replaced() {
        let (m, one, two) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/one.bin"), b"garbage").unwrap();
        std::fs::write(dir.path().join("two.json"), &two).unwrap();
        let (base, _) = serve(vec![(&release_path(&m, "one.bin"), vec![Reply::Full(one.clone())])]).await;
        run(&m, dir.path(), &net_sources(), &base, 1).await.unwrap();
        assert_eq!(std::fs::read(dir.path().join("a/one.bin")).unwrap(), one);
    }

    #[tokio::test]
    async fn the_folder_the_user_chose_is_used_without_the_network() {
        let (m, one, two) = test_manifest();
        let user = tempfile::tempdir().unwrap();
        // flat layout (asset names) is accepted, so is the upstream layout
        std::fs::write(user.path().join("one.bin"), &one).unwrap();
        std::fs::write(user.path().join("two.json"), &two).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (base, seen) = serve(vec![]).await;
        let sources = plan_sources(Some(user.path()), &[], None);
        let report = run(&m, dir.path(), &sources, &base, 1).await.unwrap();
        assert!(seen.lock().unwrap().is_empty());
        assert!(report.summary().starts_with("the folder you chose"));
        assert_eq!(std::fs::read(dir.path().join("a/one.bin")).unwrap(), one);
    }

    #[tokio::test]
    async fn a_chosen_folder_with_the_wrong_bytes_is_ignored_and_the_download_is_used() {
        let (m, one, two) = test_manifest();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(user.path().join("one.bin"), b"someone else's model").unwrap();
        std::fs::write(user.path().join("two.json"), &two).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (base, _) = serve(vec![(&release_path(&m, "one.bin"), vec![Reply::Full(one.clone())])]).await;
        let sources = plan_sources(Some(user.path()), &[], None);
        let report = run(&m, dir.path(), &sources, &base, 1).await.unwrap();
        let used: HashMap<_, _> = report.sources.into_iter().collect();
        assert_eq!(used["a/one.bin"], "the Plugable Chat release on GitHub");
        assert!(used["two.json"].starts_with("the folder you chose"));
    }

    #[tokio::test]
    async fn when_every_source_fails_the_error_names_each_one_and_keeps_the_raw_text() {
        let (m, _, _) = test_manifest();
        let dir = tempfile::tempdir().unwrap();
        let (base, _) = serve(vec![
            (&release_path(&m, "one.bin"), vec![Reply::Status(404)]),
            (&hf_path("a/one.bin"), vec![Reply::Status(503)]),
        ])
        .await;
        let err = run(&m, dir.path(), &net_sources(), &base, 1).await.unwrap_err();
        assert!(err.starts_with("Failed to retrieve a/one.bin"), "{err}");
        assert!(err.contains("GitHub") && err.contains("Hugging Face") && err.contains("404") && err.contains("503"), "{err}");
        assert!(!dir.path().join("a/one.bin").exists());
    }

    #[test]
    fn earlier_fastembed_downloads_are_found_as_a_source() {
        let (m, _, _) = test_manifest();
        let cache = tempfile::tempdir().unwrap();
        let snap = cache.path().join("models--org--tiny").join("snapshots").join("abc");
        std::fs::create_dir_all(&snap).unwrap();
        assert_eq!(legacy_snapshot_dirs(cache.path(), &m), vec![snap]);
        assert!(legacy_snapshot_dirs(&cache.path().join("nope"), &m).is_empty());
    }

    /// Loads the real model: `PLUGABLE_TEST_EMBEDDING_MODEL_DIR=<folder with the pinned layout> cargo test --lib real_model -- --ignored`.
    #[test]
    #[ignore = "needs the real 440 MB model files"]
    fn real_model_loads_through_the_user_defined_path_and_ranks_sensibly() {
        let dir = PathBuf::from(std::env::var("PLUGABLE_TEST_EMBEDDING_MODEL_DIR").expect("set PLUGABLE_TEST_EMBEDDING_MODEL_DIR"));
        let manifest = ModelManifest::pinned();
        for f in &manifest.files {
            let path = dir.join(&f.path);
            let ok = tokio::runtime::Runtime::new().unwrap().block_on(file_matches(&path, f));
            assert!(ok, "{} does not match the manifest", f.path);
        }
        let model = load_text_embedding(manifest, &dir).unwrap();
        let out = model.embed(vec!["How do I reset my password?", "Steps to change your account password", "Chicago crime statistics by district"], None).unwrap();
        crate::actors::foundry::embedding_init::validate_embeddings(&out).unwrap();
        assert_eq!(out[0].len(), 768);
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        assert!(dot(&out[0], &out[1]) > dot(&out[0], &out[2]), "related sentences must be closer");
    }
}
