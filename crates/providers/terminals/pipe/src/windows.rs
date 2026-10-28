// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Windows: the child starts inside its Job Object, inherits only its own pipes, opens no
//! console window, and is found on `PATH` with `PATHEXT`.
//!
//! The standard library's `Command` can do none of the first two (its attribute lists are
//! unstable), so the child is started with `CreateProcessW` here, with the standard library's
//! quoting rules for an argument and for a batch file's arguments.

#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::marker::PhantomData;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::ptr;

use ai6::directorates::q::terminal::ExitInfo;
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, TRUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

use crate::process_tree::ProcessTree;
use crate::{ErrorOutput, Launch};

/// What `PATHEXT` holds when the environment does not say.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// A started child: its process handle and ID.
pub(crate) struct Process {
    handle: OwnedHandle,
    pid: u32,
}

impl Process {
    pub(crate) fn id(&self) -> u32 {
        self.pid
    }

    /// How the child ended, once it has.
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitInfo>> {
        // SAFETY: the handle is owned and open.
        match unsafe { WaitForSingleObject(self.raw(), 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0u32;
                // SAFETY: as above; `code` outlives the call.
                if unsafe { GetExitCodeProcess(self.raw(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(exit_info(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }

    /// Terminates the child with exit code 1; an error means it had already exited.
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        // SAFETY: the handle is owned and open.
        if unsafe { TerminateProcess(self.raw(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(crate) fn wait(&mut self) -> io::Result<()> {
        // SAFETY: the handle is owned and open.
        match unsafe { WaitForSingleObject(self.raw(), INFINITE) } {
            WAIT_OBJECT_0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    }

    fn raw(&self) -> HANDLE {
        self.handle.as_raw_handle()
    }
}

/// Starts the program in a new kill-on-close Job Object, which it belongs to from its first
/// instruction (`PROC_THREAD_ATTRIBUTE_JOB_LIST`), so every process it starts is in the job
/// too. It inherits only its three standard handles (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`),
/// whatever else this process holds inheritable, and gets a console of its own with no window
/// (`CREATE_NO_WINDOW`): a Station without a console would otherwise open a visible one.
///
/// The program is found by [`resolve`]; a bare name found on no `PATH` directory is refused
/// rather than looked for in the current directory. A `.bat` or `.cmd` file runs under the
/// system's `cmd.exe` with its arguments quoted as the standard library quotes them, which
/// refuses an argument holding a CR or LF.
pub(crate) fn spawn(launch: Launch<'_>) -> io::Result<(Process, ProcessTree)> {
    let program = resolve(launch.program, launch.env);
    if !program.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the program is not a file, nor found on the child's PATH",
        ));
    }
    let (application, mut command_line) = if is_batch_file(&program) {
        let mut cmd = system_directory()?;
        cmd.push("cmd.exe");
        (cmd, batch_command_line(&program, launch.args)?)
    } else {
        let line = command_line(&program, launch.args)?;
        (program, line)
    };
    command_line.push(0);
    let application = wide_nul(application.as_os_str())?;
    let mut environment = launch.env.map(environment_block).transpose()?;
    let directory = launch
        .cwd
        .map(|dir| wide_nul(OsStr::new(dir)))
        .transpose()?;

    // Inheritable copies of the child's ends, made just before the child is started: any
    // other process started meanwhile with every inheritable handle (the standard library's
    // `Command` starts them so) would also hold them, and keep a pipe open.
    let stdin = inheritable(launch.stdin.as_raw_handle())?;
    let stdout = inheritable(launch.stdout.as_raw_handle())?;
    let stderr = match launch.stderr {
        ErrorOutput::Stdout => None,
        ErrorOutput::Null => {
            let null = std::fs::OpenOptions::new().write(true).open("NUL")?;
            Some(inheritable(null.as_raw_handle())?)
        }
        ErrorOutput::Pipe(writer) => Some(inheritable(writer.as_raw_handle())?),
    };
    let error_handle = stderr.as_ref().unwrap_or(&stdout).as_raw_handle();
    let mut inherited = vec![stdin.as_raw_handle(), stdout.as_raw_handle()];
    inherited.extend(stderr.as_ref().map(AsRawHandle::as_raw_handle));

    let tree = ProcessTree::new()?;
    let jobs = [tree.handle()];
    let mut attributes = AttributeList::new(2)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &inherited)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin.as_raw_handle();
    startup.StartupInfo.hStdOutput = stdout.as_raw_handle();
    startup.StartupInfo.hStdError = error_handle;
    startup.lpAttributeList = attributes.as_ptr();
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: every pointer is to a NUL-terminated buffer, a structure or a list that lives past
    // the call; the handles in the attribute list are open and inheritable; `info` receives the
    // new process's handles, owned below.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            TRUE,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            environment
                .as_mut()
                .map_or(ptr::null(), |block| block.as_mut_ptr().cast_const().cast()),
            directory.as_ref().map_or(ptr::null(), |dir| dir.as_ptr()),
            &startup.StartupInfo,
            &mut info,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateProcessW returned this thread handle for the caller to close.
    unsafe { CloseHandle(info.hThread) };
    let process = Process {
        // SAFETY: CreateProcessW returned this process handle for the caller to own.
        handle: unsafe { OwnedHandle::from_raw_handle(info.hProcess as RawHandle) },
        pid: info.dwProcessId,
    };
    Ok((process, tree))
}

/// A process and thread attribute list of `'a` handle lists.
struct AttributeList<'a> {
    buffer: Vec<usize>,
    _values: PhantomData<&'a [HANDLE]>,
}

impl<'a> AttributeList<'a> {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0usize;
        // SAFETY: with a null list the call only reports the size it needs; it fails doing so.
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &mut size) };
        let mut buffer = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        // SAFETY: `buffer` holds at least `size` bytes, aligned for the list.
        if unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), count, 0, &mut size)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            buffer,
            _values: PhantomData,
        })
    }

    /// Sets `attribute` to `values`, which the list points to until it is dropped.
    fn set(&mut self, attribute: u32, values: &'a [HANDLE]) -> io::Result<()> {
        // SAFETY: the list was initialized for this many attributes, and `values` outlives it.
        if unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                values.as_ptr().cast(),
                std::mem::size_of_val(values),
                ptr::null_mut(),
                ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }
}

