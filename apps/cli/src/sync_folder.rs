//! "Sync this folder now" hand-offs (issue #123): `sync-folder [OPTIONS] FOLDER` and
//! `open-url [OPTIONS] URL`, the same as `run [OPTIONS] folder.synchronize path=FOLDER` (with
//! `--connect`, in the running app; else headless, on `--library DIR`). A URL is one of the
//! app's links (`<scheme>://sync?path=…`, see `lcc_engine::link`): only the commands a link may
//! name are run.

use serde_json::Value;

/// Split `args` into the `run` options and the rest.
fn split(args: &[String]) -> Result<(Vec<String>, Vec<String>), String> {
    let mut opts = Vec::new();
    let mut rest = Vec::new();
    let mut i = 0;
    while let Some(a) = args.get(i) {
        if a.starts_with("--") {
            opts.push(a.clone());
            let takes = matches!(a.as_str(), "--library" | "--import")
                || (a == "--connect"
                    && args
                        .get(i + 1)
                        .is_some_and(|n| !n.is_empty() && !n.contains('=') && (n.contains(':') || n.chars().all(|c| c.is_ascii_digit()))));
            if takes {
                i += 1;
                opts.push(args.get(i).cloned().ok_or_else(|| format!("{a} needs a value"))?);
            }
        } else {
            rest.push(a.clone());
        }
        i += 1;
    }
    Ok((opts, rest))
}

fn run_line(mut opts: Vec<String>, command: &str, params: &Value) -> Vec<String> {
    opts.push(command.to_string());
    opts.push(params.to_string());
    opts
}

/// `sync-folder [OPTIONS] FOLDER`: the `run` arguments.
pub fn sync_folder_args(args: &[String]) -> Result<Vec<String>, String> {
    let (opts, rest) = split(args)?;
    let [folder] = rest.as_slice() else {
        return Err("sync-folder: give one folder of the library".into());
    };
    let folder = std::path::absolute(folder).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| folder.clone());
    Ok(run_line(opts, "folder.synchronize", &serde_json::json!({"path": folder})))
}

/// `open-url [OPTIONS] URL`: the `run` arguments.
pub fn open_url_args(args: &[String]) -> Result<Vec<String>, String> {
    let (opts, rest) = split(args)?;
    let [url] = rest.as_slice() else {
        return Err("open-url: give one link".into());
    };
    let link = lcc_engine::link::parse(url).map_err(|e| format!("open-url: {e}"))?;
    Ok(run_line(opts, link.command, &link.params))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn builds_run_lines() {
        let r = sync_folder_args(&s(&["--connect", "18101", "/p/trip"])).unwrap();
        assert_eq!(&r[..3], &s(&["--connect", "18101", "folder.synchronize"])[..]);
        let p: Value = serde_json::from_str(&r[3]).unwrap();
        assert!(p["path"].as_str().unwrap().ends_with("trip"));
        let url = lcc_engine::link::sync_link("/p/a b");
        let r = open_url_args(&s(&["--library", "/l", &url])).unwrap();
        assert_eq!(&r[..3], &s(&["--library", "/l", "folder.synchronize"])[..]);
        let p: Value = serde_json::from_str(&r[3]).unwrap();
        assert_eq!(p["path"], "/p/a b");
    }

    #[test]
    fn bad_arguments_are_errors() {
        assert!(sync_folder_args(&s(&[])).is_err());
        assert!(sync_folder_args(&s(&["a", "b"])).is_err());
        assert!(sync_folder_args(&s(&["--library"])).is_err());
        assert!(open_url_args(&s(&["https://example.com"])).is_err());
        assert!(open_url_args(&s(&[])).is_err());
    }
}
