use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CommandId {
    Hotkeys,
    Themes,
    InputHistory,
    LoopDetection,
    SystemPrompt,
    ClearUi,
    ClearContext,
    ToggleDebugger,
    Sessions,
    NameSession,
    LatestFiles,
    Models,
    LspServers,
    AgentMode,
    AgentGeneral,
    PythonIsolated,
    PythonNonlocal,
    PythonPermissive,
    DeletePythonRuntime,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum IconId {
    Command,
    Theme,
    Processing,
    Model,
    Trash,
    Debug,
    Search,
    Quit,
}

impl IconId {
    pub const ALL: [Self; 8] = [
        Self::Command,
        Self::Theme,
        Self::Processing,
        Self::Model,
        Self::Trash,
        Self::Debug,
        Self::Search,
        Self::Quit,
    ];

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Command => crate::icons::COMMAND,
            Self::Theme => crate::icons::THEME,
            Self::Processing => crate::icons::PROCESSING,
            Self::Model => crate::icons::MODEL,
            Self::Trash => crate::icons::TRASH,
            Self::Debug => crate::icons::DEBUG,
            Self::Search => crate::icons::SEARCH,
            Self::Quit => crate::icons::QUIT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CommandBehavior {
    Execute,
    OpenPanel,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: CommandId,
    pub base_label: &'static str,
    pub icon: IconId,
    pub behavior: CommandBehavior,
    pub accelerator: Option<char>,
    pub requires_idle: bool,
    /// One-line explanation shown under the label in both palettes.
    pub description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct CommandView {
    pub id: CommandId,
    pub label: String,
    pub icon: IconId,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub behavior: CommandBehavior,
    pub accelerator: Option<char>,
    pub description: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandContext {
    pub loop_mode: String,
    pub agent_mode: String,
    pub agent_mode_locked: bool,
    pub session_name: Option<String>,
    pub has_history: bool,
    pub fully_idle: bool,
}

impl CommandId {
    pub const ALL: [Self; 20] = [
        Self::Hotkeys,
        Self::Themes,
        Self::InputHistory,
        Self::LoopDetection,
        Self::SystemPrompt,
        Self::ClearUi,
        Self::ClearContext,
        Self::ToggleDebugger,
        Self::Sessions,
        Self::NameSession,
        Self::LatestFiles,
        Self::Models,
        Self::LspServers,
        Self::AgentMode,
        Self::AgentGeneral,
        Self::PythonIsolated,
        Self::PythonNonlocal,
        Self::PythonPermissive,
        Self::DeletePythonRuntime,
        Self::Quit,
    ];

    /// Commands that open the Agent Mode dialog, directly or through a preset.
    pub fn opens_agent_mode(self) -> bool {
        matches!(
            self,
            Self::AgentMode
                | Self::AgentGeneral
                | Self::PythonIsolated
                | Self::PythonNonlocal
                | Self::PythonPermissive
        )
    }

    pub fn spec(self) -> &'static CommandSpec {
        COMMAND_SPECS
            .iter()
            .find(|spec| spec.id == self)
            .expect("every command ID has one canonical specification")
    }

    pub fn from_accelerator(accelerator: char) -> Option<Self> {
        let accelerator = accelerator.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|command| command.spec().accelerator == Some(accelerator))
    }

    pub fn view(self, context: &CommandContext) -> CommandView {
        let spec = self.spec();
        let label = match self {
            Self::LoopDetection => format!("Loop Detection: {}", context.loop_mode),
            Self::AgentMode => format!("Agent Mode: {}", context.agent_mode),
            Self::NameSession => context
                .session_name
                .as_deref()
                .map(|name| format!("Rename Session: {name}"))
                .unwrap_or_else(|| "Name Session".to_string()),
            _ => spec.base_label.to_string(),
        };
        let enabled = match self {
            Self::InputHistory => context.has_history,
            command if command.opens_agent_mode() && context.agent_mode_locked => false,
            command if command.spec().requires_idle => context.fully_idle,
            _ => true,
        };
        let disabled_reason = (!enabled).then(|| match self {
            Self::InputHistory => "No input history is available".to_string(),
            Self::ClearContext => {
                "Wait for the active turn before clearing all context".to_string()
            }
            Self::NameSession => "Wait for the active turn before naming the session".to_string(),
            Self::LspServers => "Wait for the active turn before managing LSP servers".to_string(),
            command if command.opens_agent_mode() && context.agent_mode_locked => {
                crate::python_policy::CLI_PYTHON_POLICY_LOCKED_ERROR.to_string()
            }
            command if command.opens_agent_mode() => {
                "Wait for the active turn before configuring Agent Mode".to_string()
            }
            Self::DeletePythonRuntime => {
                "Wait for the active turn before deleting Python packages".to_string()
            }
            _ => "Command is unavailable while work is active".to_string(),
        });
        CommandView {
            id: self,
            label,
            icon: spec.icon,
            enabled,
            disabled_reason,
            behavior: spec.behavior,
            accelerator: spec.accelerator,
            description: match self {
                command if command.opens_agent_mode() && context.agent_mode_locked => {
                    "Locked by a --python-only flag for this process.".to_string()
                }
                _ => spec.description.to_string(),
            },
        }
    }
}

pub const COMMAND_SPECS: [CommandSpec; 20] = [
    CommandSpec {
        id: CommandId::Hotkeys,
        base_label: "Hotkeys",
        icon: IconId::Command,
        behavior: CommandBehavior::OpenPanel,
        accelerator: Some('h'),
        requires_idle: false,
        description: "Keyboard shortcuts for the terminal UI.",
    },
    CommandSpec {
        id: CommandId::Themes,
        base_label: "Themes",
        icon: IconId::Theme,
        behavior: CommandBehavior::OpenPanel,
        accelerator: Some('t'),
        requires_idle: false,
        description: "Pick one of the built-in colour themes.",
    },
    CommandSpec {
        id: CommandId::InputHistory,
        base_label: "Input History",
        icon: IconId::Command,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: false,
        description: "Recall a previous prompt, shared across this project's sessions.",
    },
    CommandSpec {
        id: CommandId::LoopDetection,
        base_label: "Loop Detection",
        icon: IconId::Processing,
        behavior: CommandBehavior::Execute,
        accelerator: None,
        requires_idle: false,
        description: "Cycle checks that stop runaway output. Block length limit is opt-in.",
    },
    CommandSpec {
        id: CommandId::SystemPrompt,
        base_label: "System Prompt",
        icon: IconId::Model,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: false,
        description: "Edit, switch, or save the system prompt template.",
    },
    CommandSpec {
        id: CommandId::ClearUi,
        base_label: "Clear UI (Keep Context)",
        icon: IconId::Trash,
        behavior: CommandBehavior::Execute,
        accelerator: Some('c'),
        requires_idle: false,
        description: "Clear the transcript on screen; the model keeps its context.",
    },
    CommandSpec {
        id: CommandId::ClearContext,
        base_label: "Clear All Context",
        icon: IconId::Trash,
        behavior: CommandBehavior::Confirm,
        accelerator: None,
        requires_idle: true,
        description: "Start a fresh session with an empty context.",
    },
    CommandSpec {
        id: CommandId::ToggleDebugger,
        base_label: "Toggle Debugger",
        icon: IconId::Debug,
        behavior: CommandBehavior::Execute,
        accelerator: Some('d'),
        requires_idle: false,
        description: "Show or hide the debug log pane (F12).",
    },
    CommandSpec {
        id: CommandId::Sessions,
        base_label: "Sessions",
        icon: IconId::Command,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: false,
        description: "Resume, compact (C), rename, or delete saved sessions.",
    },
    CommandSpec {
        id: CommandId::NameSession,
        base_label: "Name Session",
        icon: IconId::Command,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: true,
        description: "Give this session a display name; its ID and folder stay the same.",
    },
    CommandSpec {
        id: CommandId::LatestFiles,
        base_label: "Latest Files",
        icon: IconId::Command,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: false,
        description: "Files cached in context; remove ones you no longer need.",
    },
    CommandSpec {
        id: CommandId::Models,
        base_label: "Models",
        icon: IconId::Model,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: false,
        description: "Switch connection and model; remembered for this directory.",
    },
    CommandSpec {
        id: CommandId::LspServers,
        base_label: "LSP Servers",
        icon: IconId::Search,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: true,
        description: "Language servers used for code intelligence tools.",
    },
    CommandSpec {
        id: CommandId::AgentMode,
        base_label: "Agent Mode",
        icon: IconId::Command,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: true,
        description: "Configure General or Python-only tools step by step.",
    },
    CommandSpec {
        id: CommandId::AgentGeneral,
        base_label: "Agent Mode: General tools",
        icon: IconId::Command,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: true,
        description: "Back to the normal tool set; Python is not exposed.",
    },
    CommandSpec {
        id: CommandId::PythonIsolated,
        base_label: "Python-only: isolated (Podman, no network)",
        icon: IconId::Processing,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: true,
        description: "Only a python tool, in rootless Podman: no network, no installs.",
    },
    CommandSpec {
        id: CommandId::PythonNonlocal,
        base_label: "Python-only: nonlocal packages (retained Podman)",
        icon: IconId::Processing,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: true,
        description: "Python in retained Podman: public HTTP(S) only, lethetic-pkg installs.",
    },
    CommandSpec {
        id: CommandId::PythonPermissive,
        base_label: "Python-only: permissive network (Podman)",
        icon: IconId::Processing,
        behavior: CommandBehavior::OpenPanel,
        accelerator: None,
        requires_idle: true,
        description: "Python in rootless Podman with full host, LAN and Internet access.",
    },
    CommandSpec {
        id: CommandId::DeletePythonRuntime,
        base_label: "Delete Python runtime/packages",
        icon: IconId::Trash,
        behavior: CommandBehavior::Confirm,
        accelerator: None,
        requires_idle: true,
        description: "Delete this chat's retained package layer; source is kept.",
    },
    CommandSpec {
        id: CommandId::Quit,
        base_label: "Quit",
        icon: IconId::Quit,
        behavior: CommandBehavior::Confirm,
        accelerator: None,
        requires_idle: false,
        description: "Exit Lethetic after confirmation.",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn command_ids_are_unique_stable_and_name_session_follows_sessions() {
        let serialized = CommandId::ALL
            .iter()
            .map(|id| serde_json::to_string(id).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            serialized.len(),
            serialized.iter().collect::<HashSet<_>>().len()
        );
        assert_eq!(serialized[8], "\"sessions\"");
        assert_eq!(serialized[9], "\"name-session\"");
        assert_eq!(CommandId::ALL.len(), COMMAND_SPECS.len());
        let mut accelerators = HashSet::new();
        for id in CommandId::ALL {
            assert_eq!(id.spec().id, id);
            if let Some(accelerator) = id.spec().accelerator {
                assert!(accelerators.insert(accelerator));
                assert_eq!(CommandId::from_accelerator(accelerator), Some(id));
                assert_eq!(
                    CommandId::from_accelerator(accelerator.to_ascii_uppercase()),
                    Some(id)
                );
            }
        }
    }

    #[test]
    fn browser_palette_accelerators_map_to_the_canonical_commands() {
        for (accelerator, command) in [
            ('h', CommandId::Hotkeys),
            ('t', CommandId::Themes),
            ('c', CommandId::ClearUi),
            ('d', CommandId::ToggleDebugger),
        ] {
            assert_eq!(CommandId::from_accelerator(accelerator), Some(command));
            assert_eq!(command.spec().accelerator, Some(accelerator));
        }
    }

    #[test]
    fn dynamic_views_report_names_modes_and_disabled_reasons() {
        let context = CommandContext {
            loop_mode: "NGram".to_string(),
            agent_mode: "PythonOnly".to_string(),
            agent_mode_locked: false,
            session_name: Some("Demo".to_string()),
            has_history: false,
            fully_idle: false,
        };
        assert_eq!(
            CommandId::LoopDetection.view(&context).label,
            "Loop Detection: NGram"
        );
        assert_eq!(
            CommandId::AgentMode.view(&context).label,
            "Agent Mode: PythonOnly"
        );
        assert_eq!(
            CommandId::NameSession.view(&context).label,
            "Rename Session: Demo"
        );
        assert!(!CommandId::NameSession.view(&context).enabled);
        assert!(!CommandId::ClearContext.view(&context).enabled);
        assert!(
            CommandId::ClearContext
                .view(&context)
                .disabled_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("clearing all context"))
        );
        assert!(
            CommandId::NameSession
                .view(&context)
                .disabled_reason
                .is_some()
        );
        assert!(!CommandId::InputHistory.view(&context).enabled);
        assert!(CommandId::Hotkeys.view(&context).enabled);
        let mut locked = context.clone();
        locked.fully_idle = true;
        locked.agent_mode_locked = true;
        let agent_mode = CommandId::AgentMode.view(&locked);
        assert!(!agent_mode.enabled);
        assert_eq!(
            agent_mode.disabled_reason.as_deref(),
            Some(crate::python_policy::CLI_PYTHON_POLICY_LOCKED_ERROR)
        );
        for command in [
            CommandId::ClearContext,
            CommandId::DeletePythonRuntime,
            CommandId::Quit,
        ] {
            assert_eq!(command.spec().behavior, CommandBehavior::Confirm);
        }
    }
}
