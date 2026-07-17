use open_agent_sdk::tools::command_runner::{run_command, CmdOutput, CommandRunOptions};
use open_agent_sdk::tools::{execute_tools, ToolRegistry};
use open_agent_sdk::types::{
    ContentBlock, Message, MessageRole, SDKMessage, Tool, ToolError, ToolUseContext,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

fn create_test_context(dir: &str) -> ToolUseContext {
    ToolUseContext::new(dir.to_string())
}

#[cfg(unix)]
async fn run_test_command(
    command: &str,
    timeout: Duration,
    event_sender: Option<&tokio::sync::mpsc::Sender<SDKMessage>>,
    tool_use_id: Option<&str>,
) -> Result<CmdOutput, String> {
    let mut process = Command::new("sh");
    process.args(["-c", command]).current_dir("/tmp");
    run_command(
        &mut process,
        &CancellationToken::new(),
        CommandRunOptions {
            timeout: Some(timeout),
            event_sender,
            tool_name: "test command",
            description: Some("test command"),
            tool_use_id,
        },
    )
    .await
}

// --- Registry Tests ---

#[test]
fn test_default_registry() {
    let registry = ToolRegistry::default_registry();
    assert!(registry.len() > 0);

    // Check core tools exist
    assert!(registry.get("Bash").is_some());
    assert!(registry.get("Read").is_some());
    assert!(registry.get("Write").is_some());
    assert!(registry.get("Edit").is_some());
    assert!(registry.get("Glob").is_some());
    assert!(registry.get("Grep").is_some());
    assert!(registry.get("WebFetch").is_some());
    assert!(registry.get("WebSearch").is_some());
    assert!(registry.get("AskUserQuestion").is_some());
    assert!(registry.get("TaskCreate").is_some());
    assert!(registry.get("TaskGet").is_some());
    assert!(registry.get("TaskList").is_some());
    assert!(registry.get("TaskUpdate").is_some());
    assert!(registry.get("ToolSearch").is_some());
}

#[test]
fn test_registry_register_custom() {
    use async_trait::async_trait;
    use open_agent_sdk::types::{ToolError, ToolInputSchema, ToolResult};
    use serde_json::Value;

    struct CustomTool;

    #[async_trait]
    impl Tool for CustomTool {
        fn name(&self) -> &str {
            "CustomTool"
        }
        fn description(&self) -> &str {
            "A test tool"
        }
        fn input_schema(&self) -> ToolInputSchema {
            ToolInputSchema::default()
        }
        async fn call(
            &self,
            _input: Value,
            _ctx: &ToolUseContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::text("custom result"))
        }
    }

    let mut registry = ToolRegistry::new();
    assert!(registry.is_empty());

    registry.register(Arc::new(CustomTool));
    assert_eq!(registry.len(), 1);
    assert!(registry.get("CustomTool").is_some());
}

#[test]
fn test_registry_filter() {
    let registry = ToolRegistry::default_registry();
    let read_only = registry.filter(|t| t.is_read_only(&json!({})));
    assert!(read_only.len() > 0);

    // Read, Glob, Grep should be read-only
    let names: Vec<&str> = read_only.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"Read"));
    assert!(names.contains(&"Glob"));
    assert!(names.contains(&"Grep"));
}

#[test]
fn test_registry_retain() {
    let mut registry = ToolRegistry::default_registry();
    let initial_count = registry.len();

    registry.retain(&["Read", "Glob", "Grep"]);
    assert_eq!(registry.len(), 3);
    assert!(registry.len() < initial_count);

    assert!(registry.get("Read").is_some());
    assert!(registry.get("Bash").is_none());
}

#[test]
fn test_registry_remove() {
    let mut registry = ToolRegistry::default_registry();
    assert!(registry.get("Bash").is_some());

    registry.remove(&["Bash"]);
    assert!(registry.get("Bash").is_none());
}

// --- Bash Tool Tests ---

