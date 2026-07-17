use crate::types::{
    CanUseToolFn, ContentBlock, Message, MessageRole, PermissionDecision, SDKMessage, Tool,
    ToolError, ToolResult, ToolResultContentBlock, ToolUseContext,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::registry::ToolRegistry;

const TOOL_CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone)]
struct ToolEventContext {
    sender: mpsc::Sender<SDKMessage>,
    tool_use_id: String,
}

tokio::task_local! {
    static TOOL_EVENT_CONTEXT: ToolEventContext;
}

pub(crate) fn current_tool_event_context() -> Option<(mpsc::Sender<SDKMessage>, String)> {
    TOOL_EVENT_CONTEXT
        .try_with(|context| (context.sender.clone(), context.tool_use_id.clone()))
        .ok()
}

/// Execute a set of tool calls from an assistant message.
/// Concurrent-safe tools run in parallel; others run sequentially.
pub async fn execute_tools(
    message: &Message,
    registry: &ToolRegistry,
    context: &ToolUseContext,
    permission_fn: Option<&CanUseToolFn>,
    event_sender: Option<mpsc::Sender<SDKMessage>>,
) -> Vec<(String, String, ToolResult)> {
    let tool_uses: Vec<(String, String, Value)> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect();

    if tool_uses.is_empty() {
        return Vec::new();
    }

    // Partition into concurrent and sequential
    let mut concurrent_calls = Vec::new();
    let mut sequential_calls = Vec::new();

    for (id, name, input) in &tool_uses {
        if let Some(tool) = registry.get(name) {
            if tool.is_concurrency_safe(input) {
                concurrent_calls.push((id.clone(), name.clone(), input.clone(), tool));
            } else {
                sequential_calls.push((id.clone(), name.clone(), input.clone(), tool));
            }
        } else {
            // Unknown tool
            sequential_calls.push((
                id.clone(),
                name.clone(),
                input.clone(),
                Arc::new(UnknownTool(name.clone())) as Arc<dyn Tool>,
            ));
        }
    }

    let mut results = Vec::new();

    // Run concurrent tools in parallel
    if !concurrent_calls.is_empty() {
        let mut handles = Vec::new();
        for (id, name, input, tool) in concurrent_calls {
            let ctx = context.clone();
            let perm_fn = permission_fn.cloned();
            let tool = tool.clone();
            let sender = event_sender.clone();
            handles.push(tokio::spawn(async move {
                if let Err(message) = send_tool_start(
                    sender.as_ref(),
                    &ctx.abort_signal,
                    SDKMessage::ToolStart {
                        tool_use_id: id.clone(),
                        tool_name: name.clone(),
                        input: input.clone(),
                    },
                )
                .await
                {
                    return (id, name, ToolResult::error(message));
                }
                let input =
                    check_permission(&name, input, perm_fn.as_ref(), &ctx.abort_signal).await;
                match input {
                    Ok(input) => {
                        let result = call_tool_with_events(
                            tool.as_ref(),
                            input,
                            &ctx,
                            sender.clone(),
                            id.clone(),
                        )
                        .await;
                        let tool_result = match result {
                            Ok(r) => r,
                            Err(e) => ToolResult::error(e.to_string()),
                        };
                        (id, name, tool_result)
                    }
                    Err(msg) => (id, name, ToolResult::error(msg)),
                }
            }));
        }

        for handle in handles {
            if let Ok(result) = handle.await {
                results.push(result);
            }
        }
    }

    // Run sequential tools one at a time
    for (id, name, input, tool) in sequential_calls {
        if let Err(message) = send_tool_start(
            event_sender.as_ref(),
            &context.abort_signal,
            SDKMessage::ToolStart {
                tool_use_id: id.clone(),
                tool_name: name.clone(),
                input: input.clone(),
            },
        )
        .await
        {
            results.push((id, name, ToolResult::error(message)));
            continue;
        }
        let input = check_permission(&name, input, permission_fn, &context.abort_signal).await;
        match input {
            Ok(input) => {
                let result = call_tool_with_events(
                    tool.as_ref(),
                    input,
                    context,
                    event_sender.clone(),
                    id.clone(),
                )
                .await;
                let tool_result = match result {
                    Ok(r) => r,
                    Err(e) => ToolResult::error(e.to_string()),
                };
                results.push((id, name, tool_result));
            }
            Err(msg) => {
                results.push((id, name, ToolResult::error(msg)));
            }
        }
    }

    results
}

