//! Where a harness's programs are, for the sandboxes that must let them be read: the harness
//! and the interpreters an assistant's scripts run, found on the harness's `PATH`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::assistant::credential_dir_var;

/// The directories a sandbox opens for reading so `programs` run in it: each one's install
/// directory ([`install_dir`]) as found on the `PATH` of `environment` with links followed. A
/// directory that is or holds the home directory, `cwd` or its parent (other conversations),
/// or a credential directory ([`credential_dir_var`]), or lies inside a credential directory,
/// is left out, so a program there is unreachable; so are `/` and duplicates.
pub fn program_dirs(
    programs: &[&str],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> Vec<PathBuf> {
    let credentials: Vec<PathBuf> = environment
        .iter()
        .filter(|(name, _)| credential_dir_var(name))
        .map(|(_, path)| canonical(Path::new(path)))
        .collect();
    let mut kept: Vec<PathBuf> = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(|name| variable(environment, name).or_else(|| std::env::var(name).ok()))
        .map(|home| canonical(Path::new(&home)))
        .collect();
    kept.push(canonical(cwd));
    kept.extend(cwd.parent().map(canonical));
    kept.extend(credentials.iter().cloned());
    let mut dirs = Vec::new();
    for program in programs {
        let Some(dir) =
            find_program(program, environment).map(|found| install_dir(&canonical(&found)))
        else {
            continue;
        };
        let holds = |inner: &Path| starts_with(inner, &dir);
        if dir.parent().is_none()
            || kept.iter().any(|path| holds(path))
            || credentials.iter().any(|path| starts_with(&dir, path))
            || dirs.contains(&dir)
        {
            continue;
        }
        dirs.push(dir);
    }
    dirs
}

/// Where the program at `path` is installed: its directory, or the one above when that is
/// `bin` (`/opt/python3.12/bin/python3` is `/opt/python3.12`, beside its `lib`).
pub fn install_dir(path: &Path) -> PathBuf {
    let dir = path.parent().unwrap_or(path);
    match (dir.file_name(), dir.parent()) {
        (Some(name), Some(above))
            if name.eq_ignore_ascii_case("bin") && above.parent().is_some() =>
        {
            above.to_path_buf()
        }
        _ => dir.to_path_buf(),
    }
}

/// `name`'s value in `environment`, case ignored as Windows ignores it.
pub fn variable(environment: &BTreeMap<String, String>, name: &str) -> Option<String> {
    environment
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// `path` with links followed and without Windows' verbatim `\\?\` prefix, or as given when it
/// cannot be resolved.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .map(|resolved| PathBuf::from(path_text(&resolved)))
        .unwrap_or_else(|_| path.to_path_buf())
}

/// `path` as text, without Windows' verbatim `\\?\` prefix on a drive path.
pub fn path_text(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\")
        .filter(|rest| rest.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(&text)
        .to_owned()
}

/// Whether `path` is `base` or lies under it, case ignored on Windows.
pub fn starts_with(path: &Path, base: &Path) -> bool {
    if cfg!(windows) {
        let lower = |path: &Path| PathBuf::from(path_text(path).to_lowercase());
        lower(path).starts_with(lower(base))
    } else {
        path.starts_with(base)
    }
}

/// Where `program` is: itself when it names a path, else the first match on the `PATH` of
/// `environment` (the process's own when it has none), with Windows' executable extensions.
pub fn find_program(program: &str, environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    let given = Path::new(program);
    if given.components().count() > 1 || given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    let path = variable(environment, "PATH").or_else(|| std::env::var("PATH").ok())?;
    let extensions: Vec<String> = if cfg!(windows) && given.extension().is_none() {
        variable(environment, "PATHEXT")
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|ext| !ext.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    std::env::split_paths(&path).find_map(|dir| {
        extensions
            .iter()
            .map(|ext| dir.join(format!("{program}{ext}")))
            .find(|candidate| candidate.is_file())
    })
}

/// Where a Station on Unix may keep its configuration and secrets inside what `:minimal` or an
/// interpreter's install directory (`/usr`) re-opens.
pub const STATION_CONFIG_ROOTS: [&str; 3] = ["/etc/ai6", "/usr/local/etc", "/usr/etc"];

/// The Station's own paths a profile denies again: [`STATION_CONFIG_ROOTS`] (not on Windows),
/// AI6's state directory, `AI6_HOME`, and the directory of the program hosting this handler.
pub fn station_paths(cwd: &Path, open: &[PathBuf]) -> Vec<PathBuf> {
    denied_station_paths(
        cwd,
        open,
        crate::station::safehouse::state_dir().ok(),
        std::env::var_os("AI6_HOME").map(PathBuf::from),
        std::env::current_exe().ok(),
    )
}

/// [`station_paths`] from the given state directory, `AI6_HOME` and program. The program's
/// directory is denied unless it holds or lies in an `open` directory, which keeps it readable,
/// and then only the program itself is. A path that is or holds `cwd` or an `open` directory is
/// left out: its deny would take those with it.
pub fn denied_station_paths(
    cwd: &Path,
    open: &[PathBuf],
    state: Option<PathBuf>,
    home: Option<PathBuf>,
    program: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = if cfg!(windows) {
        Vec::new()
    } else {
        STATION_CONFIG_ROOTS.iter().map(PathBuf::from).collect()
    };
    candidates.extend(state);
    candidates.extend(home.filter(|home| !home.as_os_str().is_empty()));
    if let Some(program) = program.map(|program| canonical(&program)) {
        let overlaps = |dir: &Path| {
            open.iter()
                .any(|open| starts_with(dir, open) || starts_with(open, dir))
        };
        match program.parent() {
            Some(dir) if !overlaps(dir) => candidates.push(dir.to_path_buf()),
            _ => candidates.push(program),
        }
    }
    let cwd = canonical(cwd);
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in candidates.iter().map(|path| canonical(path)) {
        if starts_with(&cwd, &path)
            || open.iter().any(|dir| starts_with(dir, &path))
            || paths.contains(&path)
        {
            continue;
        }
        paths.push(path);
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_in_bin_is_installed_in_the_directory_above() {
        assert_eq!(
            install_dir(Path::new("/opt/python3.12/bin/python3")),
            Path::new("/opt/python3.12")
        );
        assert_eq!(
            install_dir(Path::new("/usr/bin/python3")),
            Path::new("/usr")
        );
        assert_eq!(install_dir(Path::new("/bin/sh")), Path::new("/bin"));
        assert_eq!(
            install_dir(Path::new("/opt/tools/python3")),
            Path::new("/opt/tools")
        );
    }
}
