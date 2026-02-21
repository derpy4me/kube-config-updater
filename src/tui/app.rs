use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use crossterm::event::KeyEvent;

use crate::config::Config;
use crate::state::ServerRunState;

// ─── Events ──────────────────────────────────────────────────────────────────

pub enum AppEvent {
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,
    FetchComplete {
        server_name: String,
        result: Result<(), String>,
    },
    ProbeComplete {
        server_name: String,
        result: Result<Option<chrono::DateTime<chrono::Utc>>, String>,
    },
    StateFileChanged,
}

// ─── Probe State ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum ProbeState {
    Probing,
    Done(Option<chrono::DateTime<chrono::Utc>>),
    Failed(String),
}

// ─── View State Machine ───────────────────────────────────────────────────────

#[allow(clippy::large_enum_variant)]
pub enum View {
    Dashboard,
    Detail(String),              // server name
    Wizard(WizardState),
    SetupWizard(SetupWizardState),
    CredentialMenu(String),      // server name
    CredentialInput(String),     // server name
    DeleteConfirm(String),       // server name
    Help,
    Error {
        message: String,
        underlying: Box<View>,
    },
}

// ─── Setup Wizard ─────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct SetupWizardState {
    pub step: SetupStep,
    pub output_dir: String,
    pub default_user: String,
    pub default_file_path: String,
    pub default_file_name: String,
    pub error: Option<String>,
}

#[derive(Clone, PartialEq, Default)]
pub enum SetupStep {
    #[default]
    OutputDir,
    DefaultUser,
    DefaultFilePath,
    DefaultFileName,
}

impl SetupStep {
    pub fn index(&self) -> usize {
        match self {
            SetupStep::OutputDir => 0,
            SetupStep::DefaultUser => 1,
            SetupStep::DefaultFilePath => 2,
            SetupStep::DefaultFileName => 3,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SetupStep::OutputDir => "Local output directory",
            SetupStep::DefaultUser => "Default SSH user",
            SetupStep::DefaultFilePath => "Default remote file path",
            SetupStep::DefaultFileName => "Default remote file name",
        }
    }

    pub fn next(&self) -> Option<SetupStep> {
        match self {
            SetupStep::OutputDir => Some(SetupStep::DefaultUser),
            SetupStep::DefaultUser => Some(SetupStep::DefaultFilePath),
            SetupStep::DefaultFilePath => Some(SetupStep::DefaultFileName),
            SetupStep::DefaultFileName => None,
        }
    }

    pub fn prev(&self) -> Option<SetupStep> {
        match self {
            SetupStep::OutputDir => None,
            SetupStep::DefaultUser => Some(SetupStep::OutputDir),
            SetupStep::DefaultFilePath => Some(SetupStep::DefaultUser),
            SetupStep::DefaultFileName => Some(SetupStep::DefaultFilePath),
        }
    }
}

// ─── Wizard ───────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct WizardState {
    pub step: WizardStep,
    pub name: String,
    pub address: String,
    pub user: String,
    pub file_path: String,
    pub file_name: String,
    pub target_cluster_ip: String,
    pub context_name: String,
    pub auth_method: AuthMethod,
    pub password_input: MaskedInput,
    pub identity_file_input: String,
    pub testing: bool,
    pub test_passed: bool,
    pub error: Option<String>,
}

impl WizardState {
    pub fn new() -> Self {
        WizardState {
            step: WizardStep::Name,
            name: String::new(),
            address: String::new(),
            user: String::new(),
            file_path: String::new(),
            file_name: String::new(),
            target_cluster_ip: String::new(),
            context_name: String::new(),
            auth_method: AuthMethod::Password,
            password_input: MaskedInput::new(),
            identity_file_input: String::new(),
            testing: false,
            test_passed: false,
            error: None,
        }
    }
}

#[derive(Clone, PartialEq, Default)]
pub enum WizardStep {
    #[default]
    Name,
    Address,
    User,
    FilePath,
    FileName,
    TargetClusterIp,
    ContextName,
    Auth,
}

impl WizardStep {
    pub fn index(&self) -> usize {
        match self {
            WizardStep::Name => 0,
            WizardStep::Address => 1,
            WizardStep::User => 2,
            WizardStep::FilePath => 3,
            WizardStep::FileName => 4,
            WizardStep::TargetClusterIp => 5,
            WizardStep::ContextName => 6,
            WizardStep::Auth => 7,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            WizardStep::Name => "Name",
            WizardStep::Address => "Address",
            WizardStep::User => "SSH User",
            WizardStep::FilePath => "File Path",
            WizardStep::FileName => "File Name",
            WizardStep::TargetClusterIp => "Target Cluster IP",
            WizardStep::ContextName => "Context Name",
            WizardStep::Auth => "Authentication",
        }
    }