async fn send_tool_start(
    sender: Option<&mpsc::Sender<SDKMessage>>,
    abort_signal: &tokio_util::sync::CancellationToken,
    message: SDKMessage,
) -> Result<(), String> {
    let Some(sender) = sender else {
        return Ok(());
    };

    tokio::select! {
        biased;
        _ = abort_signal.cancelled() => Err("Tool aborted before start".to_string()),
        result = sender.send(message) => result
            .map_err(|_| "Event receiver dropped before tool start".to_string()),
    }
}

async fn check_permission(
    tool_name: &str,
    input: Value,
    permission_fn: Option<&CanUseToolFn>,
    abort_signal: &tokio_util::sync::CancellationToken,
) -> Result<Value, String> {
    if abort_signal.is_cancelled() {
        return Err("Tool aborted".to_string());
    }

    if let Some(perm_fn) = permission_fn {
        let decision = tokio::select! {
            decision = perm_fn(tool_name, &input) => decision,
            _ = abort_signal.cancelled() => return Err("Tool aborted".to_string()),
        };
        match decision {
            PermissionDecision::Allow => Ok(input),
            PermissionDecision::Deny(msg) => Err(msg),
            PermissionDecision::AllowWithModifiedInput(new_input) => Ok(new_input),
        }
    } else {
        Ok(input)
    }
}

async fn call_tool_with_cancel(
    tool: &dyn Tool,
    input: Value,
    context: &ToolUseContext,
) -> Result<ToolResult, ToolError> {
    let call = tool.call(input, context);
    tokio::pin!(call);
    tokio::select! {
        biased;
        result = &mut call => result,
        _ = context.abort_signal.cancelled() => {
            match tokio::time::timeout(TOOL_CANCEL_GRACE, &mut call).await {
                Ok(result) => result,
                Err(_) => Ok(ToolResult::error("Tool aborted before cleanup completed")),
            }
        },
    }
}

async fn call_tool_with_events(
    tool: &dyn Tool,
    input: Value,
    context: &ToolUseContext,
    event_sender: Option<mpsc::Sender<SDKMessage>>,
    tool_use_id: String,
) -> Result<ToolResult, ToolError> {
    let call = call_tool_with_cancel(tool, input, context);
    match event_sender {
        Some(sender) => {
            TOOL_EVENT_CONTEXT
                .scope(
                    ToolEventContext {
                        sender,
                        tool_use_id,
                    },
                    call,
                )
                .await
        }
        None => call.await,
    }
}

/// Build a user message containing tool results.
pub fn build_tool_results_message(results: &[(String, String, ToolResult)]) -> Message {
    let content: Vec<ContentBlock> = results
        .iter()
        .map(|(id, _name, result)| {
            let content_blocks: Vec<ToolResultContentBlock> = result
                .content
                .iter()
                .map(|c| match c {
                    crate::types::ToolResultContent::Text { text } => {
                        ToolResultContentBlock::Text { text: text.clone() }
                    }
                    crate::types::ToolResultContent::Image { source } => {
                        ToolResultContentBlock::Image {
                            source: crate::types::ImageContentSource {
                                source_type: source.source_type.clone(),
                                media_type: source.media_type.clone(),
                                data: source.data.clone(),
                            },
                        }
                    }
                })
                .collect();

            ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                content: content_blocks,
                is_error: result.is_error,
            }
        })
        .collect();

    Message {
        role: MessageRole::User,
        content,
    }
}

/// Placeholder tool for unknown tool names.
struct UnknownTool(String);

#[async_trait::async_trait]
impl Tool for UnknownTool {
    fn name(&self) -> &str {
        &self.0
    }

    fn description(&self) -> &str {
        "Unknown tool"
    }

    fn input_schema(&self) -> crate::types::ToolInputSchema {
        crate::types::ToolInputSchema::default()
    }

    async fn call(
        &self,
        _input: Value,
        _context: &ToolUseContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::error(format!(
            "Unknown tool: {}. Use ToolSearch to discover available tools.",
            self.0
        )))
    }
}