#[test]
fn test_bash_schema_supports_timeout() {
    let registry = ToolRegistry::default_registry();
    let bash = registry.get("Bash").unwrap();

    assert!(bash.input_schema().properties.contains_key("timeout"));
}

#[tokio::test]
async fn test_bash_echo() {
    let registry = ToolRegistry::default_registry();
    let bash = registry.get("Bash").unwrap();
    let ctx = create_test_context("/tmp");

    let result = bash
        .call(json!({"command": "echo 'hello world'"}), &ctx)
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(result.get_text().contains("hello world"));
}

#[tokio::test]
async fn test_bash_timeout() {
    let registry = ToolRegistry::default_registry();
    let bash = registry.get("Bash").unwrap();
    let ctx = create_test_context("/tmp");

    let result = bash
        .call(json!({"command": "sleep 5", "timeout": 20}), &ctx)
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("timed out"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_bash_timeout_clears_status() {
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(16);

    let result = run_test_command(
        "sleep 5",
        Duration::from_millis(20),
        Some(&event_sender),
        None,
    )
    .await
    .unwrap_err();

    assert!(result.contains("timed out"));
    assert!(
        std::iter::from_fn(|| event_receiver.try_recv().ok()).any(|message| {
            matches!(message, SDKMessage::Status { message } if message.is_empty())
        })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_bash_does_not_block_when_event_channel_is_full() {
    let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(1);
    event_sender
        .try_send(SDKMessage::Status {
            message: "occupied".to_string(),
        })
        .unwrap();

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run_test_command(
            "sleep 5",
            Duration::from_millis(20),
            Some(&event_sender),
            None,
        ),
    )
    .await
    .expect("a full event channel must not block command timeout indefinitely")
    .unwrap_err();

    assert!(result.contains("timed out"));
    assert!(result.contains("event channel remained full"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_final_events_survive_temporary_channel_backpressure() {
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(1);
    event_sender
        .try_send(SDKMessage::Status {
            message: "occupied".to_string(),
        })
        .unwrap();
    let receiver = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        event_receiver.recv().await.unwrap();
        let output = event_receiver.recv().await.unwrap();
        let clear = event_receiver.recv().await.unwrap();
        (output, clear)
    });

    let output = run_test_command(
        "printf done",
        Duration::from_secs(1),
        Some(&event_sender),
        Some("backpressure"),
    )
    .await
    .unwrap();
    assert_eq!(output.exit_code, 0);

    let (output_event, clear_event) = tokio::time::timeout(Duration::from_secs(1), receiver)
        .await
        .expect("final events should be delivered after capacity becomes available")
        .unwrap();
    assert!(matches!(
        output_event,
        SDKMessage::ToolOutput { tool_use_id, content, .. }
            if tool_use_id == "backpressure" && content == "done"
    ));
    assert!(matches!(clear_event, SDKMessage::Status { message } if message.is_empty()));
}

#[cfg(unix)]
#[tokio::test]
async fn test_bash_streams_long_unicode_status() {
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(16);

    let result = run_test_command(
        "printf '你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好你好'; sleep 2",
        Duration::from_secs(5),
        Some(&event_sender),
        Some("unicode-status"),
    )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0);
    let statuses: Vec<String> = std::iter::from_fn(|| event_receiver.try_recv().ok())
        .filter_map(|message| match message {
            SDKMessage::Status { message } if !message.is_empty() => Some(message),
            _ => None,
        })
        .collect();
    assert!(statuses.iter().any(|message| message.ends_with("...")));
}

#[cfg(unix)]
#[tokio::test]
async fn test_bash_streams_stderr_output() {
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::channel(16);

    let result = run_test_command(
        "printf 'streamed error' >&2",
        Duration::from_secs(5),
        Some(&event_sender),
        Some("stderr-output"),
    )
    .await
    .unwrap();

    assert_eq!(result.exit_code, 0);
    let streamed_output: Vec<String> = std::iter::from_fn(|| event_receiver.try_recv().ok())
        .filter_map(|message| match message {
            SDKMessage::ToolOutput { content, .. } => Some(content),
            _ => None,
        })
        .collect();
    assert!(streamed_output
        .iter()
        .any(|content| content.contains("streamed error")));
}

#[cfg(unix)]
#[tokio::test]
async fn test_command_timeout_kills_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let pid_path = temp.path().join("descendant.pid");
    let command = format!("sleep 30 & echo $! > '{}'; wait", pid_path.display());

    let error = run_test_command(&command, Duration::from_millis(150), None, None)
        .await
        .unwrap_err();
    assert!(error.contains("timed out"));

    let pid: i32 = std::fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut process_exists = true;
    for _ in 0..20 {
        // SAFETY: signal 0 only checks whether the pid still exists.
        process_exists = unsafe { libc::kill(pid, 0) } == 0;
        if !process_exists {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!process_exists, "descendant process {pid} survived timeout");
}

#[cfg(unix)]
#[tokio::test]
async fn test_executor_cancellation_kills_process_group() {
    let temp = tempfile::tempdir().unwrap();
    let pid_path = temp.path().join("executor-descendant.pid");
    let command = format!("sleep 30 & echo $! > '{}'; wait", pid_path.display());
    let message = Message {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "cancelled-bash".to_string(),
            name: "Bash".to_string(),
            input: json!({"command": command, "timeout": 60_000}),
        }],
    };
    let cancellation = CancellationToken::new();
    let context = ToolUseContext::with_abort(
        temp.path().to_string_lossy().to_string(),
        cancellation.clone(),
    );
    let registry = ToolRegistry::default_registry();
    let execution =
        tokio::spawn(async move { execute_tools(&message, &registry, &context, None, None).await });

    for _ in 0..40 {
        if pid_path.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        pid_path.exists(),
        "bash did not start its descendant process"
    );
    cancellation.cancel();
    tokio::time::timeout(Duration::from_secs(3), execution)
        .await
        .expect("executor cancellation should finish after process cleanup")
        .unwrap();

    let pid: i32 = std::fs::read_to_string(&pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let mut process_exists = true;
    for _ in 0..20 {
        // SAFETY: signal 0 only checks whether the pid still exists.
        process_exists = unsafe { libc::kill(pid, 0) } == 0;
        if !process_exists {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        !process_exists,
        "descendant process {pid} survived cancellation"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn test_executor_cancels_while_tool_start_is_backpressured() {
    let message = Message {
        role: MessageRole::Assistant,
        content: vec![ContentBlock::ToolUse {
            id: "blocked-start".to_string(),
            name: "Bash".to_string(),
            input: json!({"command": "sleep 30"}),
        }],
    };
    let cancellation = CancellationToken::new();
    let context = ToolUseContext::with_abort("/tmp".to_string(), cancellation.clone());
    let registry = ToolRegistry::default_registry();
    let (event_sender, _event_receiver) = tokio::sync::mpsc::channel(1);
    event_sender
        .try_send(SDKMessage::Status {
            message: "occupied".to_string(),
        })
        .unwrap();
    let execution = tokio::spawn(async move {
        execute_tools(&message, &registry, &context, None, Some(event_sender)).await
    });

    tokio::time::sleep(Duration::from_millis(25)).await;
    cancellation.cancel();
    let results = tokio::time::timeout(Duration::from_secs(1), execution)
        .await
        .expect("tool-start backpressure must remain cancellable")
        .unwrap();

    assert_eq!(results.len(), 1);
    assert!(results[0]
        .2
        .get_text()
        .contains("Tool aborted before start"));
}

#[cfg(unix)]
#[tokio::test]
async fn test_command_output_reports_capture_truncation() {
    let output = run_test_command("yes x | head -c 210000", Duration::from_secs(5), None, None)
        .await
        .unwrap();

    assert_eq!(output.exit_code, 0);
    assert!(output.stdout.ends_with("\n... (stdout truncated)"));
    assert!(output.stdout.len() <= 200_000);
}

#[tokio::test]
async fn test_bash_exit_code() {
    let registry = ToolRegistry::default_registry();
    let bash = registry.get("Bash").unwrap();
    let ctx = create_test_context("/tmp");

    let result = bash.call(json!({"command": "false"}), &ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("Exit code:"));
}

#[tokio::test]
async fn test_lsp_search_honors_cancellation() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("main.rs"), "fn target() {}\n").unwrap();
    let cancellation = tokio_util::sync::CancellationToken::new();
    cancellation.cancel();
    let ctx = ToolUseContext::with_abort(temp.path().to_string_lossy().to_string(), cancellation);
    let registry = ToolRegistry::default_registry();
    let lsp = registry.get("LSP").unwrap();

    let error = lsp
        .call(
            json!({
                "operation": "findReferences",
                "file_path": "main.rs",
                "line": 0,
                "character": 4
            }),
            &ctx,
        )
        .await
        .unwrap_err();

    assert!(matches!(error, ToolError::ExecutionError(message) if message.contains("aborted")));
}

#[tokio::test]
async fn test_bash_destructive_detection() {
    let registry = ToolRegistry::default_registry();
    let bash = registry.get("Bash").unwrap();
    let ctx = create_test_context("/tmp");

    let result = bash
        .call(json!({"command": "rm -rf /"}), &ctx)
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("destructive"));
}

#[tokio::test]
async fn test_bash_is_read_only() {
    let registry = ToolRegistry::default_registry();
    let bash = registry.get("Bash").unwrap();

    assert!(bash.is_read_only(&json!({"command": "ls -la"})));
    assert!(bash.is_read_only(&json!({"command": "git status"})));
    assert!(!bash.is_read_only(&json!({"command": "rm file.txt"})));
    assert!(!bash.is_read_only(&json!({"command": "cargo build"})));
}

// --- File Read Tool Tests ---

#[tokio::test]
async fn test_read_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "line 1\nline 2\nline 3\n").unwrap();

    let registry = ToolRegistry::default_registry();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = read
        .call(json!({"file_path": file_path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = result.get_text();
    assert!(text.contains("line 1"));
    assert!(text.contains("line 2"));
    assert!(text.contains("line 3"));
    // Should have line numbers
    assert!(text.contains("1\t"));
}

#[tokio::test]
async fn test_read_file_not_found() {
    let registry = ToolRegistry::default_registry();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context("/tmp");

    let result = read
        .call(
            json!({"file_path": "/tmp/nonexistent_file_12345.txt"}),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("not found"));
}

#[tokio::test]
async fn test_read_file_with_offset_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    let content: String = (1..=100).map(|i| format!("line {}\n", i)).collect();
    std::fs::write(&file_path, &content).unwrap();

    let registry = ToolRegistry::default_registry();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = read
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "offset": 10,
                "limit": 5
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = result.get_text();
    assert!(text.contains("line 11"));
    assert!(text.contains("line 15"));
    assert!(!text.contains("line 16"));
}

#[tokio::test]
async fn test_read_directory_error() {
    let registry = ToolRegistry::default_registry();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context("/tmp");

    let result = read.call(json!({"file_path": "/tmp"}), &ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("directory"));
}

#[tokio::test]
async fn test_read_is_read_only() {
    let registry = ToolRegistry::default_registry();
    let read = registry.get("Read").unwrap();
    assert!(read.is_read_only(&json!({})));
    assert!(read.is_concurrency_safe(&json!({})));
}

// --- File Write Tool Tests ---

#[tokio::test]
async fn test_write_new_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("new_file.txt");

    let registry = ToolRegistry::default_registry();
    let write = registry.get("Write").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = write
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "content": "hello world"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(result.get_text().contains("Created"));
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello world");
}

#[tokio::test]
async fn test_write_creates_directories() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("a/b/c/file.txt");

    let registry = ToolRegistry::default_registry();
    let write = registry.get("Write").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = write
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "content": "nested"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(file_path.exists());
}

