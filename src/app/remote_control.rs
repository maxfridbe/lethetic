//! Command-palette remote control: a setup dialog that mirrors the `--rc`
//! chooser, and a popup with the controller URL once the listener is up.

use super::*;
use crate::remote_control::{BindChoice, DEFAULT_RC_PORT, bind_choices, normalize_rc_target};
use crossterm::event::{self, KeyCode, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RcSetupStage {
    Address,
    Port,
    Auth,
    Files,
    Confirm,
}

pub struct RcSetupState {
    pub stage: RcSetupStage,
    pub choices: Vec<BindChoice>,
    /// Index into `choices`; `choices.len()` is the custom-host row.
    pub selected: usize,
    pub custom_host: String,
    pub port: String,
    /// true = no controller token (`--rc-open`).
    pub open: bool,
    /// true = share the launch directory read-only (`--rc-files`).
    pub files: bool,
    pub error: Option<String>,
}

impl RcSetupState {
    pub fn new() -> Self {
        Self {
            stage: RcSetupStage::Address,
            choices: bind_choices(),
            selected: 0,
            custom_host: String::new(),
            port: DEFAULT_RC_PORT.to_string(),
            open: false,
            files: false,
            error: None,
        }
    }

    pub fn host(&self) -> String {
        self.choices
            .get(self.selected)
            .map(|choice| choice.host.clone())
            .unwrap_or_else(|| self.custom_host.trim().to_string())
    }

    pub fn target(&self) -> String {
        normalize_rc_target(&format!("{}:{}", self.host(), self.port.trim()))
    }

    /// Equivalent launch flags, shown on the confirmation screen.
    pub fn flags(&self) -> String {
        let mut flags = format!("--rc {}", self.target());
        if self.open {
            flags.push_str(" --rc-open");
        }
        if self.files {
            flags.push_str(" --rc-files");
        }
        flags
    }
}

impl Default for RcSetupState {
    fn default() -> Self {
        Self::new()
    }
}

/// Offered after resuming a session that had remote control, when none is
/// running now: start it again with the same settings, edit them, or skip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RcResumeOffer {
    pub target: String,
    pub open: bool,
    pub files: bool,
}

impl RcResumeOffer {
    /// The offer for a resumed session: only when it had remote control, none
    /// is running now, and launch flags did not fix the choice.
    pub fn for_resumed_session(
        app: &App,
        target: Option<String>,
        open: bool,
        files: bool,
    ) -> Option<Self> {
        target
            .filter(|_| app.remote_control_target.is_none() && !app.remote_control_locked)
            .map(|target| Self {
                target,
                open,
                files,
            })
    }
}

impl RcSetupState {
    /// The setup dialog pre-filled with a session's saved settings.
    pub fn from_saved(offer: &RcResumeOffer) -> Self {
        let mut setup = Self::new();
        let (host, port) = split_target(&offer.target);
        match setup.choices.iter().position(|choice| choice.host == host) {
            Some(index) => setup.selected = index,
            None => {
                setup.selected = setup.choices.len();
                setup.custom_host = host;
            }
        }
        if let Some(port) = port {
            setup.port = port;
        }
        setup.open = offer.open;
        setup.files = offer.files;
        setup
    }
}

/// `https://host:port` → (`host`, `port`); IPv6 hosts keep their brackets
/// stripped, matching the address chooser.
fn split_target(target: &str) -> (String, Option<String>) {
    let rest = target
        .strip_prefix("https://")
        .unwrap_or(target)
        .trim_end_matches('/');
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() => {
            (host, Some(port.to_string()))
        }
        _ => (rest, None),
    };
    (
        host.trim_start_matches('[').trim_end_matches(']').to_string(),
        port,
    )
}

pub(super) fn handle_rc_resume_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let Some(offer) = app.rc_resume_offer.clone() else {
        return AppEventOutcome::Continue;
    };
    app.should_redraw = true;
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
            app.rc_resume_offer = None;
            app.stop_reason = "Starting remote control…".to_string();
            return AppEventOutcome::StartRemoteControl {
                target: offer.target,
                open: offer.open,
                files: offer.files,
            };
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            app.rc_resume_offer = None;
            app.rc_setup = Some(RcSetupState::from_saved(&offer));
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
            app.rc_resume_offer = None;
            app.stop_reason = "Remote control not restarted".to_string();
        }
        _ => {}
    }
    AppEventOutcome::Continue
}

/// Shown after the listener starts; holds the private controller URL.
pub struct RcInfoState {
    pub lines: Vec<String>,
    pub url: String,
}

