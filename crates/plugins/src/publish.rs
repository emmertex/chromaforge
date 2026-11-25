//! The publish-service boundary.
//!
//! Built-in publish services (the `publish` crate) and plug-in services implement the same shape.
//! [`PublishProvider`] is that shape as seen from here; the publish crate wires a
//! [`PluginPublisher`] into its own service trait with a thin adapter, so this crate doesn't depend
//! on it (and stays below it in the layer stack).
//!
//! Requests the plug-in receives:
//! - `{"hook": "publish.upload", "service", "item": {path, photo, title, caption, keywords, remoteId?}}`
//!   → `{remoteId, url?}`; `item.path` (the rendered file) is lent to the call for reading.
//! - `{"hook": "publish.delete", "service", "remoteId"}` → anything.
//! - `{"hook": "publish.comments", "service", "remoteId"}` → `{comments: [{author, text, date?}]}`,
//!   only for a service declared with `comments: true` (host feature `publishServices`).
//!
//! With a [`PublishTarget`] set ([`PluginPublisher::with_target`]), every request also carries
//! `account: {id, name, settings}` (the user's service: one plug-in service can be set up several
//! times, for several accounts) and `collection: {album, name, folder}` (the published collection,
//! so a service can keep one remote album per collection).
//!
//! Replies are untrusted: a remote id must be 1–1024 bytes without control characters, and
//! comments are capped ([`MAX_COMMENTS`], [`MAX_COMMENT_BYTES`]).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::context::HostApi;
use crate::manager::Manager;
use crate::{Error, Result};

/// The most comments kept from one `publish.comments` reply.
pub const MAX_COMMENTS: usize = 1000;
/// Longest comment text (and author) kept, in bytes; longer ones are cut at a character boundary.
pub const MAX_COMMENT_BYTES: usize = 8 * 1024;
/// Longest remote id accepted from a plug-in.
pub const MAX_REMOTE_ID_BYTES: usize = 1024;

/// One rendered photo to publish.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishItem {
    /// The rendered file.
    pub path: String,
    /// Catalog photo id.
    pub photo: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub caption: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Set when re-publishing a photo the service already has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
}

/// Where the service put it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishedRef {
    pub remote_id: String,
    #[serde(default)]
    pub url: Option<String>,
}

/// The user's publish service and the collection a call is about.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishTarget {
    /// The user's service: `{id, name, settings}` (`svc-1`, "My Gallery", the service's settings).
    pub account: Value,
    /// The published collection: `{album, name, folder}`.
    pub collection: Value,
}

/// A remote comment on a published photo, as a plug-in reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteComment {
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub text: String,
    /// ISO 8601, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// A publish service.
pub trait PublishProvider {
    fn service_id(&self) -> &str;
    fn label(&self) -> &str;
    fn upload(&mut self, host: &mut dyn HostApi, item: &PublishItem) -> Result<PublishedRef>;
    fn delete(&mut self, host: &mut dyn HostApi, remote_id: &str) -> Result<()>;
    /// Whether the service has remote comments.
    fn has_comments(&self) -> bool {
        false
    }
    /// Remote comments on a published photo (none for services without them).
    fn comments(&mut self, _host: &mut dyn HostApi, _remote_id: &str) -> Result<Vec<RemoteComment>> {
        Ok(Vec::new())
    }
}

/// `s` cut to at most `max` bytes at a character boundary, control characters but newlines dropped.
fn clean(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars().filter(|c| !c.is_control() || *c == '\n') {
        if out.len().saturating_add(c.len_utf8()) > max {
            break;
        }
        out.push(c);
    }
    out
}

/// A `publish.comments` reply, checked: `{comments: [...]}` (or the bare array), capped.
pub fn parse_comments(v: &Value) -> Result<Vec<RemoteComment>> {
    let list = v.get("comments").unwrap_or(v);
    let list = list.as_array().ok_or_else(|| Error::Failed("publish.comments reply: expected {comments: [{author, text, date?}]}".into()))?;
    Ok(list
        .iter()
        .take(MAX_COMMENTS)
        .filter_map(|c| {
            let text = clean(c.get("text").and_then(Value::as_str)?, MAX_COMMENT_BYTES);
            let author = clean(c.get("author").and_then(Value::as_str).unwrap_or_default(), MAX_COMMENT_BYTES);
            let date = c.get("date").and_then(Value::as_str).map(|d| clean(d, 64)).filter(|d| !d.is_empty());
            (!text.trim().is_empty()).then_some(RemoteComment { author, text, date })
        })
        .collect())
}