impl Drop for AttributeList<'_> {
    fn drop(&mut self) {
        // SAFETY: the list was initialized in `new` and is deleted once.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}

/// An inheritable duplicate of `handle`, which stays as it is.
fn inheritable(handle: RawHandle) -> io::Result<OwnedHandle> {
    let mut duplicate: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no closing; `handle` is open
    // for the call, and `duplicate` receives a new handle owned below.
    let duplicated = unsafe {
        let me = GetCurrentProcess();
        DuplicateHandle(
            me,
            handle,
            me,
            &mut duplicate,
            0,
            TRUE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if duplicated == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: DuplicateHandle made this handle for the caller to own.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate as RawHandle) })
}

/// The system directory (`C:\Windows\System32`).
fn system_directory() -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 260];
    loop {
        // SAFETY: the buffer is writable for its length.
        let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
        match length as usize {
            0 => return Err(io::Error::last_os_error()),
            n if n < buffer.len() => {
                use std::os::windows::ffi::OsStringExt;
                return Ok(PathBuf::from(OsString::from_wide(&buffer[..n])));
            }
            n => buffer.resize(n, 0),
        }
    }
}

/// `text` as UTF-16 with a terminating NUL; a NUL inside it is refused.
fn wide_nul(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut wide: Vec<u16> = text.encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a NUL character in the program, its directory or its environment",
        ));
    }
    wide.push(0);
    Ok(wide)
}

/// The environment block of `env`: `NAME=value` strings, each NUL-terminated, sorted by name
/// without regard to case as Windows expects, then one more NUL. A name differing from an
/// earlier one only in case is dropped, since Windows would treat them as one.
fn environment_block(env: &BTreeMap<String, String>) -> io::Result<Vec<u16>> {
    let mut entries: Vec<(&String, &String)> = env.iter().collect();
    entries.sort_by_key(|(name, _)| name.to_uppercase());
    entries.dedup_by_key(|(name, _)| name.to_uppercase());
    let mut block = Vec::new();
    for (name, value) in entries {
        // A name may start with `=` (`=C:` holds a drive's directory) and holds none after.
        let refused = name.is_empty()
            || name.chars().skip(1).any(|c| c == '=')
            || name.contains('\0')
            || value.contains('\0');
        if refused {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("the environment variable {name:?} cannot be passed"),
            ));
        }
        block.extend(name.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// Whether `program` names a batch file, which only `cmd.exe` runs.
fn is_batch_file(program: &Path) -> bool {
    program
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|ext| ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd"))
}

/// The command line of a program: its path in quotes, then each argument as the standard
/// library and the Microsoft C runtime quote one, so the program's argument parser reads them
/// back exactly.
fn command_line(program: &Path, args: &[String]) -> io::Result<Vec<u16>> {
    let program = program.as_os_str();
    if program.encode_wide().any(|c| c == u16::from(b'"')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a program path holding `\"`",
        ));
    }
    let mut line = vec![u16::from(b'"')];
    line.extend(program.encode_wide());
    line.push(u16::from(b'"'));
    for arg in args {
        line.push(u16::from(b' '));
        append_arg(&mut line, arg)?;
    }
    Ok(line)
}

