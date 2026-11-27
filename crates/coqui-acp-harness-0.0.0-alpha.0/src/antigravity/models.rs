//! The models `agy models` lists, with nothing started: the same ids and names its ACP session offers.

use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;

use coqui_acp_api::{ModelInfo, ModelListing, UnknownKind};

use super::AcpModel;
use crate::cli;
use crate::process::{Launch, Spawn};

/// The arguments `agy` is run with to list its models.
pub const MODELS_ARGS: &[&str] = &["models"];

/// The most of the listing that is read: a real one is under 2 KB.
const LISTING_BYTES: usize = 1 << 16;

/// The models `text` lists: one for each line of an id, a tab and a name. Any other line (the `Fetching available models...`
/// that comes first) is left out.
#[must_use]
pub fn parse_models(text: &str) -> Vec<AcpModel> {
    text.lines()
        .filter_map(|line| {
            let (id, name) = line.trim().split_once('\t')?;
            let (id, name) = (id.trim(), name.trim());
            (!id.is_empty() && !id.contains(char::is_whitespace) && !name.is_empty())
                .then(|| AcpModel::new(id, name))
        })
        .collect()
}

/// The models `program models` lists, run through `spawn` and ended after `limit`; known when it lists any.
#[must_use]
pub fn list_models(spawn: &dyn Spawn, program: &str, limit: Duration) -> ModelListing {
    let launch = Launch::new(program).args(MODELS_ARGS.iter().copied());
    let done = match cli::run(spawn, &launch, None, limit, LISTING_BYTES) {
        Ok(done) => done,
        Err(error) => {
            return ModelListing::unavailable(
                if error.kind() == std::io::ErrorKind::NotFound {
                    UnknownKind::NotInstalled
                } else {
                    UnknownKind::Failed
                },
                alloc::format!("Antigravity could not be asked: {error}"),
            );
        }
    };
    let models = parse_models(&String::from_utf8_lossy(&done.stdout));
    if !done.succeeded() || models.is_empty() {
        return ModelListing::unavailable(
            UnknownKind::Failed,
            if done.timed_out {
                "Antigravity did not list its models in time"
            } else {
                "Antigravity listed no model"
            },
        );
    }
    ModelListing::of(
        models
            .into_iter()
            .map(|model| {
                let mut info = ModelInfo::new(model.id);
                info.name = Some(model.name);
                info
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_is_one_model_for_each_id_and_name() {
        let text = include_str!("../../tests/fixtures/agy-1.5.15/models.txt");
        let models = parse_models(text);
        assert_eq!(models.len(), 5);
        assert_eq!(
            models[0],
            AcpModel::new("gemini-4-neon-high", "Gemini 4 Neon (High)")
        );
        assert_eq!(models[3].id, "gemini-4-argon-medium");
        assert_eq!(models[4].name, "Claude Opus 5.5 (High)");
    }

    #[test]
    fn a_line_that_is_not_an_id_and_a_name_is_left_out() {
        assert!(parse_models("Fetching available models...\n\nnot a model\tx y\n").is_empty());
    }
}
