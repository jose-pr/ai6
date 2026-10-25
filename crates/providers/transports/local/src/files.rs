use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use ai6::guatiao::Bytes;
use ai6::guatiao::library::ProviderError;
use ai6::{DirEntry, DirListing, FileData, ReadSpec, WriteSpec};

use ai6::directorates::q::transport::kit::io_error;

pub(crate) fn read(spec: &ReadSpec) -> Result<FileData, ProviderError> {
    let fail = |error: std::io::Error| io_error(&error, &spec.path);
    let mut file = File::open(&spec.path).map_err(fail)?;
    let size = file.metadata().map_err(fail)?.len();
    file.seek(SeekFrom::Start(spec.offset)).map_err(fail)?;
    let mut data = Vec::new();
    file.take(spec.limit).read_to_end(&mut data).map_err(fail)?;
    Ok(FileData {
        data: Bytes(data),
        size,
    })
}

pub(crate) fn write(spec: &WriteSpec) -> Result<(), ProviderError> {
    let fail = |error: std::io::Error| io_error(&error, &spec.path);
    let path = Path::new(&spec.path);
    if spec.create_dirs
        && let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(fail)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    if spec.append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    let mut file = options.open(path).map_err(fail)?;
    file.write_all(&spec.data.0).map_err(fail)
}

pub(crate) fn list(path: &str) -> Result<DirListing, ProviderError> {
    let fail = |error: std::io::Error| io_error(&error, path);
    let mut entries = Vec::new();
    for entry in fs::read_dir(path).map_err(fail)? {
        let entry = entry.map_err(fail)?;
        let kind = entry.file_type().map_err(fail)?;
        let (kind, size) = if kind.is_symlink() {
            ("link", 0)
        } else if kind.is_dir() {
            ("dir", 0)
        } else if kind.is_file() {
            ("file", entry.metadata().map_err(fail)?.len())
        } else {
            ("other", 0)
        };
        entries.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            kind: kind.to_owned(),
            size,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(DirListing { entries })
}
