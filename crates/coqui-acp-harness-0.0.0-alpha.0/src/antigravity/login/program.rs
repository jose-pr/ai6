//! Antigravity's OAuth clients, read from its installed program: the Google client ids (`<number>-<32 characters>.apps.googleusercontent.com`)
//! and client secrets (`GOCSPX-` and 28 characters) among its strings. The program does not say which secret is whose, so each
//! id is paired with each secret and the token endpoint says which pair is right.

use alloc::string::String;
use alloc::vec::Vec;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::OAuthClient;

const ID_SUFFIX: &[u8] = b".apps.googleusercontent.com";
const SECRET_PREFIX: &[u8] = b"GOCSPX-";
/// What follows a client id's number and dash.
const ID_HASH: usize = 32;
/// What follows `GOCSPX-` in a client secret.
const SECRET_REST: usize = 28;
/// How much of one chunk is read again with the next, so a string across the seam is found whole.
const OVERLAP: usize = 256;
const CHUNK: usize = 4 * 1024 * 1024;

/// Which file was read, so a program replaced by an update is read again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FileKey {
    path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

impl FileKey {
    pub(super) fn of(path: &Path) -> io::Result<Self> {
        let meta = std::fs::metadata(path)?;
        Ok(Self {
            path: path.to_owned(),
            len: meta.len(),
            modified: meta.modified().ok(),
        })
    }
}

/// agy's program: `agy` on this process's `PATH`, else on Windows where its installer puts it.
pub(super) fn find() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let pathext = std::env::var("PATHEXT").unwrap_or_default();
    if let Some(found) = crate::process::find_in(&path, &[], &pathext, "agy") {
        return Some(found);
    }
    if cfg!(windows) {
        let local = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty())?;
        let installed = PathBuf::from(local).join("agy").join("bin").join("agy.exe");
        return installed.is_file().then_some(installed);
    }
    None
}

/// Every pairing of the client ids and secrets in the file at `path`, ids and secrets each in the order they first appear.
pub(super) fn clients(path: &Path) -> io::Result<Vec<OAuthClient>> {
    let (ids, secrets) = strings(File::open(path)?)?;
    Ok(ids
        .iter()
        .flat_map(|id| {
            secrets
                .iter()
                .map(move |secret| OAuthClient::new(id.clone(), secret.clone()))
        })
        .collect())
}

/// The client ids and secrets `reader` holds, each once.
fn strings(mut reader: impl Read) -> io::Result<(Vec<String>, Vec<String>)> {
    let (mut ids, mut secrets) = (Vec::new(), Vec::new());
    let id_finder = memchr::memmem::Finder::new(ID_SUFFIX);
    let secret_finder = memchr::memmem::Finder::new(SECRET_PREFIX);
    let mut buffer: Vec<u8> = Vec::with_capacity(CHUNK + OVERLAP);
    // Where the bytes not yet searched begin: a string that ends before it was found in the chunk before.
    let mut fresh = 0;
    let mut last = false;
    while !last {
        let start = buffer.len();
        buffer.resize(start + CHUNK, 0);
        let mut filled = start;
        while filled < buffer.len() {
            match reader.read(&mut buffer[filled..]) {
                Ok(0) => {
                    last = true;
                    break;
                }
                Ok(read) => filled += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        buffer.truncate(filled);

        for at in id_finder.find_iter(&buffer) {
            if at + ID_SUFFIX.len() > fresh
                && let Some(id) = id_at(&buffer, at)
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
        for at in secret_finder.find_iter(&buffer) {
            let end = at + SECRET_PREFIX.len() + SECRET_REST;
            if end > fresh
                && let Some(secret) = buffer.get(at..end)
                && secret[SECRET_PREFIX.len()..]
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                let secret = String::from_utf8_lossy(secret).into_owned();
                if !secrets.contains(&secret) {
                    secrets.push(secret);
                }
            }
        }

        let keep = buffer.len().min(OVERLAP);
        buffer.drain(..buffer.len() - keep);
        // A string that ends within what is kept was found in this chunk; one cut off at its end is found whole in the next.
        fresh = keep;
    }
    Ok((ids, secrets))
}

/// The client id whose `.apps.googleusercontent.com` begins at `suffix`: a number, a dash and 32 lower-case letters and digits.
fn id_at(bytes: &[u8], suffix: usize) -> Option<String> {
    let hash_start = suffix.checked_sub(ID_HASH)?;
    let hash = &bytes[hash_start..suffix];
    if !hash
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return None;
    }
    let dash = hash_start.checked_sub(1)?;
    if bytes[dash] != b'-' {
        return None;
    }
    let digits = bytes[..dash]
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return None;
    }
    let id = &bytes[dash - digits..suffix + ID_SUFFIX.len()];
    Some(String::from_utf8_lossy(id).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client id of `number` and a secret of `letter`, as a program's strings hold them, run together.
    fn id(number: &str, letter: char) -> String {
        alloc::format!(
            "{number}-{}{}",
            letter.to_string().repeat(ID_HASH),
            core::str::from_utf8(ID_SUFFIX).unwrap()
        )
    }

    fn secret(letter: char) -> String {
        alloc::format!(
            "{}{}",
            core::str::from_utf8(SECRET_PREFIX).unwrap(),
            letter.to_string().repeat(SECRET_REST)
        )
    }

    /// A reader that hands out at most `step` bytes a call, so a string falls across the seams of chunks.
    struct Trickle<'a>(&'a [u8], usize);

    impl Read for Trickle<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = self.0.len().min(self.1).min(out.len());
            out[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    #[test]
    fn ids_and_secrets_are_found_where_they_run_into_other_strings() {
        let text = alloc::format!(
            "max){}https://x{}{}r{}{}x",
            id("123456789012", 'a'),
            secret('B'),
            secret('C'),
            id("98765", 'b'),
            secret('B'),
        );
        let (ids, secrets) = strings(text.as_bytes()).unwrap();
        assert_eq!(ids, [id("123456789012", 'a'), id("98765", 'b')]);
        assert_eq!(secrets, [secret('B'), secret('C')]);
    }

    #[test]
    fn a_string_across_a_chunk_seam_is_found_once() {
        for offset in [CHUNK - 40, CHUNK - 20, CHUNK - 3, CHUNK + 1] {
            let mut bytes = alloc::vec![b'.'; offset];
            bytes.extend(id("42", 'c').as_bytes());
            bytes.extend(secret('D').as_bytes());
            bytes.extend([b'.'; 100]);
            let (ids, secrets) = strings(Trickle(&bytes, 1 << 20)).unwrap();
            assert_eq!(ids, [id("42", 'c')], "at {offset}");
            assert_eq!(secrets, [secret('D')], "at {offset}");
        }
    }

    #[test]
    fn a_suffix_without_a_whole_id_is_not_one() {
        let text = alloc::format!("-short{}", core::str::from_utf8(ID_SUFFIX).unwrap());
        assert!(strings(text.as_bytes()).unwrap().0.is_empty());
    }
}
