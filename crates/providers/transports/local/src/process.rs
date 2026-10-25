use ai6::directorates::q::transport::kit::{ProcessInput, run_process};
use ai6::guatiao::Status;
use ai6::guatiao::library::ProviderError;
use ai6::{Output, RunSpec};

use crate::refuse;

pub(crate) fn run(spec: &RunSpec) -> Result<Output, ProviderError> {
    let (program, args) = spec
        .argv
        .split_first()
        .ok_or_else(|| refuse(Status::GUATIAO_ERR_BAD_VALUE, "argv is empty"))?;
    let mut command = std::process::Command::new(program);
    command.args(args);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = &spec.env {
        command.envs(env);
    }
    run_process(
        &mut command,
        &ProcessInput {
            stdin: spec.stdin.as_ref().map(|stdin| stdin.0.as_slice()),
            timeout_ms: spec.timeout_ms,
            max_output: spec.max_output,
        },
    )
}
