//! The provider's session import and export, over the rollout Codex keeps.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::ToString;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use coqui_acp_api::guatiao::Status;
use coqui_acp_api::guatiao::library::{Object, ProviderError};
use coqui_acp_api::{
    AccountGrant, Environment, ExportPage, HistoryLoss, ImportReport, LINE_TOO_LARGE,
    ProcessSpawner, SeedPoint, SessionExport, SessionImport,
};
use coqui_acp_history::conversation::is_uuid;
use coqui_acp_history::paging::{PageError, page};
use coqui_acp_history::{Document, Header, Origin};

use crate::accounts;
use crate::history::{
    DEFAULT_MAX_BYTES, ImportOptions, codex_home, export_file_named, import_file, rollout,
};

fn error(status: Status, why: &str) -> ProviderError {
    ProviderError::new(status, why)
}

/// The directory the session's transcript is in, after the checks every call makes: the spawner
/// runs on this machine (its files are this machine's), the id is a UUID, and the directory is
/// absolute.
fn place(
    spawner: &Object<dyn ProcessSpawner>,
    id: &str,
    working_directory: &str,
    environment: &Environment,
    account: Option<&AccountGrant>,
) -> Result<(PathBuf, PathBuf), ProviderError> {
    if !spawner.describe()?.local {
        return Err(error(
            Status::GUATIAO_ERR_UNSUPPORTED,
            "this spawner runs where this machine's files are not, and a session is read and written there",
        ));
    }
    if !is_uuid(id) {
        return Err(error(
            Status::GUATIAO_ERR_BAD_VALUE,
            "the session id is not a UUID",
        ));
    }
    let cwd = Path::new(working_directory);
    if !cwd.is_absolute() {
        return Err(error(
            Status::GUATIAO_ERR_BAD_VALUE,
            "the working directory is not absolute",
        ));
    }
    if let Some(grant) = account
        && let Some(applied) = accounts::plan(grant)?
        && !applied.dir.is_empty()
    {
        return Ok((PathBuf::from(applied.dir.clone()), cwd.to_path_buf()));
    }
    let mut variables = BTreeMap::new();
    for (name, value) in environment.iter() {
        variables.insert(OsString::from(name), OsString::from(value));
    }
    let config = codex_home(&variables, false).ok_or_else(|| {
        error(
            Status::GUATIAO_ERR_BAD_VALUE,
            "Codex's home directory is unknown",
        )
    })?;
    Ok((config, cwd.to_path_buf()))
}

pub(crate) fn import(
    session: &SessionImport,
    spawner: &Object<dyn ProcessSpawner>,
    seeds: &crate::Seeds,
) -> Result<ImportReport, ProviderError> {
    let (config, cwd) = place(
        spawner,
        &session.session_id,
        &session.working_directory,
        session.environment.expose(),
        session.account.as_ref(),
    )?;
    if rollout(&config, &session.session_id).is_ok()
        || seeds
            .lock()
            .is_ok_and(|seeds| seeds.contains_key(&session.session_id))
    {
        return Err(error(
            Status::GUATIAO_ERR_BAD_VALUE,
            "the id names a session already",
        ));
    }
    if session.history.trim().is_empty() {
        // Nothing is written: Codex gives a thread an id of its own at the first start that opens this one, and the
        // provider keeps which.
        if let Ok(mut seeds) = seeds.lock() {
            seeds.insert(session.session_id.clone(), None);
        }
        return Ok(ImportReport::new());
    }
    let doc = Document::from_slice(session.history.as_bytes()).map_err(|why| {
        // A version this crate does not read is the harness not importing it, not a malformed history.
        let status = if matches!(why, coqui_acp_history::HistoryError::UnknownVersion { .. }) {
            Status::GUATIAO_ERR_UNSUPPORTED
        } else {
            Status::GUATIAO_ERR_BAD_VALUE
        };
        error(status, &format!("the history cannot be read: {why}"))
    })?;
    let imported = import_file(
        &config,
        &cwd,
        &session.session_id,
        &doc,
        &ImportOptions::default(),
    )
    .map_err(|why| error(Status::GUATIAO_ERR_BAD_VALUE, &why.to_string()))?;
    let mut report = ImportReport::new();
    report.seeded_from = imported.seeded_from.map(|point| {
        let mut seeded = SeedPoint::line(point.line);
        seeded.digest = Some(point.digest);
        seeded
    });
    report.losses = imported
        .report
        .losses()
        .iter()
        .map(|loss| {
            let mut mapped = HistoryLoss::new(&loss.kind, loss.count);
            mapped.detail.clone_from(&loss.detail);
            mapped
        })
        .collect();
    Ok(report)
}

pub(crate) fn export(
    request: &SessionExport,
    spawner: &Object<dyn ProcessSpawner>,
    seeds: &crate::Seeds,
) -> Result<ExportPage, ProviderError> {
    let (config, _) = place(
        spawner,
        &request.session_id,
        &request.working_directory,
        request.environment.expose(),
        request.account.as_ref(),
    )?;
    let id = request.session_id.as_str();
    let seeded = seeds.lock().ok().and_then(|seeds| seeds.get(id).cloned());
    // A seeded session lives in the thread Codex gave it, when it has started one that kept something.
    let thread = match &seeded {
        Some(Some(thread)) if rollout(&config, thread).is_ok() => Some(thread.as_str()),
        Some(_) => None,
        None if rollout(&config, id).is_ok() => Some(id),
        None => {
            return Err(error(
                Status::GUATIAO_ERR_NOT_FOUND,
                "no session has this id",
            ));
        }
    };
    let doc = match thread {
        Some(thread) => export_file_named(&config, thread, id, DEFAULT_MAX_BYTES)
            .map_err(|why| error(Status::GUATIAO_ERR_BAD_VALUE, &why.to_string()))?,
        None => {
            let mut doc = Document::new(Header::new(id, Origin::new(crate::history::HARNESS)));
            doc.seal()
                .map_err(|why| error(Status::GUATIAO_ERR_BAD_VALUE, &why.to_string()))?;
            doc
        }
    };
    let text = doc
        .to_string()
        .map_err(|why| error(Status::GUATIAO_ERR_BAD_VALUE, &why.to_string()))?;
    match page(&text, request.after.as_deref(), request.max_bytes) {
        Ok(page) => Ok(ExportPage::new(page.text, page.next)),
        Err(PageError::LineTooLarge(size)) => Err(error(
            Status::GUATIAO_ERR_BAD_VALUE,
            &format!(
                "{LINE_TOO_LARGE}{size} bytes, more than max_bytes {}",
                request.max_bytes
            ),
        )),
        Err(other) => Err(error(Status::GUATIAO_ERR_BAD_VALUE, &other.to_string())),
    }
}
