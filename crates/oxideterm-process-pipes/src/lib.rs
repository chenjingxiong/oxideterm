// Copyright (C) 2026 AnalyseDeCircuit
// SPDX-License-Identifier: GPL-3.0-only

//! Child-process stdio pipes that stay safe under tokio on Windows.
//!
//! `std::process` creates `Stdio::piped()` ends as overlapped handles on
//! Windows. tokio converts those raw handles into `std::fs::File` and
//! services them with blocking-pool threads. When two operations overlap on
//! one overlapped handle (for example a cancelled future leaves its blocking
//! read running while the next one starts), the wait in std's synchronous
//! I/O path returns before that operation completes and std aborts the whole
//! process with `fatal runtime error: I/O error: operation failed to
//! complete synchronously`. Spawning through `std::io::pipe()` instead gives
//! the parent synchronous (CreatePipe) handles, whose blocking reads and
//! writes finish inside the kernel and can never reach that abort path,
//! regardless of how futures are cancelled or interleaved.

use std::io;
use std::process::Stdio;

/// Writing end of a child's stdin held by the parent.
#[cfg(windows)]
pub type ProcessStdin = tokio::fs::File;
/// Reading end of a child's stdout held by the parent.
#[cfg(windows)]
pub type ProcessStdout = tokio::fs::File;
/// Reading end of a child's stderr held by the parent.
#[cfg(windows)]
pub type ProcessStderr = tokio::fs::File;

/// On Unix the tokio pipe ends are registered with the async I/O driver and
/// the overlapped-handle hazard does not exist; the stdio types are used
/// directly so the platform keeps its native async behavior.
#[cfg(not(windows))]
pub type ProcessStdin = tokio::process::ChildStdin;
#[cfg(not(windows))]
pub type ProcessStdout = tokio::process::ChildStdout;
#[cfg(not(windows))]
pub type ProcessStderr = tokio::process::ChildStderr;

/// A spawned child together with the parent-held ends of its pipes.
pub struct PipedChild {
    child: tokio::process::Child,
    pub stdin: Option<ProcessStdin>,
    pub stdout: Option<ProcessStdout>,
    pub stderr: Option<ProcessStderr>,
}

impl PipedChild {
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }

    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    pub async fn kill(&mut self) -> io::Result<()> {
        self.child.kill().await
    }

    pub fn start_kill(&mut self) -> io::Result<()> {
        self.child.start_kill()
    }
}

/// Spawns `command` with pipes for every direction requested by a `true`
/// flag. Stdio left as `false` is not touched, so callers configure null or
/// inherited stdio themselves before calling.
pub fn spawn(
    command: &mut tokio::process::Command,
    want_stdin: bool,
    want_stdout: bool,
    want_stderr: bool,
) -> io::Result<PipedChild> {
    imp::spawn(command, want_stdin, want_stdout, want_stderr)
}

#[cfg(windows)]
mod imp {
    use super::*;

    pub(super) fn spawn(
        command: &mut tokio::process::Command,
        want_stdin: bool,
        want_stdout: bool,
        want_stderr: bool,
    ) -> io::Result<PipedChild> {
        let stdin = want_stdin.then(|| {
            // The child reads stdin, so the child gets the reading end and
            // Stdio::from duplicates that end inheritable for the spawn.
            let (child_end, ours) = std::io::pipe()?;
            command.stdin(Stdio::from(child_end));
            Ok(parent_end_file(ours))
        });
        let stdout = want_stdout.then(|| {
            let (ours, child_end) = std::io::pipe()?;
            command.stdout(Stdio::from(child_end));
            Ok(parent_end_file(ours))
        });
        let stderr = want_stderr.then(|| {
            let (ours, child_end) = std::io::pipe()?;
            command.stderr(Stdio::from(child_end));
            Ok(parent_end_file(ours))
        });
        let child = command.spawn()?;
        Ok(PipedChild {
            child,
            stdin: stdin.transpose()?,
            stdout: stdout.transpose()?,
            stderr: stderr.transpose()?,
        })
    }

    fn parent_end_file(pipe: impl Into<std::os::windows::io::OwnedHandle>) -> tokio::fs::File {
        tokio::fs::File::from_std(std::fs::File::from(pipe.into()))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub(super) fn spawn(
        command: &mut tokio::process::Command,
        want_stdin: bool,
        want_stdout: bool,
        want_stderr: bool,
    ) -> io::Result<PipedChild> {
        if want_stdin {
            command.stdin(Stdio::piped());
        }
        if want_stdout {
            command.stdout(Stdio::piped());
        }
        if want_stderr {
            command.stderr(Stdio::piped());
        }
        let mut child = command.spawn()?;
        Ok(PipedChild {
            stdin: if want_stdin { child.stdin.take() } else { None },
            stdout: if want_stdout {
                child.stdout.take()
            } else {
                None
            },
            stderr: if want_stderr {
                child.stderr.take()
            } else {
                None
            },
            child,
        })
    }
}
