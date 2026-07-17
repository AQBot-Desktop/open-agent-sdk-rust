use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::types::SDKMessage;

/// Result of a completed command execution.
#[derive(Debug)]
pub struct CmdOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub struct CommandRunOptions<'a> {
    pub timeout: Option<Duration>,
    pub event_sender: Option<&'a mpsc::Sender<SDKMessage>>,
    pub tool_name: &'a str,
    pub description: Option<&'a str>,
    pub tool_use_id: Option<&'a str>,
}

enum PipeMessage {
    Data {
        is_stdout: bool,
        data: Vec<u8>,
    },
    ReadError {
        stream: &'static str,
        error: std::io::Error,
    },
}

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const USER_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const STALL_THRESHOLD: Duration = Duration::from_secs(90);
const LAST_OUTPUT_CHARS: usize = 500;
const OUTPUT_THROTTLE_MS: u64 = 200;
const MAX_CAPTURE_BYTES: usize = 200_000;
const FINAL_EVENT_SEND_TIMEOUT: Duration = Duration::from_secs(1);

fn append_capped(buffer: &mut Vec<u8>, data: &[u8], truncated: &mut bool, marker: &[u8]) -> bool {
    if *truncated {
        return false;
    }

    let content_limit = MAX_CAPTURE_BYTES.saturating_sub(marker.len());
    let remaining = content_limit.saturating_sub(buffer.len());
    if data.len() <= remaining {
        buffer.extend_from_slice(data);
        return !data.is_empty();
    }

    buffer.extend_from_slice(&data[..remaining]);
    buffer.extend_from_slice(marker);
    *truncated = true;
    true
}

fn truncate_with_ellipsis(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }

    let visible_chars = max_chars.saturating_sub(3);
    format!(
        "{}...",
        value.chars().take(visible_chars).collect::<String>()
    )
}

fn tail_with_ellipsis(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }

    format!(
        "...{}",
        value
            .chars()
            .skip(char_count - max_chars)
            .collect::<String>()
    )
}

fn normalized_stream_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut output = String::from_utf8_lossy(stdout)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let stderr = String::from_utf8_lossy(stderr)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str("STDERR:\n");
        output.push_str(&stderr);
    }
    output
}

async fn clear_status(event_sender: Option<&mpsc::Sender<SDKMessage>>) -> Result<(), String> {
    if let Some(sender) = event_sender {
        send_final_events(
            sender,
            [SDKMessage::Status {
                message: String::new(),
            }],
        )
        .await?;
    }
    Ok(())
}

async fn send_final_events(
    sender: &mpsc::Sender<SDKMessage>,
    messages: impl IntoIterator<Item = SDKMessage>,
) -> Result<(), String> {
    for message in messages {
        match sender.send_timeout(message, FINAL_EVENT_SEND_TIMEOUT).await {
            Ok(()) => {}
            Err(mpsc::error::SendTimeoutError::Timeout(_)) => {
                return Err(
                    "event channel remained full while sending final command events".into(),
                );
            }
            Err(mpsc::error::SendTimeoutError::Closed(_)) => {
                return Err("event receiver dropped while sending final command events".into());
            }
        }
    }
    Ok(())
}

async fn command_error_with_status_clear(
    event_sender: Option<&mpsc::Sender<SDKMessage>>,
    command_error: String,
) -> String {
    match clear_status(event_sender).await {
        Ok(()) => command_error,
        Err(event_error) => format!("{}; {}", command_error, event_error),
    }
}

fn send_event(sender: &mpsc::Sender<SDKMessage>, message: SDKMessage, event_name: &str) {
    match sender.try_send(message) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::debug!(event_name, "event channel full; dropping command update");
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            tracing::debug!(
                event_name,
                "event receiver dropped during command execution"
            );
        }
    }
}

fn format_elapsed(secs: u64) -> String {
    if secs >= 86400 {
        let days = secs / 86400;
        let hours = (secs % 86400) / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;
        format!("运行{}天{}小时{}分{}秒", days, hours, mins, secs)
    } else if secs >= 3600 {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        let secs = secs % 60;
        format!("运行{}小时{}分{}秒", hours, mins, secs)
    } else if secs >= 60 {
        let mins = secs / 60;
        let secs = secs % 60;
        format!("运行{}分{}秒", mins, secs)
    } else {
        format!("运行{}秒", secs)
    }
}

