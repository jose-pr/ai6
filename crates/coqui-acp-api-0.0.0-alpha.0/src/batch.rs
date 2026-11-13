//! Starting a program in a folder on a share (`\\server\share\work`).
//!
//! `cmd.exe`, which Windows starts for a `.cmd` or `.bat` file, takes no share as its working directory and falls back to the
//! Windows folder, so a program that is such a file, npm's shim for `codex` for one, is not started in the folder it was
//! asked to be started in. [`start_in`] says what to start instead: for a file npm wrote, the program the file names. Any
//! other batch file in such a folder is refused with the reason, since running it elsewhere would be a quiet mistake.

use alloc::borrow::ToOwned as _;
use alloc::format;
use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;

/// What to start for `program args` to run in `cwd`: as asked, unless this is Windows, `program` is a `.cmd` or `.bat` file
/// and `cwd` is on a share. Then it is the program an npm-written file names (its `node.exe` beside it, or `node`, and the
/// script, or the executable it calls) with that file's own arguments first, and `args` after them.
///
/// # Errors
///
/// `program` is a batch file, `cwd` is on a share, and the file cannot be read as an npm shim.
pub fn start_in(
    program: &OsStr,
    args: &[OsString],
    cwd: &Path,
) -> io::Result<(OsString, Vec<OsString>)> {
    let asked = || (program.to_owned(), args.to_vec());
    let batch = Path::new(program)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"));
    if !(cfg!(windows) && batch && on_share(cwd)) {
        return Ok(asked());
    }
    let shim = std::fs::read_to_string(program)
        .ok()
        .and_then(|text| npm_shim(&text, Path::new(program)));
    let Some((start, mut all)) = shim else {
        return Err(io::Error::other(format!(
            "{} is a batch file, which cmd.exe cannot start in a folder on a share",
            Path::new(program).display()
        )));
    };
    all.extend(args.iter().cloned());
    Ok((start, all))
}

/// Whether `path` is written `\\server\share\…` (or with `/`): a share, not a device namespace.
#[must_use]
pub fn on_share(path: &Path) -> bool {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    let separator = |byte: u8| matches!(byte, b'\\' | b'/');
    bytes.len() > 3
        && separator(bytes[0])
        && separator(bytes[1])
        && !matches!(bytes[2], b'.' | b'?')
        && !separator(bytes[2])
}

/// The program and first arguments of the `.cmd` file `text` that npm wrote at `file`: its last command, which names its
/// program (`%_prog%`: a `node.exe` beside the file, else `node`) or a path under the file's folder (`%dp0%`), and its
/// arguments, ended by `%*`. `None` for any other file.
fn npm_shim(text: &str, file: &Path) -> Option<(OsString, Vec<OsString>)> {
    let folder = file.parent()?.to_string_lossy().into_owned();
    let line = text.lines().rev().find(|line| line.contains("%*"))?;
    let command = line.rsplit_once("& ").map_or(line, |(_, command)| command);
    let mut words: Vec<&str> = Vec::new();
    let mut rest = command.trim();
    while !rest.is_empty() {
        let (word, after) = if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"')?;
            (&quoted[..end], &quoted[end + 1..])
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        words.push(word);
        rest = after.trim_start();
    }
    if words.pop()? != "%*" {
        return None;
    }
    let first = *words.first()?;
    if !(first == "%_prog%" || first.starts_with("%dp0%") || first.starts_with("%~dp0")) {
        return None;
    }
    let mut made: Vec<OsString> = Vec::new();
    for word in words {
        let word: String = if word == "%_prog%" {
            let own = Path::new(&folder).join("node.exe");
            if own.is_file() {
                own.to_string_lossy().into_owned()
            } else {
                "node".to_owned()
            }
        } else {
            word.replace("%dp0%", &folder).replace("%~dp0", &folder)
        };
        if word.contains('%') {
            return None;
        }
        made.push(OsString::from(word.to_string()));
    }
    let mut made = made.into_iter();
    let program = made.next()?;
    Some((program, made.collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const SHIM: &str = concat!(
        "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\n",
        "IF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n",
        "  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\n",
        "endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@scope\\tool\\bin\\tool.js\" %*\r\n"
    );

    #[cfg(windows)]
    #[test]
    fn an_npm_command_file_names_its_program_and_script() {
        let (program, args) = npm_shim(SHIM, Path::new(r"C:\npm\tool.cmd")).expect("a shim");
        assert_eq!(program, "node");
        assert_eq!(args, [r"C:\npm\node_modules\@scope\tool\bin\tool.js"]);
        let direct = "@SETLOCAL\r\n\"%~dp0\\tool.exe\"   %*\r\n";
        let (program, args) = npm_shim(direct, Path::new(r"C:\npm\tool.cmd")).expect("a shim");
        assert_eq!(program, r"C:\npm\tool.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn any_other_batch_file_is_not_read_as_one() {
        for text in [
            "@echo off\r\nset X=%TEMP%\r\ncall other.cmd %*\r\n",
            "@echo hi\r\n",
            "\"%_prog%\" \"%SOMETHING%\\x.js\" %*\r\n",
        ] {
            assert_eq!(
                npm_shim(text, Path::new(r"C:\npm\tool.cmd")),
                None,
                "{text}"
            );
        }
    }

    #[test]
    fn a_share_is_told_from_a_device_and_a_drive() {
        for share in [
            r"\\server\share",
            r"\\server\share\dir",
            "//server/share/dir",
        ] {
            assert!(on_share(Path::new(share)), "{share}");
        }
        for other in [
            r"C:\dir",
            r"\\.\pipe\x",
            r"\\?\C:\dir",
            r"\\",
            r"\\\share",
            "/dir",
        ] {
            assert!(!on_share(Path::new(other)), "{other}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn only_a_batch_file_in_a_share_folder_is_started_another_way() {
        let args = [OsString::from("--mode")];
        for (program, cwd) in [
            (r"C:\bin\tool.exe", r"\\server\share\app"),
            (r"C:\bin\tool.cmd", r"C:\work"),
        ] {
            let (start, all) =
                start_in(OsStr::new(program), &args, Path::new(cwd)).expect("as asked");
            assert_eq!(start, program);
            assert_eq!(all, args);
        }
        let error = start_in(
            OsStr::new(r"C:\bin\absent.cmd"),
            &args,
            Path::new(r"\\server\share"),
        )
        .expect_err("no file to read");
        assert!(error.to_string().contains("batch file"), "{error}");
    }

    #[cfg(not(windows))]
    #[test]
    fn nothing_is_started_another_way_off_windows() {
        let args = [OsString::from("a")];
        let (start, all) =
            start_in(OsStr::new("x.cmd"), &args, Path::new("//server/share/d")).expect("as asked");
        assert_eq!((start, all), (OsString::from("x.cmd"), args.to_vec()));
    }
}