pub(super) fn handle_rc_setup_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let Some(setup) = app.rc_setup.as_mut() else {
        return AppEventOutcome::Continue;
    };
    app.should_redraw = true;
    setup.error = None;
    let custom_row = setup.choices.len();
    match (setup.stage, key.code) {
        (RcSetupStage::Address, KeyCode::Esc) => app.rc_setup = None,
        (_, KeyCode::Esc) => {
            setup.stage = match setup.stage {
                RcSetupStage::Port => RcSetupStage::Address,
                RcSetupStage::Auth => RcSetupStage::Port,
                RcSetupStage::Files => RcSetupStage::Auth,
                _ => RcSetupStage::Files,
            };
        }
        (RcSetupStage::Address, KeyCode::Up) => {
            setup.selected = setup.selected.checked_sub(1).unwrap_or(custom_row);
        }
        (RcSetupStage::Address, KeyCode::Down) => {
            setup.selected = if setup.selected >= custom_row {
                0
            } else {
                setup.selected + 1
            };
        }
        (RcSetupStage::Address, KeyCode::Backspace) if setup.selected == custom_row => {
            setup.custom_host.pop();
        }
        (RcSetupStage::Address, KeyCode::Char(character))
            if setup.selected == custom_row && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            setup.custom_host.push(character);
        }
        (RcSetupStage::Address, KeyCode::Enter) => {
            if setup.host().is_empty() {
                setup.error = Some("Type a host name or IP address".to_string());
            } else {
                setup.stage = RcSetupStage::Port;
            }
        }
        (RcSetupStage::Port, KeyCode::Backspace) => {
            setup.port.pop();
        }
        (RcSetupStage::Port, KeyCode::Char(digit)) if digit.is_ascii_digit() => {
            if setup.port.len() < 5 {
                setup.port.push(digit);
            }
        }
        (RcSetupStage::Port, KeyCode::Enter) => match setup.port.parse::<u16>() {
            Ok(port) if port != 0 => setup.stage = RcSetupStage::Auth,
            _ => setup.error = Some("Port must be 1–65535".to_string()),
        },
        (RcSetupStage::Auth, KeyCode::Up | KeyCode::Down) => setup.open = !setup.open,
        (RcSetupStage::Auth, KeyCode::Enter) => setup.stage = RcSetupStage::Files,
        (RcSetupStage::Files, KeyCode::Up | KeyCode::Down) => setup.files = !setup.files,
        (RcSetupStage::Files, KeyCode::Enter) => setup.stage = RcSetupStage::Confirm,
        (RcSetupStage::Confirm, KeyCode::Enter) => {
            let outcome = AppEventOutcome::StartRemoteControl {
                target: setup.target(),
                open: setup.open,
                files: setup.files,
            };
            app.rc_setup = None;
            app.stop_reason = "Starting remote control…".to_string();
            return outcome;
        }
        _ => {}
    }
    AppEventOutcome::Continue
}

pub(super) fn handle_rc_info_key(app: &mut App, key: event::KeyEvent) -> AppEventOutcome {
    let Some(info) = app.rc_info.as_ref() else {
        return AppEventOutcome::Continue;
    };
    app.should_redraw = true;
    match key.code {
        KeyCode::Char('c') | KeyCode::Char('C') => {
            let url = info.url.clone();
            tokio::spawn(async move {
                let _ = tokio::process::Command::new("wl-copy")
                    .arg(url)
                    .status()
                    .await;
            });
            app.stop_reason = "Controller URL copied to the clipboard".to_string();
        }
        KeyCode::Enter | KeyCode::Esc => app.rc_info = None,
        _ => {}
    }
    AppEventOutcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_host_and_flags_build_the_exact_target() {
        let mut setup = RcSetupState::new();
        setup.selected = setup.choices.len();
        setup.custom_host = "brainiac".to_string();
        setup.port = "9443".to_string();
        setup.open = true;
        setup.files = true;
        assert_eq!(setup.target(), "https://brainiac:9443");
        assert_eq!(
            setup.flags(),
            "--rc https://brainiac:9443 --rc-open --rc-files"
        );
        setup.selected = 0;
        assert_eq!(setup.host(), "127.0.0.1");
    }

    #[test]
    fn saved_settings_prefill_the_setup_dialog() {
        let offer = RcResumeOffer {
            target: "https://brainiac:9443".to_string(),
            open: true,
            files: true,
        };
        let setup = RcSetupState::from_saved(&offer);
        assert_eq!(setup.target(), offer.target);
        assert!(setup.open && setup.files);
        assert_eq!(
            split_target("https://[2001:db8::1]:11223"),
            ("2001:db8::1".to_string(), Some("11223".to_string()))
        );
    }

    #[test]
    fn the_offer_appears_only_when_remote_control_is_off_and_unlocked() {
        let mut app = App::new(&crate::config::Config::default());
        let target = || Some("https://brainiac:11223".to_string());
        assert!(RcResumeOffer::for_resumed_session(&app, None, false, false).is_none());
        assert_eq!(
            RcResumeOffer::for_resumed_session(&app, target(), true, false),
            Some(RcResumeOffer {
                target: "https://brainiac:11223".to_string(),
                open: true,
                files: false,
            })
        );
        app.remote_control_target = Some("https://127.0.0.1:11223".to_string());
        assert!(RcResumeOffer::for_resumed_session(&app, target(), false, false).is_none());
        app.remote_control_target = None;
        app.remote_control_locked = true;
        assert!(RcResumeOffer::for_resumed_session(&app, target(), false, false).is_none());
    }

    #[test]
    fn the_resume_offer_starts_edits_or_skips() {
        let offer = RcResumeOffer {
            target: "https://127.0.0.1:11223".to_string(),
            open: false,
            files: true,
        };
        let key = |code| event::KeyEvent::new(code, KeyModifiers::NONE);
        let mut app = App::new(&crate::config::Config::default());

        app.rc_resume_offer = Some(offer.clone());
        let outcome = handle_rc_resume_key(&mut app, key(KeyCode::Enter));
        assert!(matches!(
            outcome,
            AppEventOutcome::StartRemoteControl { ref target, open: false, files: true }
                if target == "https://127.0.0.1:11223"
        ));
        assert!(app.rc_resume_offer.is_none());

        app.rc_resume_offer = Some(offer.clone());
        handle_rc_resume_key(&mut app, key(KeyCode::Char('e')));
        assert_eq!(app.rc_setup.as_ref().unwrap().target(), offer.target);

        app.rc_setup = None;
        app.rc_resume_offer = Some(offer);
        handle_rc_resume_key(&mut app, key(KeyCode::Esc));
        assert!(app.rc_resume_offer.is_none() && app.rc_setup.is_none());
    }
}
