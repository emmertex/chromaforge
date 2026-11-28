//! The shared AI backend (#491): `ml.status`, `ml.enable`, `ml.models.*`, `ml.pump`, `ml.index`, `ml.orientation.*` (#490), and
//! `search.semantic`.
//! The queue, encoder and embedding store live in `crate::ml`.

use super::{CommandSpec, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "ml.status",
            "AI Backend Status",
            [],
            None,
            "{preference?: auto | cpu, device?: string} → {device: {kind: gpu | cpu, name, whyCpu} | null (until the first job), preference, deviceChoice, availableDevices, pending, running (the running job's key), done, failed, cancelled, threaded, enabled, modelId, modelInstalled, indexedPhotos, unindexedPhotos, onnxRuntime: {state: notLoaded | ready | unavailable, compiledIn, version?, library?, providers?, webgpu?, webgpuMissing?, reason?}}: the shared queue the AI features (search embeddings, scores, culling, orientation) run their models through, on a worker thread with one `mlIndex` row in the activity stack while busy (its ✕ cancels the batch). `preference: cpu` keeps the jobs on the CPU; `auto` (the default, or `{ENV_PREFIX}_ML_DEVICE`) takes the GPU when GPU rendering may use one, and a job that fails on the GPU runs again on the CPU",
            super::always,
            |s, p| s.ml_status(p)
        ),
        cmd!(
            query "ml.enable",
            "Enable Background AI Indexing",
            [],
            None,
            "{enabled: bool} → {enabled}: turn semantic search and its background photo indexing on or off (saved in the models folder; on unless switched off, except on Android, where it is off until switched on). Off: nothing is indexed, `search.semantic` and `library.filter {semantic}` are refused, and a description filter on the grid is cleared; the index already made is kept",
            super::always,
            |s, p| s.ml_enable(p)
        ),
        cmd!(
            query "ml.indexStatus",
            "Search Indexing Status",
            [],
            None,
            "{} → {enabled, paused, state: off | idle | indexing | paused | complete, done, total, indexedPhotos, unindexedPhotos}: how far background indexing for semantic search is",
            super::always,
            |s, _| s.ml_progress()
        ),
        cmd!(
            "ml.pause",
            "Pause Search Indexing",
            [],
            None,
            "{} → {enabled, paused}: pause background indexing for semantic search (the photo being embedded waits, nothing new starts); it stays paused across restarts until `ml.resume`",
            super::always,
            |s, _| s.ml_set_paused("ml.pause", true)
        ),
        cmd!(
            "ml.resume",
            "Resume Search Indexing",
            [],
            None,
            "{} → {enabled, paused}: resume paused background indexing for semantic search",
            super::always,
            |s, _| s.ml_set_paused("ml.resume", false)
        ),
        cmd!(
            "ml.clearIndex",
            "Clear Search Index",
            [],
            None,
            "{} → as ml.indexStatus: drop this library's semantic search index (every photo's embedding, of every model) and pause indexing. Destructive: frontends ask first",
            super::always,
            |s, p| s.ml_clear_index(p)
        ),
        cmd!(
            "ml.restart",
            "Restart Search Indexing",
            [],
            None,
            "{} → as ml.indexStatus: drop the semantic search index (as ml.clearIndex) and index the library again from the start (resumes paused indexing)",
            super::always,
            |s, p| s.ml_restart(p)
        ),
        cmd!(
            query "ml.models.list",
            "List AI Models",
            [],
            None,
            "{} → {models: [{id, name, role, versionTag, embedDim, sizeBytes, licence: {spdx, commercial, url, attribution}, notes, installed, selected, download}], selectedEmbedder}: known and installed AI models in `<config>/ml-models`",
            super::always,
            |s, p| s.ml_models_list(p)
        ),
        cmd!(
            query "ml.models.install",
            "Install AI Model Directory",
            [],
            None,
            "{path: string, select?: true} → model manifest info: verify `manifest.json` and every declared file's size and SHA-256 in `path`, install into `<config>/ml-models/<id>`, and select as the active embedder when `select` is true",
            super::always,
            |s, p| s.ml_models_install(p)
        ),
        cmd!(
            query "ml.models.select",
            "Select Active AI Model",
            [],
            None,
            "{id: string | null} → {selectedEmbedder}: choose which installed embedder model is used for background indexing and semantic search",
            super::always,
            |s, p| s.ml_models_select(p)
        ),
        cmd!(
            query "ml.models.remove",
            "Remove Installed AI Model",
            [],
            None,
            "{id: string} → {id, removed: bool}: delete an installed AI model directory from `<config>/ml-models/<id>`",
            super::always,
            |s, p| s.ml_models_remove(p)
        ),
        cmd!(
            query "ml.models.download",
            "Download Known AI Model",
            [],
            None,
            "{id: string, acceptLicence: true} → {id, name, sizeBytes, licence, state}: download a known open-weights AI model in the background with SHA-256 verification and a cancellable `download` row in the activity stack",
            super::always,
            |s, p| s.ml_models_download(p)
        ),
        cmd!(
            query "ml.models.downloads",
            "AI Model Downloads Status",
            [],
            None,
            "{} → {downloads: [{id, state, bytes?, total?, error?}]}: status of AI model downloads started in this session",
            super::always,
            |s, p| s.ml_models_downloads(p)
        ),
        cmd!(
            query "ml.models.downloadCancel",
            "Cancel AI Model Download",
            [],
            None,
            "{id: string} → {id, cancelled: bool}: cancel a running AI model download or clear a finished/failed one",
            super::always,
            |s, p| s.ml_models_download_cancel(p)
        ),
        cmd!(
            query "ml.pump",
            "Pump Background AI Indexing",
            [],
            None,
            "{pace?: pause | light | normal | full} → {ready, enabled, paused, added, indexedPhotos, pending, running, done, failed}: drain finished embeddings into the library's embedding store and queue unindexed photos on the background AI worker",
            super::always,
            |s, p| s.ml_pump(p)
        ),
        cmd!(
            query "ml.index",
            "Index Photos for AI Features",
            [],
            None,
            "{ids?: [PhotoId], wait?: false, budgetMs?: 30000} → {queued, indexedPhotos, unindexedPhotos, inFlight}: queue unindexed library photos (or `ids`) on the AI worker and optionally wait up to `budgetMs` for completion",
            super::always,
            |s, p| s.ml_index(p)
        ),
        cmd!(
            query "ml.orientation.status",
            "Orientation Detection Status",
            [],
            None,
            "{} → {classifier: {available, origin?: fitted | builtIn, embedder?, minConfidence?, source?, report?, reason?}, running: {checked, total} | null, last: (the last finished photo.autoOrient answer) | null}: which classifier AI orientation detection uses and how its last run went",
            super::always,
            |s, p| s.ml_orientation_status(p)
        ),
        cmd!(
            query "ml.orientation.fit",
            "Fit Orientation Classifier",
            [],
            None,
            "{ids?: [PhotoId], out?: path, steps?: 600, budgetMs?: 21600000} → {path, embedder, photos, failed, notReached, minConfidence, report: {photos, holdoutPhotos, viewAccuracy, photoAccuracy, thresholds: [{minConfidence, coverage, accuracy}]}}: fit AI orientation detection's classifier on the library's photos (or `ids`) as they are shown now, taken to be upright: each is rendered, turned four ways and embedded by the active CLIP model, then a 4-way linear classifier is fitted, measured on one photo in five held out, and saved as `<models>/orientation-<model id>.json` (or `out`), where photo.autoOrient uses it. Waits until done (up to `budgetMs`). Only the fitted numbers are kept, never the photos",
            super::always,
            |s, p| s.ml_orientation_fit(p)
        ),
        cmd!(
            query "search.semantic",
            "Semantic Photo Search",
            [],
            None,
            "{text: string, topK?: 50, threshold?: -1.0, scope?: all | filtered | selected, filter?: false, clear?: false} → {query, modelId, hits: [{id, score}], indexedPhotos, pendingPhotos}: encode `text` with the active CLIP text tower at interactive priority, rank library photos by cosine similarity, and optionally filter the library grid (`filter: true`)",
            super::always,
            |s, p| s.search_semantic(p)
        ),
    ]
}
