// Copyright (C) 2026 AnalyseDeCircuit
// SPDX-License-Identifier: GPL-3.0-only

//! Regression tests for the Windows pipe abort.
//!
//! With `Stdio::piped()` tokio services the std overlapped pipe ends through
//! blocking `std::fs::File` I/O. Dropping an in-flight read future leaves its
//! blocking operation running, so the next read on the same end overlaps it
//! and std aborts the process from its synchronous I/O path. These tests run
//! exactly that cancellation pattern against `spawn`; a regression to
//! overlapped ends kills the test process outright.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use oxideterm_process_pipes::spawn;

fn echo_child() -> Command {
    if cfg!(windows) {
        // `more` copies stdin to stdout through pipes in both directions.
        let mut command = Command::new("cmd");
        command.args(["/C", "more"]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", "cat"]);
        command
    }
}

#[tokio::test]
async fn pipes_carry_stdin_to_stdout_in_both_directions() {
    let mut command = echo_child();
    command.kill_on_drop(true);
    let mut child = spawn(&mut command, true, true, false).expect("spawn");
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let mut stdout = child.stdout.take().expect("stdout pipe");

    stdin
        .write_all(b"oxideterm-pipe-round-trip\n")
        .await
        .expect("write");
    drop(stdin);

    let mut line = String::new();
    let read = tokio::time::timeout(Duration::from_secs(10), async {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).await.expect("read");
        buffer
    })
    .await
    .expect("echo child must answer within the timeout");
    line.push_str(&String::from_utf8_lossy(&read));

    assert!(
        line.trim_end().ends_with("oxideterm-pipe-round-trip"),
        "child echoed unexpected data: {line:?}"
    );
    child.kill().await.expect("kill");
}

/// Dropping a pending read and issuing another one on the same stdout end is
/// the exact overlap that aborts the process with overlapped pipe handles.
/// `ping` emits one line per second, so reads reliably stay pending between
/// cancellations.
#[cfg(windows)]
#[tokio::test]
async fn cancelled_reads_do_not_abort_the_process() {
    let mut command = Command::new("ping");
    command.args(["-n", "20", "127.0.0.1"]).kill_on_drop(true);
    let mut child = spawn(&mut command, false, true, false).expect("spawn");
    let mut stdout = child.stdout.take().expect("stdout pipe");

    let mut received = 0usize;
    for _ in 0..12 {
        let mut buffer = vec![0u8; 512];
        let pending = stdout.read(&mut buffer);
        // Dispatch the read, then drop it while the child is still silent so
        // the abandoned operation keeps running on the blocking pool.
        match tokio::time::timeout(Duration::from_millis(50), pending).await {
            Ok(Ok(count)) => received += count,
            Ok(Err(error)) => panic!("read failed: {error}"),
            Err(_) => {}
        }
    }
    // A final read must still observe ping output: the pipe stays usable
    // after the cancelled reads and the process is still alive.
    let mut buffer = vec![0u8; 4096];
    let count = tokio::time::timeout(Duration::from_secs(10), stdout.read(&mut buffer))
        .await
        .expect("ping must produce output")
        .expect("read");
    assert!(
        count > 0,
        "no data after cancelled reads; received before: {received}"
    );
    child.kill().await.expect("kill");
}
