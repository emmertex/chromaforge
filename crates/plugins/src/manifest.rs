//! The plug-in manifest (`lcc_manifest`, UTF-8 JSON): identity, requested capabilities and what
//! the plug-in contributes (commands with declarative dialogs, hooks, a publish service, settings).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::permissions::Permissions;
use crate::{Error, Result};

/// Longest manifest the host reads.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
/// Most commands one plug-in may declare.
pub const MAX_COMMANDS: usize = 64;
/// Most fields one dialog may have.
pub const MAX_FIELDS: usize = 32;
/// Most export destinations one plug-in may declare.
pub const MAX_DESTINATIONS: usize = 16;
/// Most custom metadata fields one plug-in may declare.
pub const MAX_METADATA_FIELDS: usize = 32;
/// Most host features one plug-in may require.
pub const MAX_REQUIRES: usize = 64;
/// Longest `version` the host reads.
pub const MAX_VERSION_BYTES: usize = 64;
/// Most options a choice field may have.
pub const MAX_OPTIONS: usize = 64;
/// Longest option of a choice field filled at run time.
pub const MAX_OPTION_BYTES: usize = 1024;
/// Longest text value a field takes.
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// The host features this version of the app has, by name. A plug-in lists the ones it uses in
/// `requires` and isn't installed (or loaded) where one is missing. Every manifest field,
/// permission group or host call added from now on gets a name here; what the host had before
/// `requires` existed is the baseline and has none.
pub const HOST_FEATURES: &[&str] = &[
    // `programs` in the permissions, `program.list` / `program.start` and the `program.end` hook
    "programs",
    // `settings` in the manifest, `settings.get` / `settings.set`, dialogs that remember their
    // values (`remember`), and the `command.prepare` / `settings.prepare` hooks
    "settings",
    // `libraryAdd` in the permissions and the `library.add` host call; `source` in the `command`
    // and `command.prepare` requests
    "libraryAdd",
    // `job` on a command and the `job.*` host calls (progress, cancel, `{continue: state}`)
    "jobs",
    // `pixels` in the permissions and the `photo.pixels` host call
    "pixels",
    // `crop` in the permissions and the `develop.setCrop` host call
    "crop",
];

/// The manifest a plug-in returns from `lcc_manifest`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Stable id, `[A-Za-z0-9._-]{1,64}` (reverse-DNS style: `org.example.geotag`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Semver `MAJOR.MINOR.PATCH` (checked on install, see [`Manifest::semver`]).
    #[serde(default)]
    pub version: String,
    /// Host features the plug-in uses ([`HOST_FEATURES`]): a minimum, never a maximum.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    /// Capabilities the plug-in asks for; the user grants them on install.
    #[serde(default)]
    pub permissions: Permissions,
    /// Menu commands.
    #[serde(default)]
    pub commands: Vec<CommandDecl>,
    #[serde(default)]
    pub hooks: Hooks,
    /// A publish service this plug-in provides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishDecl>,
    /// Export destinations: after the app writes the files, the plug-in delivers them
    /// (`{"hook": "export.destination"}`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub export_destinations: Vec<DestinationDecl>,
    /// Custom metadata fields shown in the Metadata panel; their values live in the plug-in's
    /// own namespace (the key is the field key).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_fields: Vec<MetadataField>,
    /// Settings, shown on the plug-in's page and kept for the whole app (`settings.get`); a field
    /// with `prepare` has its options filled by the `settings.prepare` hook.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings: Vec<Field>,
}

/// An export destination ("Upload to my server").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DestinationDecl {
    /// `[A-Za-z0-9._-]{1,64}`, unique within the plug-in.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Settings asked in the Export dialog; their values arrive as `args`.
    #[serde(default)]
    pub dialog: Vec<Field>,
}

/// A custom metadata field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataField {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: FieldKind,
    /// Shown but not editable by the user (the plug-in fills it).
    #[serde(default)]
    pub read_only: bool,
}

impl MetadataField {
    /// The field as a dialog field (same checks, same coercion).
    pub fn as_field(&self) -> Field {
        Field { key: self.key.clone(), label: self.label.clone(), kind: self.kind.clone(), remember: true, prepare: false }
    }
}

