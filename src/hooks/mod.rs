use serde_json::Value;
use std::sync::Arc;

/// Hook event types.
#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostSampling,
    Stop,
}

/// Input passed to a hook handler.
#[derive(Debug, Clone)]
pub struct HookInput {
    pub event: HookEvent,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub tool_output: Option<String>,
}

/// Output from a hook handler.
#[derive(Debug, Clone, Default)]
pub struct HookOutput {
    /// If true, block the tool execution.
    pub blocked: bool,
    /// Optional message to return instead.
    pub message: Option<String>,
}

/// Hook function type.
pub type HookFn =
    Arc<dyn Fn(HookInput) -> futures::future::BoxFuture<'static, HookOutput> + Send + Sync>;

/// A hook rule with a matcher pattern and handler.
pub struct HookRule {
    pub matcher: String,
    pub handler: HookFn,
}

/// Hook configuration for the agent.
pub struct HookConfig {
    pub pre_tool_use: Vec<HookRule>,
    pub post_tool_use: Vec<HookRule>,
    pub post_sampling: Vec<HookRule>,
    pub stop: Vec<HookRule>,
}

impl Default for HookConfig {
    fn default() -> Self {
        Self {
            pre_tool_use: Vec::new(),
            post_tool_use: Vec::new(),
            post_sampling: Vec::new(),
            stop: Vec::new(),
        }
    }
}

impl HookConfig {
    /// Run pre-tool-use hooks for a given tool name.
    pub async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
    ) -> Option<HookOutput> {
        for rule in &self.pre_tool_use {
            if matches_tool(&rule.matcher, tool_name) {
                let input = HookInput {
                    event: HookEvent::PreToolUse,
                    tool_name: Some(tool_name.to_string()),
                    tool_input: Some(tool_input.clone()),
                    tool_output: None,
                };
                let output = (rule.handler)(input).await;
                if output.blocked {
                    return Some(output);
                }
            }
        }
        None
    }

    /// Run post-tool-use hooks for a given tool name.
    pub async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_input: &Value,
        tool_output: &str,
    ) {
        for rule in &self.post_tool_use {
            if matches_tool(&rule.matcher, tool_name) {
                let input = HookInput {
                    event: HookEvent::PostToolUse,
                    tool_name: Some(tool_name.to_string()),
                    tool_input: Some(tool_input.clone()),
                    tool_output: Some(tool_output.to_string()),
                };
                (rule.handler)(input).await;
            }
        }
    }

    /// Run stop hooks.
    pub async fn run_stop(&self) {
        for rule in &self.stop {
            let input = HookInput {
                event: HookEvent::Stop,
                tool_name: None,
                tool_input: None,
                tool_output: None,
            };
            (rule.handler)(input).await;
        }
    }
}

/// Check if a tool name matches a hook matcher pattern.
fn matches_tool(matcher: &str, tool_name: &str) -> bool {
    if matcher == "*" || matcher.is_empty() {
        return true;
    }

    // Support pipe-separated patterns: "Bash|Edit|Write"
    if matcher.contains('|') {
        return matcher.split('|').any(|p| matches_tool(p.trim(), tool_name));
    }

    // Support prefix matching: "mcp__*"
    if matcher.ends_with('*') {
        let prefix = &matcher[..matcher.len() - 1];
        return tool_name.starts_with(prefix);
    }

    // Exact match
    matcher == tool_name
}