/// A `publish.upload` reply, checked.
fn parse_published(v: Value) -> Result<PublishedRef> {
    let r: PublishedRef = serde_json::from_value(v).map_err(|e| Error::Failed(format!("publish.upload reply: {e}")))?;
    if r.remote_id.is_empty() || r.remote_id.len() > MAX_REMOTE_ID_BYTES || r.remote_id.chars().any(char::is_control) {
        return Err(Error::Failed(format!("publish.upload reply: `remoteId` must be 1-{MAX_REMOTE_ID_BYTES} bytes without control characters")));
    }
    Ok(r)
}

/// A plug-in's publish service.
pub struct PluginPublisher<'m> {
    manager: &'m mut Manager,
    plugin: String,
    service: crate::PublishDecl,
    target: Option<PublishTarget>,
}

impl<'m> PluginPublisher<'m> {
    /// The service of plug-in `plugin`, if it is installed, enabled and provides one.
    pub fn new(manager: &'m mut Manager, plugin: &str) -> Result<PluginPublisher<'m>> {
        let p = manager.get(plugin).ok_or_else(|| Error::NotFound(plugin.into()))?;
        if !p.enabled {
            return Err(Error::Disabled(plugin.into()));
        }
        let service = p.plugin.manifest().publish.clone().ok_or_else(|| Error::Params(format!("plug-in {plugin} provides no publish service")))?;
        Ok(PluginPublisher { manager, plugin: plugin.into(), service, target: None })
    }

    /// Every request also names the user's service and the collection ([`PublishTarget`]).
    pub fn with_target(mut self, target: PublishTarget) -> Self {
        self.target = Some(target);
        self
    }

    fn request(&self, hook: &str) -> Value {
        let mut req = json!({"hook": hook, "service": self.service.id});
        if let Some(t) = &self.target {
            req["account"] = t.account.clone();
            req["collection"] = t.collection.clone();
        }
        req
    }
}

impl PublishProvider for PluginPublisher<'_> {
    fn service_id(&self) -> &str {
        &self.service.id
    }
    fn label(&self) -> &str {
        &self.service.name
    }
    fn upload(&mut self, host: &mut dyn HostApi, item: &PublishItem) -> Result<PublishedRef> {
        let mut req = self.request("publish.upload");
        req["item"] = json!(item);
        let v = self.manager.invoke(&self.plugin, &req, host, &[PathBuf::from(&item.path)])?;
        parse_published(v)
    }
    fn delete(&mut self, host: &mut dyn HostApi, remote_id: &str) -> Result<()> {
        let mut req = self.request("publish.delete");
        req["remoteId"] = json!(remote_id);
        self.manager.invoke(&self.plugin, &req, host, &[]).map(|_: Value| ())
    }
    fn has_comments(&self) -> bool {
        self.service.comments
    }
    fn comments(&mut self, host: &mut dyn HostApi, remote_id: &str) -> Result<Vec<RemoteComment>> {
        if !self.service.comments {
            return Ok(Vec::new());
        }
        let mut req = self.request("publish.comments");
        req["remoteId"] = json!(remote_id);
        let v = self.manager.invoke(&self.plugin, &req, host, &[])?;
        parse_comments(&v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_comment_replies_are_checked_and_capped() {
        assert!(parse_comments(&json!("no")).is_err());
        assert!(parse_comments(&json!({"comments": 5})).is_err());
        let many: Vec<Value> = (0..MAX_COMMENTS + 50).map(|i| json!({"author": "a", "text": format!("c{i}")})).collect();
        assert_eq!(parse_comments(&json!({"comments": many})).unwrap().len(), MAX_COMMENTS);
        let long = "é".repeat(MAX_COMMENT_BYTES);
        let c =
            parse_comments(&json!([{"author": long, "text": format!("x\u{0}y{long}"), "date": 7}, {"text": "  "}, {"author": "no text"}])).unwrap();
        assert_eq!(c.len(), 1, "empty and missing texts are dropped");
        assert!(c[0].text.len() <= MAX_COMMENT_BYTES && c[0].author.len() <= MAX_COMMENT_BYTES);
        assert!(c[0].text.starts_with("xy"), "control characters dropped");
        assert_eq!(c[0].date, None, "a date that isn't text is dropped");
    }

    #[test]
    fn hostile_remote_ids_are_refused() {
        assert!(parse_published(json!({"remoteId": "r1"})).is_ok());
        for bad in [
            json!({"remoteId": ""}),
            json!({"remoteId": "a\u{0}b"}),
            json!({"remoteId": "x".repeat(MAX_REMOTE_ID_BYTES + 1)}),
            json!({"remoteId": 3}),
            json!(null),
        ] {
            assert!(parse_published(bad.clone()).is_err(), "{bad}");
        }
    }
}
