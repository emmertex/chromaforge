//! A build without ONNX Runtime (no `ort` feature, the web or Android): everything is unavailable.

use crate::{Error, Provider, Runtime, Tensor, Want};

fn why() -> Error {
    Error::Unavailable("this build does not include ONNX Runtime".into())
}

pub(crate) fn runtime() -> Result<&'static Runtime, Error> {
    Err(why())
}

pub(crate) fn peek() -> Option<Result<&'static Runtime, Error>> {
    Some(Err(why()))
}

pub(crate) struct Session;

type LoadedSession = (Session, Provider, Vec<String>, Vec<String>);

pub(crate) fn load(_: &Runtime, _: &[u8], _: Want, _: &[usize]) -> Result<LoadedSession, Error> {
    Err(why())
}

pub(crate) fn run(_: &Session, _: &[String], _: &[Tensor]) -> Result<Vec<Tensor>, Error> {
    Err(why())
}

#[cfg(test)]
pub(crate) fn parse_version(_: &str) -> Option<String> {
    None
}