/// Appends `arg`, quoted when it is empty or holds a space or tab, its backslashes doubled
/// before a quote and its quotes escaped.
fn append_arg(line: &mut Vec<u16>, arg: &str) -> io::Result<()> {
    refuse_nul(arg)?;
    let quote = arg.is_empty() || arg.contains([' ', '\t']);
    if quote {
        line.push(u16::from(b'"'));
    }
    let mut backslashes = 0usize;
    for c in arg.encode_utf16() {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                // n backslashes before a quote become 2n + 1.
                line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes + 1));
            }
            backslashes = 0;
        }
        line.push(c);
    }
    if quote {
        // n backslashes before the closing quote become 2n.
        line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        line.push(u16::from(b'"'));
    }
    Ok(())
}

/// The `cmd.exe` command line that runs `script` with `args`, quoted as the standard library
/// quotes a batch file's arguments: `%` cannot expand a variable, a quote is doubled, and an
/// argument holding CR or LF, which would end the command, is refused.
fn batch_command_line(script: &Path, args: &[String]) -> io::Result<Vec<u16>> {
    let script = script.as_os_str();
    let mut line: Vec<u16> = "cmd.exe /e:ON /v:OFF /d /c \"".encode_utf16().collect();
    line.push(u16::from(b'"'));
    if script.encode_wide().any(|c| c == u16::from(b'"'))
        || script.encode_wide().last() == Some(u16::from(b'\\'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a batch file path holding `\"` or ending with `\\`",
        ));
    }
    line.extend(script.encode_wide());
    line.push(u16::from(b'"'));
    for arg in args {
        line.push(u16::from(b' '));
        if arg.contains(['\r', '\n']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a batch file argument holding CR or LF",
            ));
        }
        append_batch_arg(&mut line, arg)?;
    }
    line.push(u16::from(b'"'));
    Ok(line)
}

/// Appends a batch file's argument, quoted unless it holds only letters, digits and
/// `#$*+-./:?@\_`, with `"` doubled and each `%` preceded by `%%cd:~,`, an empty substring of
/// `%cd%` that keeps `cmd.exe` from reading `%NAME%` as a variable.
fn append_batch_arg(line: &mut Vec<u16>, arg: &str) -> io::Result<()> {
    refuse_nul(arg)?;
    const UNQUOTED: &str = r"#$*+-./:?@\_";
    let quote = arg.is_empty()
        || arg.ends_with('\\')
        || arg.chars().any(|c| {
            (c.is_ascii() && !(c.is_ascii_alphanumeric() || UNQUOTED.contains(c))) || c.is_control()
        });
    if quote {
        line.push(u16::from(b'"'));
    }
    let mut backslashes = 0usize;
    for c in arg.encode_utf16() {
        if c == u16::from(b'\\') {
            backslashes += 1;
        } else {
            if c == u16::from(b'"') {
                // n backslashes before a quote become 2n, and the quote is doubled.
                line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
                line.push(u16::from(b'"'));
            } else if c == u16::from(b'%') || c == u16::from(b'\r') {
                line.extend("%%cd:~,".encode_utf16());
            }
            backslashes = 0;
        }
        line.push(c);
    }
    if quote {
        line.extend(std::iter::repeat_n(u16::from(b'\\'), backslashes));
        line.push(u16::from(b'"'));
    }
    Ok(())
}

fn refuse_nul(arg: &str) -> io::Result<()> {
    if arg.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "an argument holding a NUL character",
        ));
    }
    Ok(())
}