/// A menu command. It runs as the engine command `plugin.run {plugin, command, args}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDecl {
    /// `[A-Za-z0-9._-]{1,64}`, unique within the plug-in.
    pub id: String,
    pub label: String,
    /// Where the UI lists it, `>`-separated under the Plug-in Extras menu (`""` = top level).
    #[serde(default)]
    pub menu: String,
    /// A dialog asked before the command runs; its values arrive as `args`.
    #[serde(default)]
    pub dialog: Vec<Field>,
    /// Ask the plug-in first (`{"hook": "command.prepare"}`): it shows the dialog with options
    /// and defaults of its own, runs the command without it, or explains why it can't run.
    #[serde(default, skip_serializing_if = "is_false")]
    pub prepare: bool,
    /// Runs as a job (host feature `jobs`): on a worker, with a row in the activity stack, its
    /// progress reported through the `job.*` host calls, and continued with `{continue: state}`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub job: bool,
}

/// Which hooks the plug-in implements.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hooks {
    /// Post-process exported files (`{"hook": "export"}`).
    #[serde(default)]
    pub export: bool,
    /// Suggest metadata and keywords for photos (`{"hook": "metadata"}`).
    #[serde(default)]
    pub metadata: bool,
}

/// A publish service.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishDecl {
    /// Service id, unique among the plug-in's services.
    pub id: String,
    /// Display name ("WebDAV").
    pub name: String,
}

/// One dialog or settings field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: FieldKind,
    /// A dialog field opens with the value it was last run with (`false`: always its default).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub remember: bool,
    /// A settings field whose options and default the `settings.prepare` hook fills.
    #[serde(default, skip_serializing_if = "is_false")]
    pub prepare: bool,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A field's type and default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FieldKind {
    Text {
        #[serde(default)]
        default: String,
    },
    Number {
        min: f64,
        max: f64,
        default: f64,
    },
    Bool {
        #[serde(default)]
        default: bool,
    },
    /// `options` may be empty in a `prepare` command's dialog or a `prepare` settings field: the
    /// plug-in gives them at run time.
    Choice {
        #[serde(default)]
        options: Vec<String>,
        #[serde(default)]
        default: Option<String>,
    },
}

/// A plug-in version as semver (`MAJOR.MINOR.PATCH`, optionally `-pre` and `+build`), or why it isn't one.
pub fn parse_version(v: &str) -> std::result::Result<semver::Version, String> {
    let form = "it must be MAJOR.MINOR.PATCH, e.g. 1.4.0";
    if v.is_empty() {
        return Err(format!("no version: {form}"));
    }
    if v.len() > MAX_VERSION_BYTES {
        return Err(format!("version is {} bytes (limit {MAX_VERSION_BYTES}): {form}", v.len()));
    }
    semver::Version::parse(v).map_err(|e| format!("version {v:?} is not semver ({e}): {form}"))
}

/// At most 64 characters of `s`, for messages about hostile input.
fn short(s: &str) -> String {
    s.chars().take(64).collect()
}

