//! The embedding provider behind the search by meaning: a trait. `HttpEmbedder`, behind the `embeddings` feature, is a
//! client for an OpenAI-compatible `/v1/embeddings` endpoint. Nothing here fetches a model; the person's provider holds it.

use super::Error;
use alloc::string::String;
use alloc::vec::Vec;

/// Something that turns text into vectors.
pub trait Embedder {
    /// The exact model the vectors come from, saved with an index so a later run cannot mix models.
    fn model(&self) -> &str;

    /// One vector per input, in order, all of one length.
    ///
    /// # Errors
    ///
    /// [`Error::Embed`] when the provider cannot answer, or answers with something else than one vector per input.
    fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, Error>;
}