/// The program found on the child's `PATH` as a shell finds it: a name with an extension as
/// given, else with each `PATHEXT` extension (`codex` as `codex.cmd`). A name with a directory
/// in it is taken as given, or with a `PATHEXT` extension when it has none and is no file; a
/// bare name found nowhere is returned as given.
pub(crate) fn resolve(program: &str, env: Option<&BTreeMap<String, String>>) -> PathBuf {
    let lookup = |name: &str| -> Option<OsString> {
        match env {
            Some(env) => env
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| OsString::from(value)),
            None => std::env::var_os(name),
        }
    };
    let extensions = lookup("PATHEXT")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| DEFAULT_PATHEXT.to_owned());
    let with_extensions = |base: &Path| -> Option<PathBuf> {
        extensions
            .split(';')
            .filter(|e| e.starts_with('.'))
            .map(|extension| {
                let mut name = base.as_os_str().to_owned();
                name.push(extension);
                PathBuf::from(name)
            })
            .find(|candidate| candidate.is_file())
    };
    let given = Path::new(program);
    // npm puts a POSIX script named `codex` beside `codex.cmd`: a bare name is not run as is.
    let named = given.extension().is_some();
    if program.contains(['/', '\\']) {
        if given.is_file() || named {
            return given.to_path_buf();
        }
        return with_extensions(given).unwrap_or_else(|| given.to_path_buf());
    }
    let Some(path) = lookup("PATH") else {
        return given.to_path_buf();
    };
    for dir in std::env::split_paths(&path) {
        let exact = dir.join(program);
        if named && exact.is_file() {
            return exact;
        }
        if let Some(found) = with_extensions(&exact) {
            return found;
        }
    }
    given.to_path_buf()
}

/// The exit code as the signed 32-bit value the standard library and every transport report:
/// Windows' `0xFFFFFFFF` is `-1` and its NTSTATUS `0xC000013A` is `-1073741510`.
fn exit_info(code: u32) -> ExitInfo {
    ExitInfo {
        running: false,
        code: Some(i64::from(code.cast_signed())),
        signal: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exit_code_with_the_high_bit_set_is_negative() {
        for (raw, code) in [
            (0, 0),
            (1, 1),
            (0xFFFF_FFFF, -1),
            (0xC000_013A, -1_073_741_510),
        ] {
            let exit = exit_info(raw);
            assert_eq!(exit.code, Some(code), "{raw:#010X}");
            assert!(!exit.running && exit.signal.is_none());
        }
    }

    fn text(line: &[u16]) -> String {
        String::from_utf16(line).unwrap()
    }

    #[test]
    fn arguments_are_quoted_as_the_c_runtime_reads_them() {
        let line = command_line(
            Path::new(r"C:\t\p.exe"),
            &[
                "plain".to_owned(),
                String::new(),
                "two words".to_owned(),
                r#"a"b"#.to_owned(),
                r"tail\".to_owned(),
                r"spaced tail\ ".to_owned(),
                r#"x\"y"#.to_owned(),
            ],
        )
        .unwrap();
        assert_eq!(
            text(&line),
            r#""C:\t\p.exe" plain "" "two words" a\"b tail\ "spaced tail\ " x\\\"y"#
        );
        assert!(command_line(Path::new("p.exe"), &["a\0b".to_owned()]).is_err());
    }

    #[test]
    fn a_batch_files_arguments_are_quoted_and_cannot_expand_or_end_the_command() {
        let line = batch_command_line(
            Path::new(r"C:\t\codex.cmd"),
            &[
                "-c".to_owned(),
                r#"k="v w""#.to_owned(),
                "%PATH%".to_owned(),
                r"C:\dir\".to_owned(),
                "a&b".to_owned(),
            ],
        )
        .unwrap();
        assert_eq!(
            text(&line),
            r#"cmd.exe /e:ON /v:OFF /d /c ""C:\t\codex.cmd" -c "k=""v w""" "%%cd:~,%PATH%%cd:~,%" "C:\dir\\" "a&b"""#
        );
        assert!(batch_command_line(Path::new("x.cmd"), &["a\nb".to_owned()]).is_err());
        assert!(batch_command_line(Path::new(r#"x".cmd"#), &[]).is_err());
    }

    #[test]
    fn the_environment_block_is_sorted_without_case_and_ends_with_two_nuls() {
        let env = BTreeMap::from([
            ("b".to_owned(), "2".to_owned()),
            ("A".to_owned(), "1".to_owned()),
            ("Path".to_owned(), r"C:\x".to_owned()),
        ]);
        let block = environment_block(&env).unwrap();
        assert_eq!(text(&block), "A=1\0b=2\0Path=C:\\x\0\0");
        assert_eq!(text(&environment_block(&BTreeMap::new()).unwrap()), "\0\0");
        let bad = BTreeMap::from([("A=B".to_owned(), "1".to_owned())]);
        assert!(environment_block(&bad).is_err());
    }
}