pub(crate) fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl Manifest {
    /// Parses and validates a manifest.
    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Manifest(format!("{} bytes (limit {MAX_MANIFEST_BYTES})", bytes.len())));
        }
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| Error::Manifest(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: String| Err(Error::Manifest(m));
        if !valid_id(&self.id) {
            return bad(format!("id {:?} must be 1-64 characters of A-Z a-z 0-9 . _ -", self.id));
        }
        if self.name.trim().is_empty() || self.name.len() > 128 {
            return bad("name must be 1-128 bytes".into());
        }
        if self.requires.len() > MAX_REQUIRES {
            return bad(format!("at most {MAX_REQUIRES} names in requires"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for r in &self.requires {
            if !valid_id(r) || !seen.insert(r.as_str()) {
                return bad(format!("requires: {:?} is invalid or repeated (names are 1-64 characters of A-Z a-z 0-9 . _ -)", short(r)));
            }
        }
        self.permissions.validate().map_err(Error::Manifest)?;
        if self.commands.len() > MAX_COMMANDS {
            return bad(format!("at most {MAX_COMMANDS} commands"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.commands {
            if !valid_id(&c.id) || !seen.insert(c.id.as_str()) {
                return bad(format!("command id {:?} is invalid or repeated", c.id));
            }
            if c.label.trim().is_empty() || c.label.len() > 128 || c.menu.len() > 256 {
                return bad(format!("command {:?}: label must be 1-128 bytes, menu at most 256", c.id));
            }
            if c.dialog.len() > MAX_FIELDS {
                return bad(format!("command {:?}: at most {MAX_FIELDS} dialog fields", c.id));
            }
            if c.job && !self.requires.iter().any(|r| r == "jobs") {
                return bad(format!("command {:?}: a job needs \"jobs\" in `requires`", c.id));
            }
            for f in &c.dialog {
                f.validate(c.prepare).map_err(|e| Error::Manifest(format!("command {:?}: {e}", c.id)))?;
                if f.prepare {
                    return bad(format!("command {:?}: field {:?}: `prepare` is for settings fields (a command takes `prepare: true`)", c.id, f.key));
                }
            }
        }
        if let Some(p) = &self.publish
            && (!valid_id(&p.id) || p.name.trim().is_empty())
        {
            return bad("publish service needs an id and a name".into());
        }
        if self.export_destinations.len() > MAX_DESTINATIONS {
            return bad(format!("at most {MAX_DESTINATIONS} export destinations"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for d in &self.export_destinations {
            if !valid_id(&d.id) || !seen.insert(d.id.as_str()) || d.name.trim().is_empty() || d.name.len() > 128 {
                return bad(format!("export destination {:?}: needs a unique id and a 1-128 byte name", d.id));
            }
            if d.dialog.len() > MAX_FIELDS {
                return bad(format!("export destination {:?}: at most {MAX_FIELDS} dialog fields", d.id));
            }
            for f in &d.dialog {
                f.validate(false).map_err(|e| Error::Manifest(format!("export destination {:?}: {e}", d.id)))?;
                if f.prepare {
                    return bad(format!("export destination {:?}: field {:?}: `prepare` is for settings fields", d.id, f.key));
                }
            }
        }
        if self.metadata_fields.len() > MAX_METADATA_FIELDS {
            return bad(format!("at most {MAX_METADATA_FIELDS} metadata fields"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.metadata_fields {
            if !seen.insert(f.key.as_str()) || f.label.trim().is_empty() {
                return bad(format!("metadata field {:?}: needs a unique key and a label", f.key));
            }
            f.as_field().validate(false).map_err(|e| Error::Manifest(format!("metadata field: {e}")))?;
        }
        if self.settings.len() > MAX_FIELDS {
            return bad(format!("at most {MAX_FIELDS} settings"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for f in &self.settings {
            if !seen.insert(f.key.as_str()) || f.label.trim().is_empty() {
                return bad(format!("setting {:?}: needs a unique key and a label", short(&f.key)));
            }
            f.validate(f.prepare).map_err(|e| Error::Manifest(format!("setting: {e}")))?;
        }
        Ok(())
    }

    /// The features in `requires` that `known` lacks, in order.
    pub fn missing_features(&self, known: &[impl AsRef<str>]) -> Vec<String> {
        self.requires.iter().filter(|r| !known.iter().any(|k| k.as_ref() == r.as_str())).cloned().collect()
    }

    /// `version` as semver, or why it isn't one.
    pub fn semver(&self) -> std::result::Result<semver::Version, String> {
        parse_version(&self.version)
    }

    pub fn command(&self, id: &str) -> Option<&CommandDecl> {
        self.commands.iter().find(|c| c.id == id)
    }

    pub fn destination(&self, id: &str) -> Option<&DestinationDecl> {
        self.export_destinations.iter().find(|d| d.id == id)
    }

    pub fn metadata_field(&self, key: &str) -> Option<&MetadataField> {
        self.metadata_fields.iter().find(|f| f.key == key)
    }

    pub fn setting(&self, key: &str) -> Option<&Field> {
        self.settings.iter().find(|f| f.key == key)
    }
}

impl Field {
    /// `run_time`: the plug-in may give a choice's options at run time, so it may declare none.
    fn validate(&self, run_time: bool) -> std::result::Result<(), String> {
        if !valid_id(&self.key) || self.label.len() > 128 {
            return Err(format!("field {:?}: bad key or label", short(&self.key)));
        }
        match &self.kind {
            FieldKind::Number { min, max, default } if !(min.is_finite() && max.is_finite() && min <= max && (min..=max).contains(&default)) => {
                Err(format!("field {:?}: needs finite min <= default <= max", self.key))
            }
            FieldKind::Choice { options, .. } if options.is_empty() && !run_time => {
                Err(format!("field {:?}: 1-{MAX_OPTIONS} options (none only when the plug-in gives them at run time: `prepare`)", self.key))
            }
            FieldKind::Choice { options, default } if options.len() > MAX_OPTIONS || default.as_ref().is_some_and(|d| !options.contains(d)) => {
                Err(format!("field {:?}: at most {MAX_OPTIONS} options, default among them", self.key))
            }
            FieldKind::Text { default } if default.len() > 4096 => Err(format!("field {:?}: default too long", self.key)),
            _ => Ok(()),
        }
    }

    /// A choice whose options the plug-in gives at run time (it declares none).
    pub fn options_at_run_time(&self) -> bool {
        matches!(&self.kind, FieldKind::Choice { options, .. } if options.is_empty())
    }

    /// The field with the options and default a `prepare` hook gave (`{options?, default?}`), or
    /// why they don't fit it: options only for a choice (1-64 non-empty texts of at most 1024
    /// bytes), a default the field (with those options) takes.
    pub fn prepared(&self, o: &Value) -> std::result::Result<Field, String> {
        let o = o.as_object().ok_or_else(|| format!("`{}`: expected {{options?, default?}}", self.key))?;
        if let Some(k) = o.keys().find(|k| !matches!(k.as_str(), "options" | "default")) {
            return Err(format!("`{}`: unknown key {:?}", self.key, short(k)));
        }
        if self.options_at_run_time() && !o.contains_key("options") {
            return Err(format!("`{}`: no options given", self.key));
        }
        let mut f = self.clone();
        if let Some(opts) = o.get("options") {
            let FieldKind::Choice { options, default } = &mut f.kind else {
                return Err(format!("`{}`: only a choice field takes options", self.key));
            };
            let list = opts.as_array().filter(|a| (1..=MAX_OPTIONS).contains(&a.len()));
            let list = list.ok_or_else(|| format!("`{}`: options must be a list of 1-{MAX_OPTIONS} texts", self.key))?;
            let mut new = Vec::with_capacity(list.len());
            for v in list {
                let t = v.as_str().filter(|t| !t.is_empty() && t.len() <= MAX_OPTION_BYTES);
                new.push(t.ok_or_else(|| format!("`{}`: each option must be a text of 1-{MAX_OPTION_BYTES} bytes", self.key))?.to_string());
            }
            // the declared default stays only if it is still offered
            if default.as_ref().is_some_and(|d| !new.contains(d)) {
                *default = None;
            }
            *options = new;
        }
        if let Some(d) = o.get("default") {
            let v = f.coerce(d)?;
            match &mut f.kind {
                FieldKind::Text { default } => {
                    let t = v.as_str().unwrap_or_default();
                    // the same cap as a declared default
                    if t.len() > 4096 {
                        return Err(format!("`{}`: default too long", self.key));
                    }
                    *default = t.to_string();
                }
                FieldKind::Number { default, .. } => *default = v.as_f64().unwrap_or(*default),
                FieldKind::Bool { default } => *default = v.as_bool().unwrap_or(*default),
                FieldKind::Choice { default, .. } => *default = v.as_str().map(str::to_string),
            }
        }
        Ok(f)
    }

    pub fn default_value(&self) -> Value {
        match &self.kind {
            FieldKind::Text { default } => Value::from(default.clone()),
            FieldKind::Number { default, .. } => Value::from(*default),
            FieldKind::Bool { default } => Value::from(*default),
            FieldKind::Choice { options, default } => Value::from(default.clone().or_else(|| options.first().cloned()).unwrap_or_default()),
        }
    }

    /// A value written as text (an XMP sidecar's `lcpf:value`) read as this field's type, then
    /// checked like [`Field::coerce`].
    pub fn from_text(&self, t: &str) -> std::result::Result<Value, String> {
        let t = t.trim();
        let v = match &self.kind {
            FieldKind::Number { .. } => t.parse::<f64>().ok().filter(|x| x.is_finite()).map(Value::from).unwrap_or(Value::Null),
            FieldKind::Bool { .. } => match t.to_ascii_lowercase().as_str() {
                "true" | "1" | "yes" => Value::Bool(true),
                "false" | "0" | "no" => Value::Bool(false),
                _ => Value::Null,
            },
            _ => Value::from(t),
        };
        self.coerce(&v)
    }

    /// `v` checked against the field (numbers clamped), or an error naming the field.
    pub fn coerce(&self, v: &Value) -> std::result::Result<Value, String> {
        let wrong = || format!("`{}` must be {}", self.key, self.kind_name());
        match &self.kind {
            FieldKind::Text { .. } => v.as_str().filter(|s| s.len() <= MAX_TEXT_BYTES).map(Value::from).ok_or_else(wrong),
            FieldKind::Number { min, max, .. } => v.as_f64().filter(|x| x.is_finite()).map(|x| Value::from(x.clamp(*min, *max))).ok_or_else(wrong),
            FieldKind::Bool { .. } => v.as_bool().map(Value::from).ok_or_else(wrong),
            // options given at run time: any option the plug-in could have given (it checks them)
            FieldKind::Choice { options, .. } if options.is_empty() => {
                v.as_str().filter(|s| !s.is_empty() && s.len() <= MAX_OPTION_BYTES).map(Value::from).ok_or_else(wrong)
            }
            FieldKind::Choice { options, .. } => v.as_str().filter(|s| options.iter().any(|o| o == s)).map(Value::from).ok_or_else(wrong),
        }
    }

    fn kind_name(&self) -> &'static str {
        match self.kind {
            FieldKind::Text { .. } => "text",
            FieldKind::Number { .. } => "a number",
            FieldKind::Bool { .. } => "true or false",
            FieldKind::Choice { .. } => "one of the options",
        }
    }
}

/// The dialog values for `fields` from `args`: defaults for missing keys, checked values, unknown
/// keys passed through (a command may take extra arguments from agents).
pub fn dialog_args(fields: &[Field], args: &Value) -> Result<Value> {
    let mut out = match args {
        Value::Object(m) => m.clone(),
        Value::Null => Map::new(),
        _ => return Err(Error::Params("args must be an object".into())),
    };
    for f in fields {
        let v = match out.get(&f.key) {
            Some(v) => f.coerce(v).map_err(Error::Params)?,
            None => f.default_value(),
        };
        out.insert(f.key.clone(), v);
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_a_full_manifest() {
        let m = Manifest::parse(
            json!({
                "id": "org.example.geo", "name": "Geo", "version": "1.0.0",
                "permissions": {"catalog": true, "network": ["api.example.com"]},
                "commands": [{"id": "lookup", "label": "Look Up Places", "menu": "Metadata",
                    "dialog": [{"key": "radius", "label": "Radius", "type": "number", "min": 0, "max": 10, "default": 1},
                               {"key": "lang", "label": "Language", "type": "choice", "options": ["en", "de"]}]}],
                "hooks": {"metadata": true},
                "publish": {"id": "webdav", "name": "WebDAV"}
            })
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert!(m.permissions.catalog && m.hooks.metadata && !m.hooks.export);
        let c = m.command("lookup").unwrap();
        let a = dialog_args(&c.dialog, &json!({"radius": 50})).unwrap();
        assert_eq!(a, json!({"radius": 10.0, "lang": "en"}));
        assert!(dialog_args(&c.dialog, &json!({"lang": "fr"})).is_err());
    }

    #[test]
    fn rejects_bad_manifests() {
        for bad in [
            json!({"id": "", "name": "x"}),
            json!({"id": "a b", "name": "x"}),
            json!({"id": "a", "name": " "}),
            json!({"id": "a", "name": "x", "commands": [{"id": "c", "label": "C"}, {"id": "c", "label": "D"}]}),
            json!({"id": "a", "name": "x", "permissions": {"network": ["no spaces allowed"]}}),
            json!({"id": "a", "name": "x", "commands": [{"id": "c", "label": "C", "dialog": [{"key": "n", "label": "N", "type": "number", "min": 2, "max": 1, "default": 1}]}]}),
        ] {
            assert!(Manifest::parse(bad.to_string().as_bytes()).is_err(), "{bad}");
        }
        assert!(Manifest::parse(b"not json").is_err());
    }

    #[test]
    fn destinations_and_metadata_fields() {
        let m = Manifest::parse(
            json!({"id": "a", "name": "A",
                "exportDestinations": [{"id": "srv", "name": "My Server", "dialog": [{"key": "album", "label": "Album", "type": "text"}]}],
                "metadataFields": [{"key": "job", "label": "Job", "type": "text"},
                    {"key": "score", "label": "Score", "type": "number", "min": 0, "max": 10, "default": 0, "readOnly": true}]})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(m.destination("srv").unwrap().dialog.len(), 1);
        assert!(m.metadata_field("score").unwrap().read_only);
        assert_eq!(m.metadata_field("score").unwrap().as_field().coerce(&json!(99)).unwrap(), json!(10.0));
        for bad in [
            json!({"id": "a", "name": "A", "exportDestinations": [{"id": "x", "name": "X"}, {"id": "x", "name": "Y"}]}),
            json!({"id": "a", "name": "A", "exportDestinations": [{"id": "bad id", "name": "X"}]}),
            json!({"id": "a", "name": "A", "metadataFields": [{"key": "k", "label": "K", "type": "text"}, {"key": "k", "label": "L", "type": "text"}]}),
            json!({"id": "a", "name": "A", "metadataFields": [{"key": "k k", "label": "K", "type": "text"}]}),
            json!({"id": "a", "name": "A", "metadataFields": [{"key": "c", "label": "C", "type": "choice", "options": []}]}),
        ] {
            assert!(Manifest::parse(bad.to_string().as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn settings_prepare_and_remember() {
        let m = Manifest::parse(
            json!({"id": "a", "name": "A",
                "commands": [{"id": "go", "label": "Go", "prepare": true, "dialog": [
                    {"key": "v", "label": "V", "type": "choice", "options": []},
                    {"key": "once", "label": "Once", "type": "bool", "remember": false}]}],
                "settings": [{"key": "v", "label": "V", "type": "choice", "prepare": true},
                    {"key": "n", "label": "N", "type": "number", "min": 0, "max": 1, "default": 0}]})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        let go = m.command("go").unwrap();
        assert!(go.prepare && go.dialog[0].remember && !go.dialog[1].remember);
        let v = m.setting("v").unwrap();
        assert!(v.prepare && v.options_at_run_time());
        assert_eq!(Manifest::parse(&serde_json::to_vec(&m).unwrap()).unwrap(), m, "written back as read (plugin.list)");
        // until the plug-in gives the options, any option it could give
        assert_eq!(v.coerce(&json!("4.2")).unwrap(), "4.2");
        assert!(v.coerce(&json!("")).is_err() && v.coerce(&json!("x".repeat(MAX_OPTION_BYTES + 1))).is_err());
        // what a prepare step may give
        let p = v.prepared(&json!({"options": ["1", "2"], "default": "2"})).unwrap();
        assert_eq!(p.default_value(), "2");
        assert!(p.coerce(&json!("3")).is_err(), "only what it offered");
        for bad in [json!({"options": []}), json!({"options": [""]}), json!({"options": ["1"], "default": "3"}), json!({"extra": 1}), json!("1")] {
            assert!(v.prepared(&bad).is_err(), "{bad}");
        }
        let n = m.setting("n").unwrap();
        assert!(n.prepared(&json!({"options": ["a"]})).is_err(), "options only for a choice");
        assert_eq!(n.prepared(&json!({"default": 5})).unwrap().default_value(), json!(1.0), "clamped");
        let t: Field = serde_json::from_value(json!({"key": "t", "label": "T", "type": "text"})).unwrap();
        assert_eq!(t.prepared(&json!({"default": "x".repeat(4096)})).unwrap().default_value(), json!("x".repeat(4096)));
        assert!(t.prepared(&json!({"default": "x".repeat(4097)})).is_err(), "no longer than a declared default");

        let choice = |options: Value| json!({"key": "c", "label": "C", "type": "choice", "options": options});
        for bad in [
            json!({"id": "a", "name": "A", "commands": [{"id": "go", "label": "Go", "dialog": [choice(json!([]))]}]}),
            json!({"id": "a", "name": "A", "settings": [choice(json!([]))]}),
            json!({"id": "a", "name": "A", "commands": [{"id": "go", "label": "Go", "prepare": true, "dialog": [{"key": "c", "label": "C", "type": "text", "prepare": true}]}]}),
            json!({"id": "a", "name": "A", "exportDestinations": [{"id": "d", "name": "D", "dialog": [choice(json!([]))]}]}),
            json!({"id": "a", "name": "A", "settings": [choice(json!(["x"])), choice(json!(["y"]))]}),
            json!({"id": "a", "name": "A", "settings": [{"key": "t", "label": " ", "type": "text"}]}),
            json!({"id": "a", "name": "A", "settings": (0..=MAX_FIELDS).map(|i| json!({"key": format!("k{i}"), "label": "K", "type": "bool"})).collect::<Vec<_>>()}),
        ] {
            assert!(Manifest::parse(bad.to_string().as_bytes()).is_err(), "{bad}");
        }
    }
}
