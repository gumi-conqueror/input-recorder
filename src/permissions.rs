use std::{
    collections::BTreeSet, ffi::OsStr, mem::size_of, os::windows::ffi::OsStrExt, path::Path,
};

use anyhow::{Context, Result, ensure};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{
                GetCurrentProcess, GetCurrentProcessId, OpenProcessToken, WaitForInputIdle,
                WaitForSingleObject,
            },
        },
        UI::{
            Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
    core::{PCWSTR, w},
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns a non-pseudo token or process handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub(crate) fn is_elevated() -> Result<bool> {
    // SAFETY: token and output buffers are owned by this function and sized for TOKEN_ELEVATION.
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let token = Handle(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut length = 0;
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )?;
        Ok(elevation.TokenIsElevated != 0)
    }
}

pub(crate) fn session_id() -> Result<u32> {
    let mut session = 0;
    // SAFETY: the process ID is ours and session points to a writable u32.
    unsafe {
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session)?;
    }
    Ok(session)
}

pub(crate) fn check_session() -> Result<()> {
    let arguments: Vec<_> = std::env::args().collect();
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--expected-session")
    {
        let expected: u32 = arguments
            .get(index + 1)
            .context("missing expected session")?
            .parse()?;
        ensure!(
            session_id()? == expected,
            "elevation changed the target Windows session"
        );
        ensure!(is_elevated()?, "the restarted process is not elevated");
    }
    Ok(())
}

pub(crate) fn restart(directory: &Path, selected: Option<&BTreeSet<String>>) -> Result<()> {
    ensure!(!is_elevated()?, "this process is already elevated");
    let executable = std::env::current_exe()?;
    let directory = std::fs::canonicalize(directory)?;
    let mut arguments = Vec::new();
    for argument in [
        OsStr::new("--data-dir"),
        directory.as_os_str(),
        OsStr::new("--expected-session"),
        OsStr::new(&session_id()?.to_string()),
    ] {
        if !arguments.is_empty() {
            arguments.push(u16::from(b' '));
        }
        arguments.extend(quote(argument));
    }
    if let Some(selected) = selected {
        let selection = selected.iter().cloned().collect::<Vec<_>>().join(",");
        arguments.extend(" --selection ".encode_utf16());
        arguments.extend(quote(OsStr::new(&selection)));
    }
    ensure!(
        arguments.len() < 32766,
        "too many selected device IDs for an elevated restart"
    );
    arguments.push(0);
    let executable: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    // SAFETY: strings are NUL terminated and remain alive through ShellExecuteExW. The new process runs only this executable.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let mut info = SHELLEXECUTEINFOW {
            cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("runas"),
            lpFile: PCWSTR(executable.as_ptr()),
            lpParameters: PCWSTR(arguments.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        let launched = ShellExecuteExW(&mut info);
        CoUninitialize();
        launched.context("UAC request cancelled or elevated startup failed")?;
        ensure!(
            !info.hProcess.is_invalid(),
            "elevated process handle is unavailable"
        );
        let process = Handle(info.hProcess);
        ensure!(
            WaitForInputIdle(process.0, 15_000) == 0,
            "elevated window did not become ready; keeping this window open"
        );
        ensure!(
            WaitForSingleObject(process.0, 0) == WAIT_TIMEOUT,
            "elevated process exited during startup"
        );
    }
    Ok(())
}

fn quote(argument: &OsStr) -> Vec<u16> {
    let mut result = vec![u16::from(b'"')];
    let mut slashes = 0;
    for unit in argument.encode_wide() {
        if unit == u16::from(b'\\') {
            slashes += 1;
            continue;
        }
        if unit == u16::from(b'"') {
            result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes));
        }
        result.push(unit);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n(u16::from(b'\\'), slashes * 2));
    result.push(u16::from(b'"'));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_spaces_quotes_and_trailing_backslashes() {
        for (input, expected) in [
            ("", "\"\""),
            ("a b", "\"a b\""),
            ("a\"b", "\"a\\\"b\""),
            ("C:\\dir\\", "\"C:\\dir\\\\\""),
        ] {
            assert_eq!(
                String::from_utf16(&quote(OsStr::new(input))).expect("UTF-16"),
                expected
            );
        }
    }

    #[test]
    fn reads_current_process_permissions() -> Result<()> {
        let _ = is_elevated()?;
        let _ = session_id()?;
        Ok(())
    }
}
