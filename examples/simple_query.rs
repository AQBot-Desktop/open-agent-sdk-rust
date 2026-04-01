use open_agent_sdk::{Agent, AgentOptions, SDKMessage};

#[tokio::main]
async fn main() {
    let mut agent = Agent::new(AgentOptions::default()).await.unwrap();

    // Streaming query
    let (mut rx, handle) = agent.query("List files in the current directory using the Glob tool").await;

    while let Some(msg) = rx.recv().await {
        match msg {
            SDKMessage::System { message } => {
                println!("[System] {}", message);
            }
            SDKMessage::Assistant { message, .. } => {
                let text = open_agent_sdk::types::extract_text(&message);
                if !text.is_empty() {
                    println!("[Assistant] {}", text);
                }
                // Check for tool uses
                let tool_uses = open_agent_sdk::types::extract_tool_uses(&message);
                for (id, name, input) in &tool_uses {
                    println!("[Tool Call] {} ({})", name, id);
                }
            }
            SDKMessage::ToolResult {
                tool_name, content, ..
            } => {
                println!("[Tool Result: {}] {}", tool_name, &content[..content.len().min(200)]);
            }
            SDKMessage::Result {
                text,
                num_turns,
                cost_usd,
                ..
            } => {
                println!("\n--- Result ---");
                println!("Text: {}", text);
                println!("Turns: {}", num_turns);
                println!("Cost: ${:.6}", cost_usd);
            }
            SDKMessage::Error { message } => {
                eprintln!("[Error] {}", message);
            }
            _ => {}
        }
    }

    handle.await.unwrap();
    agent.close().await;
}
