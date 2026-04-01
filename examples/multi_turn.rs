use open_agent_sdk::{Agent, AgentOptions};

#[tokio::main]
async fn main() {
    let mut agent = Agent::new(AgentOptions {
        max_turns: Some(10),
        ..Default::default()
    })
    .await
    .unwrap();

    // First turn
    println!("--- Turn 1 ---");
    match agent.prompt("What files are in the current directory?").await {
        Ok(result) => println!("Response: {}\n", result.text),
        Err(e) => eprintln!("Error: {}\n", e),
    }

    // Second turn (continues the conversation)
    println!("--- Turn 2 ---");
    match agent.prompt("Now read the Cargo.toml file").await {
        Ok(result) => println!("Response: {}\n", result.text),
        Err(e) => eprintln!("Error: {}\n", e),
    }

    // Third turn
    println!("--- Turn 3 ---");
    match agent
        .prompt("What dependencies does this project have?")
        .await
    {
        Ok(result) => {
            println!("Response: {}", result.text);
            println!("\nTotal messages: {}", agent.get_messages().len());
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    agent.close().await;
}
