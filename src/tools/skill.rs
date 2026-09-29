use super::icons;
use crate::skills::Skill;
use crate::tools::{FunctionDefinition, Tool};
use serde_json::json;

/// The `skill` tool, listing each enabled skill; `None` when none are enabled.
pub fn get_definition(skills: &[Skill]) -> Option<Tool> {
    if skills.is_empty() {
        return None;
    }
    let listing = skills
        .iter()
        .map(|skill| format!("- {}: {}", skill.name, skill.description))
        .collect::<Vec<_>>()
        .join("\n");
    Some(Tool {
        tool_type: "function".to_string(),
        function: FunctionDefinition {
            name: "skill".to_string(),
            description: format!(
                "Load a skill: expert instructions, scripts and references for one kind of task. \
When the task matches a skill below, call this first with its name, then follow the instructions it returns. \
Load a skill once per task; its files are read from the skill's directory.\n\nAvailable skills:\n{listing}"
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "enum": skills.iter().map(|skill| skill.name.clone()).collect::<Vec<_>>(),
                        "description": "The skill to load"
                    },
                    "description": {
                        "type": "string",
                        "description": "Short description of why"
                    },
                    "tool_call_id": {
                        "type": "string",
                        "description": "Unique identifier for this call"
                    }
                },
                "required": ["name", "description", "tool_call_id"]
            }),
        },
    })
}

pub fn get_ui_description(arguments: &serde_json::Value) -> String {
    format!(
        "{} Load skill: {}",
        icons::COMMAND,
        arguments["name"].as_str().unwrap_or("?")
    )
}

pub fn execute(arguments: &serde_json::Value) -> Result<String, String> {
    let name = arguments["name"]
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or("ERROR: name the skill to load.")?;
    let skills = crate::skills::enabled_here();
    let skill = skills.iter().find(|skill| skill.name == name).ok_or_else(|| {
        let available = skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("ERROR: no enabled skill named {name:?}. Enabled skills: {available}")
    })?;
    crate::skills::load_for_model(skill).map_err(|error| format!("ERROR: {error}"))
}