/// Run a command with safe pipe handling, cancellation, and optional heartbeat.
///
/// Replaces all `.output().await` patterns to prevent Windows pipe-inheritance hangs.
/// For long-running commands (e.g. bash install), provide `event_sender` + `description`
/// so the frontend receives periodic status updates with partial output.
pub async fn run_command(
    cmd: &mut Command,
    abort_signal: &CancellationToken,
    options: CommandRunOptions<'_>,
) -> Result<CmdOutput, String> {
    let CommandRunOptions {
        timeout,
        event_sender,
        tool_name,
        description,
        tool_use_id,
    } = options;

    if abort_signal.is_cancelled() {
        return Err(command_error_with_status_clear(
            event_sender,
            format!("{} aborted", tool_name),
        )
        .await);
    }

    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    #[cfg(unix)]
    {
        // Put the command in its own process group so cancellation and timeout
        // can terminate the shell and every descendant it started.
        cmd.process_group(0);
    }

    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn {}: {}", tool_name, e))?;

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let (chunk_tx, mut chunk_rx) = mpsc::channel::<PipeMessage>(64);

    // Read stdout in a background task
    let tx = chunk_tx.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        let mut reader = tokio::io::BufReader::new(stdout);
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx
                        .send(PipeMessage::Data {
                            is_stdout: true,
                            data: buf[..n].to_vec(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    if tx
                        .send(PipeMessage::ReadError {
                            stream: "stdout",
                            error,
                        })
                        .await
                        .is_err()
                    {
                        tracing::debug!("command runner dropped before stdout read error");
                    }
                    break;
                }
            }
        }
    });

    // Read stderr in a background task
    let stderr_tx = chunk_tx.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        let mut reader = tokio::io::BufReader::new(stderr);
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if stderr_tx
                        .send(PipeMessage::Data {
                            is_stdout: false,
                            data: buf[..n].to_vec(),
                        })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    if stderr_tx
                        .send(PipeMessage::ReadError {
                            stream: "stderr",
                            error,
                        })
                        .await
                        .is_err()
                    {
                        tracing::debug!("command runner dropped before stderr read error");
                    }
                    break;
                }
            }
        }
    });

    drop(chunk_tx);

    let mut stdout_bytes: Vec<u8> = Vec::new();
    let mut stderr_bytes: Vec<u8> = Vec::new();
    let mut stdout_truncated = false;
    let mut stderr_truncated = false;
    let start = Instant::now();
    let mut last_data_time = Instant::now();
    let mut last_output_time = Instant::now();
    let tool_use_id = tool_use_id.map(|s| s.to_string());

    let deadline = timeout.map(|t| start + t);
    let mut ai_heartbeat_ticker = tokio::time::interval(HEARTBEAT_INTERVAL);
    // Skip the immediate first tick
    ai_heartbeat_ticker.tick().await;
    let mut user_heartbeat_ticker = tokio::time::interval(USER_HEARTBEAT_INTERVAL);
    user_heartbeat_ticker.tick().await;

    let status = loop {
        tokio::select! {
            biased;
            _ = abort_signal.cancelled() => {
                kill_child(&mut child).await;
                return Err(command_error_with_status_clear(
                    event_sender,
                    format!("{} aborted", tool_name),
                ).await);
            }
            _ = async {
                match deadline {
                    Some(d) => tokio::time::sleep_until(d.into()).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                kill_child(&mut child).await;
                let elapsed = start.elapsed().as_secs();
                return Err(command_error_with_status_clear(
                    event_sender,
                    format!("{} timed out after {}s", tool_name, elapsed),
                ).await);
            }
            chunk = chunk_rx.recv() => {
                match chunk {
                    Some(PipeMessage::Data { is_stdout, data }) => {
                        let output_changed = if is_stdout {
                            append_capped(
                                &mut stdout_bytes,
                                &data,
                                &mut stdout_truncated,
                                b"\n... (stdout truncated)",
                            )
                        } else {
                            append_capped(
                                &mut stderr_bytes,
                                &data,
                                &mut stderr_truncated,
                                b"\n... (stderr truncated)",
                            )
                        };
                        last_data_time = Instant::now();

                        // Throttled real-time output streaming
                        if let Some(ref sender) = event_sender {
                            if let Some(ref tuid) = tool_use_id {
                                let now = Instant::now();
                                if output_changed && now.duration_since(last_output_time).as_millis() as u64 >= OUTPUT_THROTTLE_MS {
                                    last_output_time = now;
                                    let partial = normalized_stream_output(
                                        &stdout_bytes,
                                        &stderr_bytes,
                                    );
                                    send_event(
                                        sender,
                                        SDKMessage::ToolOutput {
                                            tool_use_id: tuid.clone(),
                                            tool_name: tool_name.to_string(),
                                            content: partial,
                                        },
                                        "tool_output",
                                    );
                                }
                            }
                        }
                    }
                    Some(PipeMessage::ReadError { stream, error }) => {
                        kill_child(&mut child).await;
                        return Err(command_error_with_status_clear(
                            event_sender,
                            format!("Failed to read {} from {}: {}", stream, tool_name, error),
                        ).await);
                    }
                    None => {
                        // Both pipe readers finished; wait for process exit
                        match child.wait().await {
                            Ok(status) => break status,
                            Err(error) => {
                                return Err(command_error_with_status_clear(
                                    event_sender,
                                    format!("Failed to wait for {}: {}", tool_name, error),
                                ).await);
                            }
                        }
                    }
                }
            }
            _ = user_heartbeat_ticker.tick() => {
                // Fast user-facing status bar update (1s)
                if let Some(sender) = event_sender {
                    let elapsed = start.elapsed().as_secs();
                    let combined = normalized_stream_output(&stdout_bytes, &stderr_bytes);
                    let last_line = combined.lines()
                        .filter(|l| !l.is_empty())
                        .last()
                        .unwrap_or("")
                        .to_string();
                    let desc = description.unwrap_or(tool_name);
                    let display_line = truncate_with_ellipsis(&last_line, 50);
                    let msg = if !last_line.is_empty() {
                        format!("{}({})--{}", desc, format_elapsed(elapsed), display_line)
                    } else {
                        format!("{}({})--运行中...", desc, format_elapsed(elapsed))
                    };
                    send_event(sender, SDKMessage::Status { message: msg }, "status");
                }
            }
            _ = ai_heartbeat_ticker.tick() => {
                // Slow AI-awareness heartbeat (30s)
                if let Some(sender) = event_sender {
                    let elapsed = start.elapsed().as_secs();
                    let total_bytes = stdout_bytes.len() + stderr_bytes.len();

                    let combined = normalized_stream_output(&stdout_bytes, &stderr_bytes);
                    let preview = tail_with_ellipsis(&combined, LAST_OUTPUT_CHARS);

                    let desc = description.map(|d| format!(" ({})", d)).unwrap_or_default();
                    let stalled = last_data_time.elapsed() >= STALL_THRESHOLD;
                    let stalled_warn = if stalled {
                        format!("\n\n[⚠️ 输出已 {} 秒无变化，可能已卡住]", last_data_time.elapsed().as_secs())
                    } else {
                        String::new()
                    };

                    let msg = format!(
                        "[{}]{} 运行中 ({}s) — 累积 {}KB\n{}{}",
                        tool_name, desc, elapsed, total_bytes / 1024, preview, stalled_warn,
                    );
                    send_event(sender, SDKMessage::Progress { message: msg }, "progress");
                }
            }
        }
    };

    // Final flush: send complete output after process exits
    if let Some(sender) = event_sender {
        let mut final_events = Vec::with_capacity(2);
        if let Some(ref tuid) = tool_use_id {
            let full = normalized_stream_output(&stdout_bytes, &stderr_bytes);
            final_events.push(SDKMessage::ToolOutput {
                tool_use_id: tuid.clone(),
                tool_name: tool_name.to_string(),
                content: full,
            });
        }
        final_events.push(SDKMessage::Status {
            message: String::new(),
        });
        send_final_events(sender, final_events).await?;
    }

    let exit_code = status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&stdout_bytes)
        .replace("\r\n", "\n")
        .replace("\r", "\n");
    let stderr = String::from_utf8_lossy(&stderr_bytes)
        .replace("\r\n", "\n")
        .replace("\r", "\n");

    Ok(CmdOutput {
        stdout,
        stderr,
        exit_code,
    })
}

async fn kill_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: killpg does not dereference pointers. The child was
            // spawned into a process group whose id is its pid above.
            let result = unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    tracing::warn!(pid, %error, "failed to kill command process group");
                }
            }
        }
    }

    #[cfg(windows)]
    {
        // On Windows, child.kill() only kills the immediate shell (e.g. cmd.exe),
        // not grandchild processes (e.g. git.exe). Use taskkill /T to kill the
        // entire process tree.
        if let Some(pid) = child.id() {
            let mut kill_cmd = tokio::process::Command::new("taskkill");
            kill_cmd.args(["/F", "/T", "/PID", &pid.to_string()]);
            kill_cmd.creation_flags(0x08000000);
            match kill_cmd.output().await {
                Ok(output) if !output.status.success() => {
                    tracing::warn!(pid, status = ?output.status, "taskkill failed");
                }
                Err(error) => tracing::warn!(pid, %error, "failed to start taskkill"),
                _ => {}
            }
        }
    }
    if let Err(error) = child.kill().await {
        tracing::debug!(%error, "child process already stopped before kill");
    }
    if let Err(error) = child.wait().await {
        tracing::warn!(%error, "failed to wait for child process termination");
    }
}
