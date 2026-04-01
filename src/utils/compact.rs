use crate::types::Message;
use crate::utils::tokens::{estimate_messages_tokens, get_auto_compact_threshold};

const MICRO_COMPACT_THRESHOLD: usize = 50_000; // chars per tool result

/// Check if auto-compaction is needed based on message token count.
pub fn should_auto_compact(messages: &[Message], model: &str) -> bool {
    let estimated = estimate_messages_tokens(messages);
    let threshold = get_auto_compact_threshold(model);
    estimated > threshold
}

/// Micro-compact: truncate large tool results in messages.
pub fn micro_compact_messages(messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .map(|msg| {
            let content = msg
                .content
                .iter()
                .map(|block| match block {
                    crate::types::ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let compacted_content: Vec<crate::types::ToolResultContentBlock> = content
                            .iter()
                            .map(|c| match c {
                                crate::types::ToolResultContentBlock::Text { text } => {
                                    if text.len() > MICRO_COMPACT_THRESHOLD {
                                        let truncated =
                                            format!("{}... (truncated)", &text[..MICRO_COMPACT_THRESHOLD]);
                                        crate::types::ToolResultContentBlock::Text {
                                            text: truncated,
                                        }
                                    } else {
                                        c.clone()
                                    }
                                }
                                _ => c.clone(),
                            })
                            .collect();
                        crate::types::ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: compacted_content,
                            is_error: *is_error,
                        }
                    }
                    _ => block.clone(),
                })
                .collect();
            Message {
                role: msg.role.clone(),
                content,
            }
        })
        .collect()
}

/// Create a compact summary prompt for the LLM to summarize the conversation.
pub fn create_compact_prompt(messages: &[Message]) -> String {
    let message_count = messages.len();
    format!(
        "Please provide a concise summary of the conversation so far ({} messages). \
         Focus on: 1) What the user asked for, 2) What has been accomplished, \
         3) Key decisions made, 4) Current state and any pending work. \
         Keep the summary under 2000 tokens.",
        message_count
    )
}
