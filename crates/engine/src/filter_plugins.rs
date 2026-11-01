//! Installed pixel-filter plug-ins (PhotoCraft ABI, [`lcc_filters`]): a process-wide registry
//! beside the general plug-in manager, in the same folder (`cmd::plugins::plugins_dir`). The
//! `plugin.*` commands route modules that [`lcc_filters::is_filter_module`] recognises here, and
//! every render (preview, loupe, export, CLI, MCP) resolves a photo's filter stack through
//! [`EngineFilterHost`], the pipeline's default host.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use lcc_filters::{Filter, Limits, Registry};
use serde_json::{Value, json};

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// The process-wide filter registry, loaded from the plug-ins folder on first use (kept in
/// memory in unit tests), with the pipeline's default host pointed at it.
pub fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let r = Registry::new(Limits::default());
        if !cfg!(test)
            && let Some(dir) = crate::cmd::plugins::plugins_dir()
        {
            load_dir(&r, &dir);
        }
        lcc_pipeline::plugin_filters::set_default_host(Some(Arc::new(EngineFilterHost)));
        r
    })
}

/// Installs every filter module in `dir` (others are the general manager's business).
fn load_dir(r: &Registry, dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> =
        rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("wasm"))).collect();
    files.sort();
    files.truncate(1000);
    for f in files {
        let Ok(bytes) = read_limited(&f) else { continue };
        if !lcc_filters::is_filter_module(&bytes) {
            continue;
        }
        match Filter::load(&bytes, Limits::default()) {
            Ok(filter) => {
                r.install(filter.with_source(f.display().to_string()));
            }
            Err(e) => log::warn!("filter plug-in {}: {e}", f.display()),
        }
    }
}

fn read_limited(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let max = Limits::default().max_module_bytes as u64;
    if !std::fs::metadata(path)?.is_file() {
        return Err(std::io::Error::other("not a file"));
    }
    let mut b = Vec::new();
    std::fs::File::open(path)?.take(max + 1).read_to_end(&mut b)?;
    Ok(b)
}

/// Resolves stack entries to the installed filters.
pub struct EngineFilterHost;

impl lcc_pipeline::plugin_filters::FilterHost for EngineFilterHost {
    fn resolve(&self, plugin: &str, _module_hash: &str) -> Result<Arc<Filter>, String> {
        registry().get(plugin).ok_or_else(|| format!("plug-in {plugin:?} is not installed"))
    }
}

/// Whether `path` holds a filter module (read errors: no).
pub fn is_filter_file(path: &Path) -> bool {
    read_limited(path).is_ok_and(|b| lcc_filters::is_filter_module(&b))
}

/// What a module file is to the filter registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterFile {
    /// Not a pixel filter (or unreadable).
    No,
    /// A pixel filter that loads.
    Loads,
    /// A pixel filter the registry refuses, and why.
    Refused(String),
}

/// What `path` is to the filter registry, remembered while the file's size and modification time
/// stay the same: `plugin.list` sorts the modules that didn't load by kind and gives a filter's own
/// error, and the Plug-in Manager lists them every frame.
pub fn filter_file_cached(path: &Path) -> FilterFile {
    type Seen = std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>, FilterFile)>;
    static SEEN: std::sync::Mutex<Option<Seen>> = std::sync::Mutex::new(None);
    let Ok(meta) = std::fs::metadata(path) else { return FilterFile::No };
    let stamp = (meta.len(), meta.modified().ok());
    let mut g = SEEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let seen = g.get_or_insert_with(Seen::new);
    if let Some((len, modified, what)) = seen.get(path)
        && (*len, *modified) == stamp
    {
        return what.clone();
    }
    let what = match read_limited(path) {
        Ok(bytes) if lcc_filters::is_filter_module(&bytes) => match Filter::load(&bytes, Limits::default()) {
            Ok(_) => FilterFile::Loads,
            Err(e) => FilterFile::Refused(e.to_string()),
        },
        _ => FilterFile::No,
    };
    if seen.len() >= 1000 {
        seen.clear();
    }
    seen.insert(path.to_path_buf(), (stamp.0, stamp.1, what.clone()));
    what
}

