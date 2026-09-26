use super::*;
use crate::python_setup::PythonSetupStage;
use crossterm::event::{self, KeyCode, KeyModifiers};

pub(super) fn handle_python_setup_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let Some(setup) = app.python_setup.as_mut() else {
        return AppEventOutcome::Continue;
    };
    setup.error = None;

    if setup.is_busy() {
        if key.code == KeyCode::Esc && setup.stage == PythonSetupStage::Probing {
            app.should_redraw = true;
            return AppEventOutcome::CancelPythonSetup {
                dismiss_when_settled: true,
            };
        }
        app.should_redraw = true;
        return AppEventOutcome::Continue;
    }

    match setup.stage {
        PythonSetupStage::Profile => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                setup.profile = match setup.profile {
                    crate::config::ToolProfile::General => crate::config::ToolProfile::PythonOnly,
                    crate::config::ToolProfile::PythonOnly => crate::config::ToolProfile::General,
                };
            }
            KeyCode::Enter => setup.next(),
            KeyCode::Esc => app.python_setup = None,
            _ => {}
        },
        PythonSetupStage::Target => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                setup.runtime.target = Some(match setup.runtime.target {
                    Some(crate::config::PythonExecutionTarget::Host) => {
                        crate::config::PythonExecutionTarget::Sandbox
                    }
                    _ => crate::config::PythonExecutionTarget::Host,
                });
            }
            KeyCode::Enter => setup.next(),
            KeyCode::Esc => setup.back(),
            _ => {}
        },
        PythonSetupStage::Backend => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                setup.runtime.sandbox.backend = Some(match setup.runtime.sandbox.backend {
                    Some(crate::config::SandboxBackend::Bubblewrap) => {
                        crate::config::SandboxBackend::Podman
                    }
                    _ => crate::config::SandboxBackend::Bubblewrap,
                });
            }
            KeyCode::Enter => setup.next(),
            KeyCode::Esc => setup.back(),
            _ => {}
        },
        PythonSetupStage::PodmanImage => match key.code {
            KeyCode::Enter => setup.next(),
            KeyCode::Esc => setup.back(),
            KeyCode::Backspace => {
                setup.runtime.sandbox.podman_image.pop();
            }
            KeyCode::F(5) => {
                setup.previous_stage = Some(PythonSetupStage::PodmanImage);
                setup.stage = PythonSetupStage::PullConfirm;
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                setup.previous_stage = Some(PythonSetupStage::PodmanImage);
                setup.stage = PythonSetupStage::PullConfirm;
            }
            KeyCode::Char(character) => setup.runtime.sandbox.podman_image.push(character),
            _ => {}
        },
        PythonSetupStage::Network => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                let next = if setup.runtime.sandbox.backend
                    == Some(crate::config::SandboxBackend::Podman)
                {
                    match setup.runtime.sandbox.network {
                        Some(crate::config::NetworkAccess::None) => {
                            crate::config::NetworkAccess::Nonlocal
                        }
                        Some(crate::config::NetworkAccess::Nonlocal) => {
                            crate::config::NetworkAccess::Full
                        }
                        _ => crate::config::NetworkAccess::None,
                    }
                } else {
                    match setup.runtime.sandbox.network {
                        Some(crate::config::NetworkAccess::None) => {
                            crate::config::NetworkAccess::Full
                        }
                        _ => crate::config::NetworkAccess::None,
                    }
                };
                setup.runtime.sandbox.network = Some(next);
                if next == crate::config::NetworkAccess::Nonlocal {
                    setup.runtime.sandbox.package_access = crate::config::PackageAccess::Session;
                    setup.runtime.sandbox.workspace_access =
                        Some(crate::config::AccessMode::ReadWrite);
                    setup.runtime.sandbox.grants.clear();
                    if setup.runtime.sandbox.podman_image.trim().is_empty()
                        || setup.runtime.sandbox.podman_image
                            == "docker.io/library/python:3.13-slim"
                    {
                        setup.runtime.sandbox.podman_image =
                            crate::config::DEFAULT_RETAINED_PODMAN_IMAGE.to_string();
                    }
                } else {
                    setup.runtime.sandbox.package_access = crate::config::PackageAccess::Disabled;
                }
            }
            KeyCode::Enter => setup.next(),
            KeyCode::Esc => setup.back(),
            _ => {}
        },
        PythonSetupStage::WorkspaceAccess => match key.code {
            KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                setup.runtime.sandbox.workspace_access =
                    Some(match setup.runtime.sandbox.workspace_access {
                        Some(crate::config::AccessMode::ReadOnly) => {
                            crate::config::AccessMode::ReadWrite
                        }
                        _ => crate::config::AccessMode::ReadOnly,
                    });
            }
            KeyCode::Enter => setup.next(),
            KeyCode::Esc => setup.back(),
            _ => {}
        },
        PythonSetupStage::Grants => match key.code {
            KeyCode::Up => setup.path_picker.move_selection(-1),
            KeyCode::Down => setup.path_picker.move_selection(1),
            KeyCode::Enter => setup.path_picker.enter_selected(),
            KeyCode::Backspace if !setup.path_picker.typed_path.is_empty() => {
                setup.path_picker.typed_path.pop();
            }
            KeyCode::Backspace => setup.path_picker.parent(),
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                setup.path_picker.toggle_hidden();
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                setup.path_picker.toggle_access();
            }
            KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                setup.toggle_selected_grant_access();
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let path = setup.path_picker.selected_path();
                if let Err(error) = setup.add_grant(path) {
                    setup.error = Some(error);
                }
            }
            KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                match setup.path_picker.resolve_typed_path() {
                    Ok(path) => {
                        if let Err(error) = setup.add_grant(path) {
                            setup.error = Some(error);
                        }
                    }
                    Err(error) => setup.error = Some(error),
                }
            }
            KeyCode::Delete => setup.remove_selected_grant(),
            KeyCode::PageUp => {
                setup.selected_grant = setup.selected_grant.saturating_sub(1);
            }
            KeyCode::PageDown => {
                setup.selected_grant = (setup.selected_grant + 1)
                    .min(setup.runtime.sandbox.grants.len().saturating_sub(1));
            }
            KeyCode::Char('n') if key.modifiers.is_empty() => setup.next(),
            KeyCode::Char(character) if key.modifiers.is_empty() => {
                setup.path_picker.typed_path.push(character);
            }
            KeyCode::Esc => setup.back(),
            _ => {}
        },
        PythonSetupStage::Confirm => match key.code {
            KeyCode::Left | KeyCode::Up => setup.cycle_persistence(-1),
            KeyCode::Right | KeyCode::Down => setup.cycle_persistence(1),
            KeyCode::Enter => {
                if let Err(error) = setup.validate() {
                    setup.error = Some(error);
                } else {
                    let outcome = AppEventOutcome::ApplyPythonPolicy {
                        snapshot: setup.snapshot(),
                        persistence: setup.persistence,
                        expected_revision: setup.expected_revision(),
                    };
                    setup.stage = PythonSetupStage::Applying;
                    app.should_redraw = true;
                    return outcome;
                }
            }
            KeyCode::Esc => setup.back(),
            _ => {}
        },
        PythonSetupStage::PullConfirm => match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                let image = setup.runtime.sandbox.podman_image.clone();
                setup.stage = PythonSetupStage::Pulling;
                app.should_redraw = true;
                return AppEventOutcome::PullPodmanImage(image);
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => setup.back(),
            _ => {}
        },
        PythonSetupStage::Probing | PythonSetupStage::Pulling | PythonSetupStage::Applying => {}
    }
    app.should_redraw = true;
    AppEventOutcome::Continue
}