// --- File Edit Tool Tests ---

#[tokio::test]
async fn test_edit_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("edit_test.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let registry = ToolRegistry::default_registry();
    let edit = registry.get("Edit").unwrap();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    // Must read first for staleness tracking
    read.call(json!({"file_path": file_path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();

    let result = edit
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "old_string": "hello",
                "new_string": "goodbye"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "goodbye world"
    );
}

#[tokio::test]
async fn test_edit_string_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("edit_test2.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let registry = ToolRegistry::default_registry();
    let edit = registry.get("Edit").unwrap();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    read.call(json!({"file_path": file_path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();

    let result = edit
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "old_string": "xyz",
                "new_string": "abc"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("not found"));
}

#[tokio::test]
async fn test_edit_multiple_occurrences() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("edit_multi.txt");
    std::fs::write(&file_path, "foo bar foo baz foo").unwrap();

    let registry = ToolRegistry::default_registry();
    let edit = registry.get("Edit").unwrap();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    read.call(json!({"file_path": file_path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();

    // Without replace_all, should fail for multiple occurrences
    let result = edit
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "qux"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("3 times"));
}

#[tokio::test]
async fn test_edit_replace_all() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("edit_all.txt");
    std::fs::write(&file_path, "foo bar foo baz foo").unwrap();

    let registry = ToolRegistry::default_registry();
    let edit = registry.get("Edit").unwrap();
    let read = registry.get("Read").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    read.call(json!({"file_path": file_path.to_str().unwrap()}), &ctx)
        .await
        .unwrap();

    let result = edit
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "old_string": "foo",
                "new_string": "qux",
                "replace_all": true
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(
        std::fs::read_to_string(&file_path).unwrap(),
        "qux bar qux baz qux"
    );
}

#[tokio::test]
async fn test_edit_same_string_error() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("edit_same.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let registry = ToolRegistry::default_registry();
    let edit = registry.get("Edit").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = edit
        .call(
            json!({
                "file_path": file_path.to_str().unwrap(),
                "old_string": "hello",
                "new_string": "hello"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(result.is_error);
    assert!(result.get_text().contains("different"));
}

// --- Glob Tool Tests ---

#[tokio::test]
async fn test_glob_find_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "").unwrap();
    std::fs::write(dir.path().join("b.rs"), "").unwrap();
    std::fs::write(dir.path().join("c.txt"), "").unwrap();

    let registry = ToolRegistry::default_registry();
    let glob = registry.get("Glob").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = glob.call(json!({"pattern": "*.rs"}), &ctx).await.unwrap();

    assert!(!result.is_error);
    let text = result.get_text();
    assert!(text.contains("a.rs"));
    assert!(text.contains("b.rs"));
    assert!(!text.contains("c.txt"));
}

#[tokio::test]
async fn test_glob_no_matches() {
    let dir = tempfile::tempdir().unwrap();

    let registry = ToolRegistry::default_registry();
    let glob = registry.get("Glob").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = glob.call(json!({"pattern": "*.xyz"}), &ctx).await.unwrap();

    assert!(!result.is_error);
    assert!(result.get_text().contains("No files found"));
}

#[tokio::test]
async fn test_glob_dot_lists_current_directory_without_recursing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("root.rs"), "").unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    std::fs::write(dir.path().join("nested").join("child.rs"), "").unwrap();

    let registry = ToolRegistry::default_registry();
    let glob = registry.get("Glob").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = glob.call(json!({"pattern": "."}), &ctx).await.unwrap();

    assert!(!result.is_error);
    let text = result.get_text();
    assert!(text.contains("root.rs"));
    assert!(!text.contains("child.rs"));
}

#[tokio::test]
async fn test_glob_skips_heavy_directories_before_descending() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("node_modules")).unwrap();
    std::fs::write(dir.path().join("node_modules").join("dep.rs"), "").unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src").join("lib.rs"), "").unwrap();

    let registry = ToolRegistry::default_registry();
    let glob = registry.get("Glob").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = glob
        .call(json!({"pattern": "**/*.rs"}), &ctx)
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = result.get_text();
    assert!(text.contains("lib.rs"));
    assert!(!text.contains("dep.rs"));
}

#[tokio::test]
async fn test_glob_returns_when_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());
    ctx.abort_signal.cancel();
    let registry = ToolRegistry::default_registry();
    let glob = registry.get("Glob").unwrap();

    let result = glob.call(json!({"pattern": "**/*"}), &ctx).await.unwrap();

    assert!(!result.is_error);
    assert!(result.get_text().contains("No files found"));
}

