use async_trait::async_trait;
use open_agent_sdk::{Agent, AgentOptions, Tool, ToolError, ToolInputSchema, ToolResult, ToolUseContext};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// A custom tool that returns the current weather.
struct GetWeatherTool;

#[async_trait]
impl Tool for GetWeatherTool {
    fn name(&self) -> &str {
        "GetWeather"
    }

    fn description(&self) -> &str {
        "Get the current weather for a city."
    }

    fn input_schema(&self) -> ToolInputSchema {
        ToolInputSchema {
            schema_type: "object".to_string(),
            properties: HashMap::from([(
                "city".to_string(),
                json!({
                    "type": "string",
                    "description": "The city name"
                }),
            )]),
            required: vec!["city".to_string()],
            additional_properties: Some(false),
        }
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn call(&self, input: Value, _context: &ToolUseContext) -> Result<ToolResult, ToolError> {
        let city = input
            .get("city")
            .and_then(|c| c.as_str())
            .unwrap_or("Unknown");

        Ok(ToolResult::text(format!(
            "Weather in {}: 22°C, partly cloudy, humidity 65%",
            city
        )))
    }
}

/// A custom calculator tool.
struct CalculatorTool;

#[async_trait]
impl Tool for CalculatorTool {
    fn name(&self) -> &str {
        "Calculator"
    }

    fn description(&self) -> &str {
        "Perform basic arithmetic calculations."
    }

    fn input_schema(&self) -> ToolInputSchema {
        ToolInputSchema {
            schema_type: "object".to_string(),
            properties: HashMap::from([(
                "expression".to_string(),
                json!({
                    "type": "string",
                    "description": "The arithmetic expression to evaluate (e.g., '2 + 3 * 4')"
                }),
            )]),
            required: vec!["expression".to_string()],
            additional_properties: Some(false),
        }
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn call(&self, input: Value, _context: &ToolUseContext) -> Result<ToolResult, ToolError> {
        let expression = input
            .get("expression")
            .and_then(|e| e.as_str())
            .unwrap_or("0");

        // Simple calculator - just return the expression for demo
        Ok(ToolResult::text(format!(
            "Result of '{}': (calculated result would go here)",
            expression
        )))
    }
}

#[tokio::main]
async fn main() {
    let options = AgentOptions {
        custom_tools: vec![
            Arc::new(GetWeatherTool),
            Arc::new(CalculatorTool),
        ],
        max_turns: Some(5),
        ..Default::default()
    };

    let mut agent = Agent::new(options).await.unwrap();

    // Use the blocking prompt API
    match agent.prompt("What's the weather in Tokyo?").await {
        Ok(result) => {
            println!("Response: {}", result.text);
            println!("Turns: {}", result.num_turns);
            println!("Cost: ${:.6}", result.cost_usd);
        }
        Err(e) => eprintln!("Error: {}", e),
    }

    agent.close().await;
}