    pub fn next(&self) -> Option<WizardStep> {
        match self {
            WizardStep::Name => Some(WizardStep::Address),
            WizardStep::Address => Some(WizardStep::User),
            WizardStep::User => Some(WizardStep::FilePath),
            WizardStep::FilePath => Some(WizardStep::FileName),
            WizardStep::FileName => Some(WizardStep::TargetClusterIp),
            WizardStep::TargetClusterIp => Some(WizardStep::ContextName),
            WizardStep::ContextName => Some(WizardStep::Auth),
            WizardStep::Auth => None,
        }
    }

    pub fn prev(&self) -> Option<WizardStep> {
        match self {
            WizardStep::Name => None,
            WizardStep::Address => Some(WizardStep::Name),
            WizardStep::User => Some(WizardStep::Address),
            WizardStep::FilePath => Some(WizardStep::User),
            WizardStep::FileName => Some(WizardStep::FilePath),
            WizardStep::TargetClusterIp => Some(WizardStep::FileName),
            WizardStep::ContextName => Some(WizardStep::TargetClusterIp),
            WizardStep::Auth => Some(WizardStep::ContextName),
        }
    }
}

#[derive(Clone, PartialEq, Default)]
pub enum AuthMethod {
    #[default]
    Password,
    IdentityFile,
}

// ─── Masked Input ─────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct MaskedInput {
    pub value: String,
}

impl MaskedInput {
    pub fn new() -> Self { MaskedInput { value: String::new() } }
    pub fn push(&mut self, c: char) { if self.value.len() < 256 { self.value.push(c); } }
    pub fn pop(&mut self) { self.value.pop(); }
    pub fn clear(&mut self) { self.value.clear(); }
    pub fn masked_display(&self) -> String { "*".repeat(self.value.len()) }
}

// ─── Spinner ──────────────────────────────────────────────────────────────────

pub const SPINNER_FRAMES: &[&str] = &["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];

#[derive(Default)]
pub struct SpinnerState {
    pub frame: usize,
}

impl SpinnerState {
    pub fn new() -> Self { SpinnerState { frame: 0 } }
    pub fn tick(&mut self) { self.frame = (self.frame + 1) % SPINNER_FRAMES.len(); }
    pub fn current(&self) -> &str { SPINNER_FRAMES[self.frame] }
}

// ─── App State ────────────────────────────────────────────────────────────────

pub struct AppState {
    pub config: Config,
    pub config_path: PathBuf,
    pub server_states: HashMap<String, ServerRunState>,
    pub cert_cache: HashMap<String, Option<chrono::DateTime<chrono::Utc>>>,
    pub in_progress: HashSet<String>,
    pub view: View,
    pub prior_view: Option<Box<View>>,  // saved when entering Help
    pub dry_run: bool,
    pub table_state: ratatui::widgets::TableState,
    pub spinner: SpinnerState,
    pub flash_rows: HashMap<String, u8>,         // server_name → frames remaining
    pub notification: Option<(String, std::time::Instant)>,
    pub credential_input: MaskedInput,
    pub use_color: bool,
    pub last_state_mtime: Option<std::time::SystemTime>,
    /// Cert expiry captured just before a fetch starts (for delta notification).
    pub pre_fetch_expiry: HashMap<String, Option<chrono::DateTime<chrono::Utc>>>,
    /// Current server cert probe result shown in the detail view.
    pub probe: Option<(String, ProbeState)>,
}

impl AppState {
    pub fn new(config: Config, config_path: PathBuf, server_states: HashMap<String, ServerRunState>, dry_run: bool) -> Self {
        let use_color = std::env::var("NO_COLOR").is_err();
        AppState {
            config,
            config_path,
            server_states,
            cert_cache: HashMap::new(),
            in_progress: HashSet::new(),
            view: View::Dashboard,
            prior_view: None,
            dry_run,
            table_state: ratatui::widgets::TableState::default(),
            spinner: SpinnerState::new(),
            flash_rows: HashMap::new(),
            notification: None,
            credential_input: MaskedInput::new(),
            use_color,
            last_state_mtime: None,
            pre_fetch_expiry: HashMap::new(),
            probe: None,
        }
    }

    /// Reads cert expiry for every server directly from the cached kubeconfig files.
    /// Called on startup, after any fetch, and when the state file changes.
    pub fn refresh_cert_cache(&mut self) {
        for server in &self.config.servers {
            let mut path = PathBuf::from(&self.config.local_output_dir);
            path.push(&server.name);
            let expiry = match crate::kube::check_local_cert_expiry(&path) {
                crate::kube::CertStatus::Valid(exp) | crate::kube::CertStatus::Expired(exp) => Some(exp),
                _ => None,
            };
            self.cert_cache.insert(server.name.clone(), expiry);
        }
    }
}