// --- Grep Tool Tests ---

#[tokio::test]
async fn test_grep_search() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("test.txt"),
        "hello world\nfoo bar\nhello again\n",
    )
    .unwrap();

    let registry = ToolRegistry::default_registry();
    let grep = registry.get("Grep").unwrap();
    let ctx = create_test_context(dir.path().to_str().unwrap());

    let result = grep
        .call(
            json!({
                "pattern": "hello",
                "path": dir.path().to_str().unwrap(),
                "output_mode": "content"
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert!(!result.is_error);
    let text = result.get_text();
    assert!(text.contains("hello world") || text.contains("hello"));
}

// --- Task Tool Tests ---

#[tokio::test]
async fn test_task_lifecycle() {
    let registry = ToolRegistry::default_registry();
    let ctx = create_test_context("/tmp");

    // Create a task
    let create = registry.get("TaskCreate").unwrap();
    let result = create
        .call(
            json!({
                "subject": "Test task",
                "description": "A test task"
            }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!result.is_error);
    assert!(result.get_text().contains("Created task"));

    // List tasks
    let list = registry.get("TaskList").unwrap();
    let result = list.call(json!({}), &ctx).await.unwrap();
    assert!(!result.is_error);
    assert!(result.get_text().contains("Test task"));

    // Get task
    let get = registry.get("TaskGet").unwrap();
    let result = get.call(json!({"id": "task_1"}), &ctx).await.unwrap();
    assert!(!result.is_error);
    assert!(result.get_text().contains("Test task"));

    // Update task
    let update = registry.get("TaskUpdate").unwrap();
    let result = update
        .call(json!({"id": "task_1", "status": "completed"}), &ctx)
        .await
        .unwrap();
    assert!(!result.is_error);

    // Verify update
    let result = get.call(json!({"id": "task_1"}), &ctx).await.unwrap();
    assert!(result.get_text().to_lowercase().contains("completed"));
}

#[tokio::test]
async fn test_task_not_found() {
    let registry = ToolRegistry::default_registry();
    let ctx = create_test_context("/tmp");

    let get = registry.get("TaskGet").unwrap();
    let result = get.call(json!({"id": "nonexistent"}), &ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.get_text().contains("not found"));
}

// --- WebFetch Tool Tests ---

#[tokio::test]
async fn test_webfetch_missing_url() {
    let registry = ToolRegistry::default_registry();
    let webfetch = registry.get("WebFetch").unwrap();
    let ctx = create_test_context("/tmp");

    let result = webfetch.call(json!({}), &ctx).await;
    assert!(result.is_err());
}

// --- Diff Tests ---

#[test]
fn test_unified_diff() {
    let old = "line 1\nline 2\nline 3\n";
    let new = "line 1\nline 2 modified\nline 3\n";
    let diff = open_agent_sdk::tools::diff::unified_diff(old, new, "test.txt");

    assert!(diff.contains("--- a/test.txt"));
    assert!(diff.contains("+++ b/test.txt"));
    assert!(diff.contains("-line 2"));
    assert!(diff.contains("+line 2 modified"));
}

#[test]
fn test_count_changes() {
    let old = "a\nb\nc\n";
    let new = "a\nx\nc\nd\n";
    let (added, removed) = open_agent_sdk::tools::diff::count_changes(old, new);
    assert!(added >= 1);
    assert!(removed >= 1);
}