/// Loads, copies into the plug-ins folder (when there is one) and installs a filter module.
pub fn install_file(path: &Path) -> Result<Arc<Filter>, String> {
    let bytes = read_limited(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let filter = Filter::load(&bytes, Limits::default()).map_err(|e| e.to_string())?;
    let r = registry();
    let filter = match (!cfg!(test)).then(crate::cmd::plugins::plugins_dir).flatten() {
        Some(dir) => {
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let dest = dir.join(format!("{}.wasm", filter.id()));
            let tmp = dir.join(format!(".{}.wasm.tmp", filter.id()));
            std::fs::write(&tmp, &bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
            std::fs::rename(&tmp, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            filter.with_source(dest.display().to_string())
        }
        None => filter,
    };
    Ok(r.install(filter))
}

/// Tests only: installs a filter module from `bytes` into the registry, with no file in the
/// plug-ins folder. Compiled into this crate's tests and, through the `test-support` feature, other
/// crates' tests; no app build has it, and no command, control request or MCP tool reaches it.
#[cfg(any(test, feature = "test-support"))]
pub fn install_in_memory(bytes: &[u8]) -> Result<Arc<Filter>, String> {
    let filter = Filter::load(bytes, Limits::default()).map_err(|e| e.to_string())?;
    Ok(registry().install(filter))
}

/// Uninstalls filter `id` and deletes its module file; false when no such filter is installed.
pub fn uninstall(id: &str) -> Result<bool, String> {
    let r = registry();
    let Some(f) = r.get(id) else { return Ok(false) };
    r.remove(id);
    if let Some(src) = f.source() {
        match std::fs::remove_file(src) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{src}: {e}")),
        }
    }
    Ok(true)
}

/// Plug-ins known to want the Source stage that don't declare `stage` in their manifest yet.
const STAGE_HINTS: &[(&str, lcc_develop::FilterStage)] = &[
    ("io.github.smyrgeorge.film-lab.convert", lcc_develop::FilterStage::Source),
    ("io.github.smyrgeorge.film-lab.convert-advanced", lcc_develop::FilterStage::Source),
];

/// The stage a new entry of `f` starts at: the manifest's `stage`, else the built-in hint, else Display.
pub fn preferred_stage(f: &Filter) -> lcc_develop::FilterStage {
    use lcc_develop::FilterStage;
    match f.manifest().stage {
        Some(lcc_filters::Stage::Source) => FilterStage::Source,
        Some(lcc_filters::Stage::Display) => FilterStage::Display,
        None => STAGE_HINTS.iter().find(|(id, _)| *id == f.id()).map(|(_, s)| *s).unwrap_or_default(),
    }
}

/// How `plugin.list` shows a filter plug-in. `params` is an object (no key order in JSON), so
/// `paramOrder` lists its keys in the manifest's order; each spec carries its display `label`.
pub fn info(f: &Filter) -> Value {
    let m = f.manifest();
    let mut params = serde_json::Map::new();
    for (k, spec) in &m.params {
        let mut v = serde_json::to_value(spec).unwrap_or(Value::Null);
        if let Some(o) = v.as_object_mut() {
            o.insert("label".into(), Value::from(m.param_label(k)));
            if let Some(g) = m.param_groups(k) {
                o.insert("groups".into(), Value::from(g.to_vec()));
            }
        }
        params.insert(k.clone(), v);
    }
    let stage = match preferred_stage(f) {
        lcc_develop::FilterStage::Source => "source",
        lcc_develop::FilterStage::Display => "display",
    };
    json!({
        "id": m.id,
        "name": m.name,
        "version": m.version,
        "kind": "filter",
        "description": m.description,
        "author": m.author,
        "params": params,
        "paramOrder": m.params.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        "stage": stage,
        "overlap": m.overlap,
        "moduleHash": f.module_hash(),
        "analyze": f.has_analyze(),
        "analyzeArea": m.analyze_area,
        "baseTone": m.base_tone,
        "size": f.size(),
        "enabled": true,
    })
}

/// Module files of installed filters (so the general manager's load failures for them, which
/// are expected, aren't listed).
pub fn sources() -> Vec<String> {
    registry().list().iter().filter_map(|f| f.source().map(str::to_string)).collect()
}
