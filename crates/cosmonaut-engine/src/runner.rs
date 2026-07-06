//! Subprocess helper. Runs a command, streams stdout + stderr line-by-line
//! to the engine's event channel, returns Ok on exit-0.

use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_stream::wrappers::LinesStream;

use crate::{Event, LogStream};

/// Run `cmd args...`. Captures stdout + stderr line-by-line and forwards
/// each line as an [`Event::Log`] over `events`. Returns Err on non-zero
/// exit, on inability to spawn, or on I/O error reading the pipes.
///
/// `cancel` is honored only insofar as we drop our handle on cancel; the
/// child may continue briefly until it notices broken pipes / SIGPIPE.
/// Steps that need hard cancellation kill the child explicitly.
pub async fn run(cmd: &str, args: &[&str], events: &mpsc::Sender<Event>) -> Result<()> {
    log_command(events, cmd, args).await;

    let mut child = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawning {cmd}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("no stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("no stderr pipe"))?;

    let stdout_events = events.clone();
    let stderr_events = events.clone();

    let stdout_task = tokio::spawn(async move {
        let mut lines = LinesStream::new(BufReader::new(stdout).lines());
        while let Some(Ok(line)) = lines.next().await {
            let _ = stdout_events
                .send(Event::Log {
                    stream: LogStream::Stdout,
                    line,
                })
                .await;
        }
    });
    let stderr_task = tokio::spawn(async move {
        let mut lines = LinesStream::new(BufReader::new(stderr).lines());
        while let Some(Ok(line)) = lines.next().await {
            let _ = stderr_events
                .send(Event::Log {
                    stream: LogStream::Stderr,
                    line,
                })
                .await;
        }
    });

    let status = child.wait().await.context("waiting for child")?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if !status.success() {
        return Err(anyhow!("{cmd} exited with {status}"));
    }
    Ok(())
}

/// Same as [`run`] but feeds `stdin_data` to the child's stdin. Used
/// for sfdisk's partition-table script and cryptsetup's passphrase.
pub async fn run_with_stdin(
    cmd: &str,
    args: &[&str],
    stdin_data: &[u8],
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    log_command(events, cmd, args).await;

    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawning {cmd}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(stdin_data).await.context("writing stdin")?;
        stdin.shutdown().await.context("closing stdin")?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("no stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("no stderr pipe"))?;

    let stdout_events = events.clone();
    let stderr_events = events.clone();

    let stdout_task = tokio::spawn(async move {
        let mut lines = LinesStream::new(BufReader::new(stdout).lines());
        while let Some(Ok(line)) = lines.next().await {
            let _ = stdout_events
                .send(Event::Log {
                    stream: LogStream::Stdout,
                    line,
                })
                .await;
        }
    });
    let stderr_task = tokio::spawn(async move {
        let mut lines = LinesStream::new(BufReader::new(stderr).lines());
        while let Some(Ok(line)) = lines.next().await {
            let _ = stderr_events
                .send(Event::Log {
                    stream: LogStream::Stderr,
                    line,
                })
                .await;
        }
    });

    let status = child.wait().await.context("waiting for child")?;
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if !status.success() {
        return Err(anyhow!("{cmd} exited with {status}"));
    }
    Ok(())
}

/// Capture stdout of a one-shot command (e.g. `blkid -o value -s UUID`).
pub async fn capture_stdout(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .await
        .with_context(|| format!("spawning {cmd}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "{cmd} exited with {}; stderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn log_command(events: &mpsc::Sender<Event>, cmd: &str, args: &[&str]) {
    let line = format!("+ {} {}", cmd, args.join(" "));
    let _ = events
        .send(Event::Log {
            stream: LogStream::Engine,
            line,
        })
        .await;
}
