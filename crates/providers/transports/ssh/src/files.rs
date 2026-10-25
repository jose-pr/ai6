//! Files over SFTP.

use std::io::SeekFrom;

use ai6::guatiao::library::ProviderError;
use ai6::guatiao::{Bytes, Status};
use ai6::{DirEntry, DirListing, FileData, ReadSpec, WriteSpec};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileType, OpenFlags, StatusCode};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::refuse;
use crate::session::{Conn, Link};

/// Runs `operation` on an SFTP session, opening a new session once if the cached one died.
async fn with_sftp<T, F>(link: &Link, path: &str, operation: F) -> Result<T, ProviderError>
where
    F: AsyncFn(&SftpSession) -> Result<T, SftpError>,
{
    let mut attempt = 0;
    loop {
        let conn = link.conn().await?;
        let sftp = match conn.sftp().await {
            Ok(sftp) => sftp,
            Err(error) if attempt == 0 && !ai6::is_unsupported(&error) => {
                link.reset(&conn).await;
                attempt += 1;
                continue;
            }
            Err(error) => return Err(error),
        };
        match operation(&sftp).await {
            Ok(value) => return Ok(value),
            Err(error) if attempt == 0 && is_connection_failure(&error) => {
                retire(link, &conn).await;
                attempt += 1;
            }
            Err(error) => return Err(map_error(&error, path)),
        }
    }
}

async fn retire(link: &Link, conn: &std::sync::Arc<Conn>) {
    conn.drop_sftp().await;
    if conn.is_closed() {
        link.reset(conn).await;
    }
}

/// Whether the SFTP session, rather than the request, failed.
fn is_connection_failure(error: &SftpError) -> bool {
    match error {
        SftpError::Status(status) => matches!(
            status.status_code,
            StatusCode::NoConnection | StatusCode::ConnectionLost
        ),
        SftpError::IO(_) | SftpError::UnexpectedBehavior(_) | SftpError::Timeout => true,
        _ => false,
    }
}

fn map_error(error: &SftpError, path: &str) -> ProviderError {
    let status = match error {
        SftpError::Status(status) if status.status_code == StatusCode::NoSuchFile => {
            Status::GUATIAO_ERR_NOT_FOUND
        }
        _ => Status::GUATIAO_ERR_INTERNAL,
    };
    refuse(status, format!("{path}: {error}"))
}

pub(crate) async fn read(link: &Link, spec: &ReadSpec) -> Result<FileData, ProviderError> {
    with_sftp(link, &spec.path, async |sftp| {
        let mut file = sftp.open(spec.path.clone()).await?;
        let size = file.metadata().await?.len();
        file.seek(SeekFrom::Start(spec.offset)).await?;
        let mut data = Vec::new();
        (&mut file).take(spec.limit).read_to_end(&mut data).await?;
        file.shutdown().await?;
        Ok(FileData {
            data: Bytes(data),
            size,
        })
    })
    .await
}

pub(crate) async fn write(link: &Link, spec: &WriteSpec) -> Result<(), ProviderError> {
    with_sftp(link, &spec.path, async |sftp| {
        if spec.create_dirs {
            make_parents(sftp, &spec.path).await;
        }
        let mode = if spec.append {
            OpenFlags::APPEND
        } else {
            OpenFlags::TRUNCATE
        };
        let mut file = sftp
            .open_with_flags(
                spec.path.clone(),
                OpenFlags::WRITE | OpenFlags::CREATE | mode,
            )
            .await?;
        file.write_all(&spec.data.0).await?;
        file.shutdown().await?;
        Ok(())
    })
    .await
}

/// Creates every missing directory above `path`. A directory that already exists, or one
/// that cannot be made, surfaces when the file itself is opened.
async fn make_parents(sftp: &SftpSession, path: &str) {
    let Some((parent, _)) = path.rsplit_once('/') else {
        return;
    };
    let mut so_far = String::new();
    for part in parent.split('/') {
        if so_far.is_empty() && part.is_empty() {
            so_far.push('/');
            continue;
        }
        if !so_far.is_empty() && !so_far.ends_with('/') {
            so_far.push('/');
        }
        so_far.push_str(part);
        if !part.is_empty() {
            let _ = sftp.create_dir(so_far.clone()).await;
        }
    }
}

pub(crate) async fn list(link: &Link, path: &str) -> Result<DirListing, ProviderError> {
    with_sftp(link, path, async |sftp| {
        let mut entries: Vec<DirEntry> = sftp
            .read_dir(path.to_owned())
            .await?
            .map(|entry| {
                let (kind, size) = match entry.file_type() {
                    FileType::Dir => ("dir", 0),
                    FileType::File => ("file", entry.metadata().len()),
                    FileType::Symlink => ("link", 0),
                    FileType::Other => ("other", 0),
                };
                DirEntry {
                    name: entry.file_name(),
                    kind: kind.to_owned(),
                    size,
                }
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(DirListing { entries })
    })
    .await
}
