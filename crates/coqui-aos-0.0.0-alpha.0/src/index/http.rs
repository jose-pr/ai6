//! A client for an OpenAI-compatible `/v1/embeddings` endpoint, such as a local model server's.

use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;

use serde_json::{Value, json};

use super::{Embedder, Error};

/// An OpenAI-compatible embeddings endpoint, reached over HTTP.
#[derive(Clone, Debug)]
pub struct HttpEmbedder {
    url: String,
    model: String,
    timeout: Duration,
    bearer: Option<String>,
}

impl HttpEmbedder {
    /// The endpoint of the server at `base` (`http://127.0.0.1:60570`, with or without a trailing `/v1`) for `model`, the
    /// exact id the server loaded.
    #[must_use]
    pub fn new(base: &str, model: impl Into<String>) -> Self {
        let base = base.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            alloc::format!("{base}/embeddings")
        } else {
            alloc::format!("{base}/v1/embeddings")
        };
        Self {
            url,
            model: model.into(),
            timeout: Duration::from_secs(120),
            bearer: None,
        }
    }

    /// Sends `token` as the request's bearer, for an endpoint that asks for a key.
    #[must_use]
    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        self.bearer = Some(token.into());
        self
    }

    /// How long one request may take, 120 seconds by default.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Embedder for HttpEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .max_redirects(0)
            .timeout_global(Some(self.timeout))
            .build()
            .into();
        let body = json!({"model": self.model, "input": inputs}).to_string();
        let mut request = agent
            .post(&self.url)
            .header("content-type", "application/json");
        if let Some(token) = &self.bearer {
            request = request.header("authorization", alloc::format!("Bearer {token}"));
        }
        let response = request
            .send(body.as_str())
            .map_err(|error| Error::Embed(alloc::format!("{}: {error}", self.url)))?;
        let text = response
            .into_body()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_string()
            .map_err(|error| Error::Embed(alloc::format!("{}: {error}", self.url)))?;
        parse(&text, inputs.len())
    }
}

/// The vectors an embeddings response holds, ordered by their `index`.
fn parse(text: &str, expected: usize) -> Result<Vec<Vec<f32>>, Error> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| Error::Embed(alloc::format!("the response is not JSON: {error}")))?;
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Embed("the response has no `data` list".into()))?;
    if data.len() != expected {
        return Err(Error::Embed(alloc::format!(
            "{} vectors for {expected} inputs",
            data.len()
        )));
    }
    let mut slots: Vec<Option<Vec<f32>>> = alloc::vec![None; expected];
    for (position, item) in data.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .map_or(position, |index| {
                usize::try_from(index).unwrap_or(usize::MAX)
            });
        let vector = item
            .get("embedding")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Embed("an item has no `embedding` list".into()))?
            .iter()
            .map(|number| {
                #[allow(clippy::cast_possible_truncation)]
                number.as_f64().map(|number| number as f32)
            })
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| {
                Error::Embed("an embedding holds a value that is not a number".into())
            })?;
        let slot = slots.get_mut(index).ok_or_else(|| {
            Error::Embed(alloc::format!("an item's index {index} is out of range"))
        })?;
        if slot.replace(vector).is_some() {
            return Err(Error::Embed(alloc::format!("two items have index {index}")));
        }
    }
    Ok(slots.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_takes_a_base_with_or_without_v1() {
        assert_eq!(
            HttpEmbedder::new("http://127.0.0.1:1/", "m").url,
            "http://127.0.0.1:1/v1/embeddings"
        );
        assert_eq!(
            HttpEmbedder::new("http://127.0.0.1:1/v1", "m").url,
            "http://127.0.0.1:1/v1/embeddings"
        );
    }

    #[test]
    fn a_response_is_ordered_by_index_and_checked() {
        let text = r#"{"data":[{"index":1,"embedding":[3,4]},{"index":0,"embedding":[1,2]}]}"#;
        assert_eq!(
            parse(text, 2).unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
        assert!(parse(text, 3).is_err());
        assert!(
            parse(
                r#"{"data":[{"index":0,"embedding":[1]},{"index":0,"embedding":[2]}]}"#,
                2
            )
            .is_err()
        );
        assert!(parse("{}", 1).is_err());
    }
}
