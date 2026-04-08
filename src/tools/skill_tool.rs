use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::skills::SkillRegistry;
use crate::skills::loader::substitute_variables;
use crate::types::{Tool, ToolError, ToolInputSchema, ToolResult, ToolUseContext};

pub struct SkillTool {
    registry: Arc<RwLock<SkillRegistry>>,
}

impl SkillTool {
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> &str {
        "Invoke a skill by name. Skills provide specialized instructions and workflows for specific tasks. Use the skill name from the Available Skills list."
    }

    fn input_schema(&self) -> ToolInputSchema {
        ToolInputSchema {
            schema_type: "object".to_string(),
            properties: HashMap::from([
                (
                    "skill_name".to_string(),
                    json!({
                        "type": "string",
                        "description": "The name of the skill to invoke"
                    }),
                ),
                (
                    "arguments".to_string(),
                    json!({
                        "type": "string",
                        "description": "Arguments to pass to the skill (replaces $ARGUMENTS in skill content)"
                    }),
                ),
            ]),
            required: vec!["skill_name".to_string()],
            additional_properties: Some(false),
        }
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn call(
        &self,
        input: Value,
        _context: &ToolUseContext,
    ) -> Result<ToolResult, ToolError> {
        let skill_name = input
            .get("skill_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput("Missing 'skill_name' field".to_string()))?;

        let arguments = input
            .get("arguments")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let registry = self.registry.read().await;

        let skill = registry.get(skill_name).ok_or_else(|| {
            ToolError::ExecutionError(format!(
                "Skill '{}' not found. Available skills: {}",
                skill_name,
                registry
                    .user_invocable()
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;

        if !registry.all_enabled().iter().any(|s| s.name == skill_name) {
            return Ok(ToolResult::error(format!(
                "Skill '{}' is currently disabled.",
                skill_name
            )));
        }

        let skill_dir = skill
            .path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let content = substitute_variables(&skill.content, arguments, &skill_dir);

        let result = format!(
            "[SKILL INSTRUCTIONS - You MUST follow these instructions precisely]\n\n{}\n\n[END SKILL INSTRUCTIONS]",
            content
        );

        Ok(ToolResult::text(result))
    }
}
