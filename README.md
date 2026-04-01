# Open Agent SDK (Rust)

A Rust framework for building autonomous AI agents that run the full agentic loop in-process. This is the Rust implementation of the Open Agent SDK, maintaining feature parity with the [TypeScript](https://github.com/codeany-ai/open-agent-sdk-typescript) and [Go](https://github.com/codeany-ai/open-agent-sdk-go) versions.

## Features

- **Full Agentic Loop** - Streaming API calls, tool execution, and multi-turn conversation management
- **15+ Built-in Tools** - Bash, Read, Write, Edit, Glob, Grep, WebFetch, WebSearch, Tasks, and more
- **MCP Integration** - Connect to Model Context Protocol servers (stdio, SSE, HTTP transports)
- **Permission System** - Fine-grained tool access control with allow/deny rules
- **Hook System** - Pre/post tool execution hooks with pattern matching
- **Cost Tracking** - Per-model token usage and USD cost calculation
- **Context Injection** - Automatic git status, project context, and date injection
- **Extended Thinking** - Support for Claude's extended thinking feature
- **Prompt Caching** - Automatic cache control for system prompts
- **Subagent Support** - Spawn specialized agents for parallel tasks
- **Task Management** - In-memory task tracking with CRUD operations

## Quick Start

Add to your `Cargo.toml`:

```toml
[dependencies]
open-agent-sdk = "0.1.0"
tokio = { version = "1", features = ["full"] }
```

### Streaming Query

```rust
use open_agent_sdk::{Agent, AgentOptions, SDKMessage};

#[tokio::main]
async fn main() {
    let mut agent = Agent::new(AgentOptions::default()).await.unwrap();

    let (mut rx, handle) = agent.query("List files in the current directory").await;

    while let Some(msg) = rx.recv().await {
        match msg {
            SDKMessage::Assistant { message, .. } => {
                let text = open_agent_sdk::types::extract_text(&message);
                if !text.is_empty() {
                    println!("{}", text);
                }
            }
            SDKMessage::ToolResult { tool_name, content, .. } => {
                println!("[{}] {}", tool_name, &content[..content.len().min(200)]);
            }
            SDKMessage::Result { text, cost_usd, .. } => {
                println!("Result: {} (${:.6})", text, cost_usd);
            }
            _ => {}
        }
    }

    handle.await.unwrap();
    agent.close().await;
}
```

### Blocking Prompt

```rust
use open_agent_sdk::{Agent, AgentOptions};

#[tokio::main]
async fn main() {
    let mut agent = Agent::new(AgentOptions::default()).await.unwrap();

    let result = agent.prompt("What files are in this directory?").await.unwrap();
    println!("{}", result.text);
    println!("Cost: ${:.6}", result.cost_usd);

    agent.close().await;
}
```

### Custom Tools

```rust
use async_trait::async_trait;
use open_agent_sdk::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

struct WeatherTool;

#[async_trait]
impl Tool for WeatherTool {
    fn name(&self) -> &str { "GetWeather" }
    fn description(&self) -> &str { "Get weather for a city" }
    fn input_schema(&self) -> ToolInputSchema {
        ToolInputSchema {
            schema_type: "object".to_string(),
            properties: HashMap::from([(
                "city".to_string(),
                json!({"type": "string", "description": "City name"}),
            )]),
            required: vec!["city".to_string()],
            additional_properties: Some(false),
        }
    }
    fn is_read_only(&self, _: &Value) -> bool { true }
    async fn call(&self, input: Value, _ctx: &ToolUseContext) -> Result<ToolResult, ToolError> {
        let city = input.get("city").and_then(|c| c.as_str()).unwrap_or("Unknown");
        Ok(ToolResult::text(format!("Weather in {}: 22°C, sunny", city)))
    }
}

#[tokio::main]
async fn main() {
    let mut agent = Agent::new(AgentOptions {
        custom_tools: vec![Arc::new(WeatherTool)],
        ..Default::default()
    }).await.unwrap();

    let result = agent.prompt("What's the weather in Tokyo?").await.unwrap();
    println!("{}", result.text);
}
```

## Configuration

```rust
let options = AgentOptions {
    model: Some("claude-sonnet-4-6-20250514".to_string()),
    api_key: Some("sk-...".to_string()),
    cwd: Some("/path/to/project".to_string()),
    system_prompt: Some("You are a code reviewer.".to_string()),
    max_turns: Some(10),
    max_budget_usd: Some(1.0),
    permission_mode: Some(PermissionMode::AcceptEdits),
    allowed_tools: Some(vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()]),
    thinking: Some(ThinkingConfig::enabled(10000)),
    ..Default::default()
};
```

## Built-in Tools

| Tool | Description | Read-Only |
|------|-------------|-----------|
| **Bash** | Execute shell commands | No |
| **Read** | Read files with line numbers | Yes |
| **Write** | Create/overwrite files | No |
| **Edit** | String replacement in files | No |
| **Glob** | File pattern matching | Yes |
| **Grep** | Regex search (ripgrep) | Yes |
| **WebFetch** | Fetch URL content | Yes |
| **WebSearch** | Web search (pluggable) | Yes |
| **AskUserQuestion** | Interactive user prompts | No |
| **TaskCreate** | Create tasks | No |
| **TaskGet** | Get task by ID | Yes |
| **TaskList** | List all tasks | Yes |
| **TaskUpdate** | Update task status | No |
| **ToolSearch** | Discover available tools | Yes |

## Architecture

```
┌─────────────────────────────────────┐
│     User Application Code           │
│  (Agent::new, query, prompt)        │
└──────────────┬──────────────────────┘
               │
        ┌──────▼──────────┐
        │  Agent          │
        │ - Messages      │
        │ - Tool Registry │
        │ - MCP Client    │
        │ - Cost Tracker  │
        └──────┬──────────┘
               │
        ┌──────▼──────────────────────┐
        │    Agentic Loop             │
        │ - API Calls (streaming)     │
        │ - Tool Execution            │
        │ - Retry Logic               │
        │ - Auto-Compaction           │
        └──────┬──────────────────────┘
               │
      ┌────────┼────────┐
      │        │        │
   ┌──▼──┐  ┌─▼──┐  ┌──▼────┐
   │ API │  │Tool│  │ MCP   │
   │Client│  │Pool│  │Servers│
   └─────┘  └────┘  └───────┘
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `ANTHROPIC_API_KEY` | API key for Claude |
| `ANTHROPIC_BASE_URL` | Custom API endpoint |
| `ANTHROPIC_MODEL` | Default model ID |
| `CODEANY_API_KEY` | Alternative API key |
| `CODEANY_BASE_URL` | Alternative base URL |
| `API_TIMEOUT_MS` | API timeout (default: 600000) |

## License

MIT
