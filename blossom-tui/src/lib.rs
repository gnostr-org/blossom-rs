//! blossom-tui — gitui-inspired Terminal User Interface for Blossom blob
//! storage.
//!
//! Multi-tab keyboard-driven TUI for managing blobs on a Blossom server.
//!
//! # Tabs
//! - **Blobs** — list, navigate, delete blobs
//! - **Upload** — upload a local file
//! - **Status** — fetch and display `/status` JSON
//! - **Keygen** — generate a fresh BIP-340 keypair

pub mod nip19;
pub mod nostr_relay;
pub mod nostr_sign;

use std::{cmp::Reverse, io::Stdout, path::PathBuf, time::Duration};

use blossom_rs::{BlobDescriptor, BlossomClient, BlossomSigner, Signer};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, TableState, Tabs, Wrap},
};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

// ── Constants ────────────────────────────────────────────────────────────────

pub const APP_TITLE: &str = "blossom-tui";
pub const TAB_NAMES: &[&str] = &[
    " Blobs ", " Upload ", " Batch ", " Admin ", " Relay ",
    " NIPs ", " Status ", " Keygen ",
];

pub const NIP_TAB_NAMES: &[&str] = &[
    " NIP-65 ", " NIP-96 ", " NIP-34 ", " NIP-B7 ", " Profile ",
];

pub const COLOR_ACCENT: Color = Color::Cyan;
pub const COLOR_OK: Color = Color::Green;
pub const COLOR_ERR: Color = Color::Red;
pub const COLOR_DIM: Color = Color::DarkGray;
pub const COLOR_SELECTED_BG: Color = Color::Blue;
pub const COLOR_TITLE_BG: Color = Color::Rgb(24, 24, 48); // deep navy

// ── Sort/Filter
// ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortField {
    #[default]
    Date,
    Size,
    Hash,
    ContentType,
}

impl SortField {
    pub fn next(self) -> Self {
        match self {
            Self::Date => Self::Size,
            Self::Size => Self::Hash,
            Self::Hash => Self::ContentType,
            Self::ContentType => Self::Date,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Date => "Date",
            Self::Size => "Size",
            Self::Hash => "Hash",
            Self::ContentType => "Type",
        }
    }
}

// ── Modal ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    /// Prompt for local save path to download selected blob.
    Download { sha256: String },
    /// Prompt for remote URL to mirror onto the server.
    Mirror,
}

// ── Async messages
// ────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum AppMsg {
    BlobsLoaded(Vec<BlobDescriptor>),
    BlobsError(String),
    UploadDone(BlobDescriptor),
    UploadError(String),
    StatusLoaded(serde_json::Value),
    StatusError(String),
    DeleteDone(String),
    DeleteError(String),
    DownloadDone(PathBuf),
    DownloadError(String),
    MirrorDone(BlobDescriptor),
    MirrorError(String),
    BatchItemDone(usize, BlobDescriptor), // (index, descriptor)
    BatchItemError(usize, String),        // (index, error)
    AdminStatsLoaded(serde_json::Value),
    AdminStatsError(String),
    AdminUsersLoaded(serde_json::Value),
    AdminUsersError(String),
    RelayPolicyLoaded(serde_json::Value),
    RelayPolicyError(String),
    Nip96InfoLoaded(serde_json::Value),
    Nip96InfoError(String),
    Nip96FilesLoaded(serde_json::Value),
    Nip96FilesError(String),
    Nip94Published(String), // relay URL
    Nip94PublishError(String),
    Nip34EventReceived(Nip34EventItem),
    Nip34Connected(String), // relay URL
    Nip34Error(String),
    GitDone(String),
    GitError(String),
}

// ── NIP-34 ────────────────────────────────────────────────────────────────────

/// A single NIP-34 event received from a relay.
#[derive(Debug, Clone)]
pub struct Nip34EventItem {
    pub kind: u64,
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub content_preview: String, // first 80 chars of content or d-tag
}

impl Nip34EventItem {
    pub fn kind_name(&self) -> &'static str {
        match self.kind {
            30617 => "RepoAnnounce",
            30618 => "RepoState",
            1617 => "Patch",
            1618 => "PullRequest",
            1619 => "PRUpdate",
            1621 => "Issue",
            1630 => "Status:Open",
            1631 => "Status:Applied",
            1632 => "Status:Closed",
            1633 => "Status:Draft",
            10317 => "GraspList",
            _ => "Unknown",
        }
    }
}

// ── Batch upload
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BatchStatus {
    Pending,
    Running,
    Done(BlobDescriptor),
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct BatchItem {
    pub path: String,
    pub status: BatchStatus,
}

// ── File browser
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRepoKind {
    /// Contains a `.git` file or directory.
    Repo,
    /// No `.git`, but has `HEAD` + `objects/` + `refs/` at root — a bare clone.
    Bare,
}

/// In-progress git operation detected from sentinel files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRepoState {
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
    Bisecting,
}

impl GitRepoState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Merging       => "MERGING",
            Self::Rebasing      => "REBASING",
            Self::CherryPicking => "CHERRY-PICK",
            Self::Reverting     => "REVERTING",
            Self::Bisecting     => "BISECTING",
        }
    }
}

/// Rich git metadata read from `.git/HEAD`, `.git/config`, and sentinel files.
/// Populated cheaply with pure `fs::read_to_string` — no subprocess.
#[derive(Debug, Clone)]
pub struct GitRepoInfo {
    pub kind:        GitRepoKind,
    /// Current branch name, or `"detached:<sha7>"` for detached HEAD.
    pub branch:      Option<String>,
    /// Name of the first remote found in `.git/config` (usually `"origin"`).
    pub remote_name: Option<String>,
    /// URL of `remote_name`.
    pub remote_url:  Option<String>,
    /// Non-`None` when a merge / rebase / cherry-pick / etc. is in progress.
    pub state:       Option<GitRepoState>,
}

#[derive(Debug, Clone)]
pub struct FileBrowserEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// Set when the entry is a directory that is (or contains) a git repo.
    pub git: Option<GitRepoInfo>,
}

impl FileBrowserEntry {
    fn new(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let is_dir = path.is_dir();
        let git = if is_dir {
            detect_git_info(&path)
        } else {
            None
        };
        Self { name, path, is_dir, git }
    }
}

/// Detect git repo kind and read metadata from the git directory.
/// Uses only `fs::read_to_string` / `is_file` / `is_dir` — no subprocess.
pub fn detect_git_info(dir: &std::path::Path) -> Option<GitRepoInfo> {
    let git_dir = if dir.join(".git").is_dir() {
        dir.join(".git")
    } else if dir.join(".git").is_file() {
        // worktree / submodule — parsing the gitfile is complex; mark as Repo
        return Some(GitRepoInfo {
            kind:        GitRepoKind::Repo,
            branch:      None,
            remote_name: None,
            remote_url:  None,
            state:       None,
        });
    } else if dir.join("HEAD").is_file()
        && dir.join("objects").is_dir()
        && dir.join("refs").is_dir()
    {
        dir.to_path_buf() // bare repo — config lives at root
    } else {
        return None;
    };

    let kind = if git_dir == dir {
        GitRepoKind::Bare
    } else {
        GitRepoKind::Repo
    };

    let branch = git_read_branch(&git_dir);
    let (remote_name, remote_url) = git_read_first_remote(&git_dir);
    let state = git_detect_state(&git_dir);

    Some(GitRepoInfo { kind, branch, remote_name, remote_url, state })
}

/// Read current branch from `<git_dir>/HEAD`.
fn git_read_branch(git_dir: &PathBuf) -> Option<String> {
    let raw = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let raw = raw.trim();
    if let Some(branch) = raw.strip_prefix("ref: refs/heads/") {
        Some(branch.to_owned())
    } else if raw.len() >= 7 {
        Some(format!("detached:{}", &raw[..7]))
    } else {
        None
    }
}

/// Parse the first `[remote "…"]` section from `<git_dir>/config`.
fn git_read_first_remote(git_dir: &PathBuf) -> (Option<String>, Option<String>) {
    let config =
        std::fs::read_to_string(git_dir.join("config")).unwrap_or_default();
    let mut name: Option<String> = None;
    let mut url: Option<String> = None;
    let mut in_remote = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with("[remote \"") {
            if let Some(n) = line
                .strip_prefix("[remote \"")
                .and_then(|s| s.strip_suffix("\"]"))
            {
                name = Some(n.to_owned());
                in_remote = true;
            }
        } else if line.starts_with('[') {
            in_remote = false;
        } else if in_remote {
            if let Some(u) = line.strip_prefix("url = ") {
                url = Some(u.to_owned());
                break;
            }
        }
    }
    (name, url)
}

/// Check for sentinel files indicating an in-progress git operation.
fn git_detect_state(git_dir: &PathBuf) -> Option<GitRepoState> {
    if git_dir.join("MERGE_HEAD").is_file() {
        return Some(GitRepoState::Merging);
    }
    if git_dir.join("CHERRY_PICK_HEAD").is_file() {
        return Some(GitRepoState::CherryPicking);
    }
    if git_dir.join("REVERT_HEAD").is_file() {
        return Some(GitRepoState::Reverting);
    }
    if git_dir.join("rebase-merge").is_dir()
        || git_dir.join("rebase-apply").is_dir()
    {
        return Some(GitRepoState::Rebasing);
    }
    if git_dir.join("BISECT_LOG").is_file() {
        return Some(GitRepoState::Bisecting);
    }
    None
}

/// Compat shim — kept so any external callers still compile.
#[deprecated(note = "use detect_git_info instead")]
pub fn detect_git_repo(dir: &std::path::Path) -> Option<GitRepoKind> {
    detect_git_info(dir).map(|i| i.kind)
}

/// Walk `path` and its ancestors until a git root is found.
/// Returns `(root_path, GitRepoInfo)` for the nearest enclosing repo, or
/// `None` if `path` is not inside any git repository.
pub fn find_git_root(
    path: &std::path::Path,
) -> Option<(PathBuf, GitRepoInfo)> {
    let mut dir = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };
    loop {
        if let Some(info) = detect_git_info(&dir) {
            return Some((dir, info));
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return None,
        }
    }
}

/// Git operations available from the file browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAction {
    Status,
    Pull,
    Push,
    Fetch,
    Log,
    Diff,
    Add,
    Commit,
}

/// Run a git sub-command inside `repo_dir` and return combined stdout+stderr.
async fn run_git_command(
    repo_dir: &std::path::Path,
    action: GitAction,
    commit_msg: &str,
) -> Result<String, String> {
    let args: &[&str] = match action {
        GitAction::Status => &["status"],
        GitAction::Pull => &["pull"],
        GitAction::Push => &["push"],
        GitAction::Fetch => &["fetch", "--all"],
        GitAction::Log => &[
            "log", "--oneline", "--graph", "--decorate", "-20",
        ],
        GitAction::Diff => &["diff"],
        GitAction::Add => &["add", "-A"],
        GitAction::Commit => &["commit", "-m", commit_msg],
    };

    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()
        .await
        .map_err(|e| format!("spawn git: {e}"))?;

    let mut combined = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(&stderr);
    }
    if combined.is_empty() {
        combined = "(no output)".into();
    }
    Ok(combined)
}

// ── App state
// ─────────────────────────────────────────────────────────────────

pub struct KeygenResult {
    pub hex_secret: String,
    pub nsec: String,
    pub pubkey: String,
    pub npub: String,
}

pub struct App {
    // Config
    pub server: String,
    pub secret_key: Option<String>,
    pub pubkey: Option<String>,

    // Navigation
    pub tab: usize,
    pub nip_tab: usize, // selected NIP sub-tab (0=NIP-65…4=Profile)

    // Blobs tab
    pub blobs: Vec<BlobDescriptor>,
    pub blobs_table: TableState,
    pub blobs_loading: bool,
    pub blobs_error: Option<String>,
    pub sort_field: SortField,
    pub filter_str: String,
    pub filter_mode: bool,

    // Upload tab
    pub upload_path: String,
    pub upload_loading: bool,
    pub upload_msg: Option<String>,
    pub upload_ok: bool,
    pub input_mode: bool,
    pub publish_nip94: bool,      // toggle: publish NIP-94 after upload
    pub publish_relay: String,    // relay URL for NIP-94 publishing
    pub publish_relay_edit: bool, // editing relay URL field

    // File browser (upload tab)
    pub filebrowser_cwd: PathBuf,
    pub filebrowser_entries: Vec<FileBrowserEntry>,
    pub filebrowser_list: ListState,
    pub filebrowser_active: bool, // true = tree pane has keyboard focus

    // Status tab
    pub status_data: Option<serde_json::Value>,
    pub status_loading: bool,
    pub status_error: Option<String>,

    // Keygen tab
    pub keygen_data: Option<KeygenResult>,
    pub keygen_copied: Option<u8>, // 1=hex secret, 2=nsec, 3=pubkey

    // Batch upload tab
    pub batch_items: Vec<BatchItem>,
    pub batch_input: String,
    pub batch_input_mode: bool,
    pub batch_running: bool,

    // File browser (batch tab) — independent from upload tab browser
    pub batch_filebrowser_cwd: PathBuf,
    pub batch_filebrowser_entries: Vec<FileBrowserEntry>,
    pub batch_filebrowser_list: ListState,
    pub batch_filebrowser_active: bool,

    // Git panel (shared across upload and batch file browsers)
    pub git_mode: bool,            // right pane shows git panel
    pub git_repo_path: PathBuf,    // repo the panel is operating on
    pub git_repo_info: Option<GitRepoInfo>, // metadata for title / hints
    pub git_action_running: bool,
    pub git_output: Vec<String>,   // lines from last git command
    pub git_output_scroll: usize,  // first visible line
    pub git_commit_msg: String,    // staging area for commit message
    pub git_commit_edit: bool,     // typing commit message

    // Admin tab
    pub admin_stats: Option<serde_json::Value>,
    pub admin_stats_loading: bool,
    pub admin_stats_error: Option<String>,
    pub admin_users: Option<serde_json::Value>,
    pub admin_users_loading: bool,
    pub admin_users_error: Option<String>,

    // Relay tab (blossom relay admin)
    pub relay_policy: Option<serde_json::Value>,
    pub relay_policy_loading: bool,
    pub relay_policy_error: Option<String>,

    // NIP-65 relay list (kind:10002)
    pub nip65_relays: Vec<(String, String)>, // (url, marker: read|write|"")
    pub nip65_selected: usize,
    pub nip65_input: String,        // URL being typed
    pub nip65_input_mode: bool,     // editing new relay URL
    pub nip65_marker: String,       // "read", "write", or ""
    pub nip65_marker_idx: usize,    // 0=both,1=read,2=write
    pub nip65_nostr_relay: String,  // relay to publish to
    pub nip65_relay_edit: bool,

    // NIP-B7 tab (Blossom Server List kind:10063)
    pub nipb7_servers: Vec<String>, // server URLs
    pub nipb7_selected: usize,
    pub nipb7_input: String,
    pub nipb7_input_mode: bool,
    pub nipb7_nostr_relay: String,
    pub nipb7_relay_edit: bool,

    // NIP-96 tab
    pub nip96_info: Option<serde_json::Value>,
    pub nip96_info_loading: bool,
    pub nip96_info_error: Option<String>,
    pub nip96_files: Option<serde_json::Value>,
    pub nip96_files_loading: bool,
    pub nip96_files_error: Option<String>,

    // NIP-34 tab
    pub nip34_relay: String,
    pub nip34_relay_edit: bool,
    pub nip34_events: Vec<Nip34EventItem>,
    pub nip34_events_table: TableState,
    pub nip34_connected: bool,
    pub nip34_status: String, // connection status message

    // Profile tab (NIP-01 kind:0)
    pub profile_name: String,
    pub profile_about: String,
    pub profile_picture: String,
    pub profile_nip05: String,
    pub profile_website: String,
    pub profile_lud16: String,
    pub profile_loading: bool,
    pub profile_error: Option<String>,
    pub profile_edit_field: usize, // 0=name,1=about,2=picture,3=nip05,4=website,5=lud16
    pub profile_editing: bool,     // currently typing in a field
    pub profile_nostr_relay: String, // relay to fetch/publish profile
    pub profile_relay_edit: bool,

    // UI state
    pub show_help: bool,
    pub notification: Option<(String, bool)>, // (message, is_error)
    pub modal: Option<Modal>,
    pub modal_input: String,

    // Channel sender for async results
    pub tx: mpsc::UnboundedSender<AppMsg>,
}

impl App {
    pub fn new(
        server: String,
        secret_key: Option<String>,
        tx: mpsc::UnboundedSender<AppMsg>,
    ) -> Self {
        let pubkey = secret_key
            .as_deref()
            .and_then(|k| Signer::from_secret_hex(k).ok().map(|s| s.public_key_hex()));

        let mut blobs_table = TableState::default();
        blobs_table.select(Some(0));

        Self {
            server,
            secret_key,
            pubkey,
            tab: 0,
            nip_tab: 0,
            blobs: Vec::new(),
            blobs_table,
            blobs_loading: false,
            blobs_error: None,
            sort_field: SortField::default(),
            filter_str: String::new(),
            filter_mode: false,
            upload_path: String::new(),
            upload_loading: false,
            upload_msg: None,
            upload_ok: false,
            input_mode: false,
            publish_nip94: false,
            publish_relay: String::new(),
            publish_relay_edit: false,
            filebrowser_cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            filebrowser_entries: Vec::new(),
            filebrowser_list: ListState::default(),
            filebrowser_active: false,
            status_data: None,
            status_loading: false,
            status_error: None,
            keygen_data: None,
            keygen_copied: None,
            batch_items: Vec::new(),
            batch_input: String::new(),
            batch_input_mode: false,
            batch_running: false,
            batch_filebrowser_cwd: std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("/")),
            batch_filebrowser_entries: Vec::new(),
            batch_filebrowser_list: ListState::default(),
            batch_filebrowser_active: false,
            git_mode: false,
            git_repo_path: PathBuf::new(),
            git_repo_info: None,
            git_action_running: false,
            git_output: Vec::new(),
            git_output_scroll: 0,
            git_commit_msg: String::new(),
            git_commit_edit: false,
            admin_stats: None,
            admin_stats_loading: false,
            admin_stats_error: None,
            admin_users: None,
            admin_users_loading: false,
            admin_users_error: None,
            relay_policy: None,
            relay_policy_loading: false,
            relay_policy_error: None,
            nip65_relays: Vec::new(),
            nip65_selected: 0,
            nip65_input: String::new(),
            nip65_input_mode: false,
            nip65_marker: String::new(),
            nip65_marker_idx: 0,
            nip65_nostr_relay: String::new(),
            nip65_relay_edit: false,
            nipb7_servers: Vec::new(),
            nipb7_selected: 0,
            nipb7_input: String::new(),
            nipb7_input_mode: false,
            nipb7_nostr_relay: String::new(),
            nipb7_relay_edit: false,
            nip96_info: None,
            nip96_info_loading: false,
            nip96_info_error: None,
            nip96_files: None,
            nip96_files_loading: false,
            nip96_files_error: None,
            nip34_relay: String::new(),
            nip34_relay_edit: false,
            nip34_events: Vec::new(),
            nip34_events_table: TableState::default(),
            nip34_connected: false,
            nip34_status: "Press 'c' to connect to a NIP-34 relay.".into(),
            profile_name: String::new(),
            profile_about: String::new(),
            profile_picture: String::new(),
            profile_nip05: String::new(),
            profile_website: String::new(),
            profile_lud16: String::new(),
            profile_loading: false,
            profile_error: None,
            profile_edit_field: 0,
            profile_editing: false,
            profile_nostr_relay: String::new(),
            profile_relay_edit: false,
            show_help: false,
            notification: None,
            modal: None,
            modal_input: String::new(),
            tx,
        }
    }

    pub fn apply(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::BlobsLoaded(blobs) => {
                self.blobs_loading = false;
                self.blobs_error = None;
                let sel = if blobs.is_empty() {
                    None
                } else {
                    Some(
                        self.blobs_table
                            .selected()
                            .unwrap_or(0)
                            .min(blobs.len() - 1),
                    )
                };
                self.blobs = blobs;
                self.blobs_table.select(sel);
            }
            AppMsg::BlobsError(e) => {
                self.blobs_loading = false;
                self.blobs_error = Some(e);
            }
            AppMsg::UploadDone(desc) => {
                self.upload_loading = false;
                self.upload_ok = true;
                self.upload_msg = Some(format!(
                    "✓  {}  ({} bytes)",
                    &desc.sha256[..16.min(desc.sha256.len())],
                    desc.size
                ));
                self.notification = Some(("Upload successful!".into(), false));
                self.refresh_blobs();
            }
            AppMsg::UploadError(e) => {
                self.upload_loading = false;
                self.upload_ok = false;
                self.upload_msg = Some(format!("✗  {e}"));
            }
            AppMsg::StatusLoaded(data) => {
                self.status_loading = false;
                self.status_error = None;
                self.status_data = Some(data);
            }
            AppMsg::StatusError(e) => {
                self.status_loading = false;
                self.status_error = Some(e);
            }
            AppMsg::DeleteDone(sha256) => {
                self.blobs.retain(|b| b.sha256 != sha256);
                let sel = if self.blobs.is_empty() {
                    None
                } else {
                    Some(
                        self.blobs_table
                            .selected()
                            .unwrap_or(0)
                            .min(self.blobs.len() - 1),
                    )
                };
                self.blobs_table.select(sel);
                self.notification = Some(("Blob deleted.".into(), false));
            }
            AppMsg::DeleteError(e) => {
                self.notification = Some((format!("Delete failed: {e}"), true));
            }
            AppMsg::DownloadDone(path) => {
                self.notification = Some((format!("Downloaded → {}", path.display()), false));
            }
            AppMsg::DownloadError(e) => {
                self.notification = Some((format!("Download failed: {e}"), true));
            }
            AppMsg::MirrorDone(desc) => {
                self.notification = Some((
                    format!("Mirrored: {}", &desc.sha256[..16.min(desc.sha256.len())]),
                    false,
                ));
                self.refresh_blobs();
            }
            AppMsg::MirrorError(e) => {
                self.notification = Some((format!("Mirror failed: {e}"), true));
            }
            AppMsg::BatchItemDone(idx, desc) => {
                if let Some(item) = self.batch_items.get_mut(idx) {
                    item.status = BatchStatus::Done(desc);
                }
                let all_done = self
                    .batch_items
                    .iter()
                    .all(|i| matches!(i.status, BatchStatus::Done(_) | BatchStatus::Failed(_)));
                if all_done {
                    self.batch_running = false;
                }
            }
            AppMsg::BatchItemError(idx, e) => {
                if let Some(item) = self.batch_items.get_mut(idx) {
                    item.status = BatchStatus::Failed(e);
                }
                let all_done = self
                    .batch_items
                    .iter()
                    .all(|i| matches!(i.status, BatchStatus::Done(_) | BatchStatus::Failed(_)));
                if all_done {
                    self.batch_running = false;
                }
            }
            AppMsg::AdminStatsLoaded(data) => {
                self.admin_stats_loading = false;
                self.admin_stats = Some(data);
                self.admin_stats_error = None;
            }
            AppMsg::AdminStatsError(e) => {
                self.admin_stats_loading = false;
                self.admin_stats_error = Some(e);
            }
            AppMsg::AdminUsersLoaded(data) => {
                self.admin_users_loading = false;
                self.admin_users = Some(data);
                self.admin_users_error = None;
            }
            AppMsg::AdminUsersError(e) => {
                self.admin_users_loading = false;
                self.admin_users_error = Some(e);
            }
            AppMsg::RelayPolicyLoaded(data) => {
                self.relay_policy_loading = false;
                self.relay_policy = Some(data);
                self.relay_policy_error = None;
            }
            AppMsg::RelayPolicyError(e) => {
                self.relay_policy_loading = false;
                self.relay_policy_error = Some(e);
            }
            AppMsg::Nip96InfoLoaded(data) => {
                self.nip96_info_loading = false;
                self.nip96_info = Some(data);
                self.nip96_info_error = None;
            }
            AppMsg::Nip96InfoError(e) => {
                self.nip96_info_loading = false;
                self.nip96_info_error = Some(e);
            }
            AppMsg::Nip96FilesLoaded(data) => {
                self.nip96_files_loading = false;
                self.nip96_files = Some(data);
                self.nip96_files_error = None;
            }
            AppMsg::Nip96FilesError(e) => {
                self.nip96_files_loading = false;
                self.nip96_files_error = Some(e);
            }
            AppMsg::Nip94Published(relay) => {
                self.notification = Some((format!("NIP-94 event published to {relay}"), false));
            }
            AppMsg::Nip94PublishError(e) => {
                self.notification = Some((format!("NIP-94 publish failed: {e}"), true));
            }
            AppMsg::Nip34EventReceived(ev) => {
                // Keep newest events at top; cap at 200
                self.nip34_events.insert(0, ev);
                if self.nip34_events.len() > 200 {
                    self.nip34_events.truncate(200);
                }
            }
            AppMsg::Nip34Connected(url) => {
                self.nip34_connected = true;
                self.nip34_status = format!("Connected to {url} — subscribing to NIP-34 events…");
                self.nip34_events.clear();
            }
            AppMsg::Nip34Error(e) => {
                self.nip34_connected = false;
                self.nip34_status = format!("Error: {e}");
            }
            AppMsg::GitDone(output) => {
                self.git_action_running = false;
                self.git_output = output.lines().map(String::from).collect();
                self.git_output_scroll = 0;
            }
            AppMsg::GitError(e) => {
                self.git_action_running = false;
                self.git_output =
                    format!("error: {e}").lines().map(String::from).collect();
                self.git_output_scroll = 0;
            }
        }
    }

    // ── Git panel ─────────────────────────────────────────────────────────────

    /// Open the git panel for the given repo path.
    pub fn git_open(&mut self, path: PathBuf) {
        self.git_repo_path = path;
        self.git_mode = true;
        self.git_output.clear();
        self.git_output_scroll = 0;
        self.git_commit_msg.clear();
        self.git_commit_edit = false;
        // Show status immediately on open.
        self.run_git_action(GitAction::Status);
    }

    pub fn git_scroll_up(&mut self) {
        self.git_output_scroll =
            self.git_output_scroll.saturating_sub(1);
    }

    pub fn git_scroll_down(&mut self, visible_lines: usize) {
        let max = self
            .git_output
            .len()
            .saturating_sub(visible_lines);
        self.git_output_scroll =
            (self.git_output_scroll + 1).min(max);
    }

    /// Auto-open or close the git panel based on whether `cwd` is inside a
    /// git repository. Called automatically by both file browser load methods.
    ///
    /// - Inside a repo → open panel (if not already open for the same root),
    ///   refresh `git status`, store `GitRepoInfo`.
    /// - Outside any repo → close the panel.
    pub fn update_git_panel_for_cwd(&mut self, cwd: &PathBuf) {
        match find_git_root(cwd) {
            Some((root, info)) => {
                // Only reset output / run status when the root changes.
                let changed = self.git_repo_path != root;
                self.git_repo_path = root;
                self.git_repo_info = Some(info);
                if !self.git_mode {
                    self.git_mode = true;
                    self.git_output.clear();
                    self.git_output_scroll = 0;
                }
                if changed || self.git_output.is_empty() {
                    self.run_git_action(GitAction::Status);
                }
            }
            None => {
                self.git_mode = false;
                self.git_repo_info = None;
            }
        }
    }

    pub fn run_git_action(&mut self, action: GitAction) {
        if self.git_action_running {
            return;
        }
        self.git_action_running = true;
        self.git_output.clear();

        let repo = self.git_repo_path.clone();
        let commit_msg = self.git_commit_msg.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let result =
                run_git_command(&repo, action, &commit_msg).await;
            match result {
                Ok(out) => tx.send(AppMsg::GitDone(out)).ok(),
                Err(e) => tx.send(AppMsg::GitError(e)).ok(),
            };
        });
    }

    pub fn refresh_blobs(&mut self) {
        if self.blobs_loading {
            return;
        }
        self.blobs_loading = true;
        self.blobs_error = None;

        let server = self.server.clone();
        let pubkey = self.pubkey.clone().unwrap_or_default();
        let secret_key = self.secret_key.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let signer = secret_key
                .as_deref()
                .and_then(|k| Signer::from_secret_hex(k).ok())
                .unwrap_or_else(Signer::generate);
            let client = BlossomClient::new(vec![server], signer);
            match client.list(&pubkey).await {
                Ok(blobs) => {
                    tx.send(AppMsg::BlobsLoaded(blobs)).ok();
                }
                Err(e) => {
                    tx.send(AppMsg::BlobsError(e)).ok();
                }
            }
        });
    }

    pub fn start_upload(&mut self) {
        let path_str = self.upload_path.trim().to_string();
        if path_str.is_empty() {
            self.upload_msg = Some("Enter a file path first (press i to edit).".into());
            self.upload_ok = false;
            return;
        }
        if self.secret_key.is_none() {
            self.upload_msg = Some("A secret key (--key / BLOSSOM_SECRET_KEY) is required.".into());
            self.upload_ok = false;
            return;
        }
        if self.upload_loading {
            return;
        }
        self.upload_loading = true;
        self.upload_msg = None;

        let server = self.server.clone();
        let key = self.secret_key.clone().unwrap();
        let path = std::path::PathBuf::from(path_str);
        let tx = self.tx.clone();
        let publish_nip94 = self.publish_nip94;
        let publish_relay = self.publish_relay.trim().to_string();

        tokio::spawn(async move {
            let signer = match Signer::from_secret_hex(&key) {
                Ok(s) => s,
                Err(e) => {
                    tx.send(AppMsg::UploadError(format!("invalid key: {e}")))
                        .ok();
                    return;
                }
            };
            let client = BlossomClient::new(vec![server.clone()], signer.clone());
            let mime = mime_from_path(&path);
            match client.upload_file(&path, &mime).await {
                Ok(desc) => {
                    // Optionally publish NIP-94 kind:1063 event
                    if publish_nip94 && !publish_relay.is_empty() {
                        let event = blossom_rs::nostr_events::build_file_metadata_event(
                            &signer, &desc, &server, &mime,
                        );
                        match blossom_rs::nostr_events::publish_to_relay(&publish_relay, &event)
                            .await
                        {
                            Ok(()) => tx.send(AppMsg::Nip94Published(publish_relay)).ok(),
                            Err(e) => tx.send(AppMsg::Nip94PublishError(e)).ok(),
                        };
                    }
                    tx.send(AppMsg::UploadDone(desc)).ok();
                }
                Err(e) => {
                    tx.send(AppMsg::UploadError(e)).ok();
                }
            }
        });
    }

    // ── File browser methods ──────────────────────────────────────────────────

    /// (Re)load `filebrowser_entries` from `filebrowser_cwd`.
    /// Directories are listed first, then files, both sorted case-insensitively.
    /// Also auto-opens the git panel when the CWD is inside a git repo.
    pub fn filebrowser_load(&mut self) {
        let mut dirs: Vec<FileBrowserEntry> = Vec::new();
        let mut files: Vec<FileBrowserEntry> = Vec::new();

        if let Ok(rd) = std::fs::read_dir(&self.filebrowser_cwd) {
            for entry in rd.flatten() {
                let e = FileBrowserEntry::new(entry.path());
                if e.is_dir {
                    dirs.push(e);
                } else {
                    files.push(e);
                }
            }
        }

        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        self.filebrowser_entries = dirs.into_iter().chain(files).collect();
        // Keep selection in bounds.
        let sel = self
            .filebrowser_list
            .selected()
            .unwrap_or(0)
            .min(self.filebrowser_entries.len().saturating_sub(1));
        self.filebrowser_list.select(if self.filebrowser_entries.is_empty() {
            None
        } else {
            Some(sel)
        });
        self.filebrowser_sync_path();

        // Auto-reveal the git panel when CWD is inside a git repo.
        let cwd = self.filebrowser_cwd.clone();
        self.update_git_panel_for_cwd(&cwd);
    }

    /// Mirror the highlighted entry's path into the File Path field.
    fn filebrowser_sync_path(&mut self) {
        if let Some(idx) = self.filebrowser_list.selected() {
            if let Some(entry) = self.filebrowser_entries.get(idx) {
                self.upload_path =
                    entry.path.to_string_lossy().into_owned();
            }
        }
    }

    pub fn filebrowser_scroll_up(&mut self) {
        let i = self.filebrowser_list.selected().unwrap_or(0);
        if i > 0 {
            self.filebrowser_list.select(Some(i - 1));
            self.filebrowser_sync_path();
        }
    }

    pub fn filebrowser_scroll_down(&mut self) {
        let max = self.filebrowser_entries.len().saturating_sub(1);
        let i = self.filebrowser_list.selected().unwrap_or(0);
        self.filebrowser_list.select(Some((i + 1).min(max)));
        self.filebrowser_sync_path();
    }

    /// Enter a directory or accept a file into `upload_path`.
    pub fn filebrowser_enter(&mut self) {
        let Some(idx) = self.filebrowser_list.selected() else {
            return;
        };
        let Some(entry) = self.filebrowser_entries.get(idx) else {
            return;
        };
        if entry.is_dir {
            self.filebrowser_cwd = entry.path.clone();
            self.filebrowser_list.select(Some(0));
            self.filebrowser_load(); // also calls sync_path
        } else {
            self.upload_path = entry.path.to_string_lossy().into_owned();
            self.filebrowser_active = false;
        }
    }

    /// Navigate to the parent directory.
    pub fn filebrowser_parent(&mut self) {
        if let Some(parent) =
            self.filebrowser_cwd.parent().map(|p| p.to_path_buf())
        {
            self.filebrowser_cwd = parent;
            self.filebrowser_list.select(Some(0));
            self.filebrowser_load(); // also calls sync_path
        }
    }

    /// Activate the file browser, loading entries if empty.
    pub fn filebrowser_activate(&mut self) {
        self.filebrowser_active = true;
        if self.filebrowser_entries.is_empty() {
            self.filebrowser_load();
        } else {
            self.filebrowser_sync_path();
        }
    }

    // ── Batch file browser methods ────────────────────────────────────────────

    pub fn batch_filebrowser_load(&mut self) {
        let mut dirs: Vec<FileBrowserEntry> = Vec::new();
        let mut files: Vec<FileBrowserEntry> = Vec::new();

        if let Ok(rd) = std::fs::read_dir(&self.batch_filebrowser_cwd) {
            for entry in rd.flatten() {
                let e = FileBrowserEntry::new(entry.path());
                if e.is_dir {
                    dirs.push(e);
                } else {
                    files.push(e);
                }
            }
        }

        dirs.sort_by(|a, b| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        });
        files.sort_by(|a, b| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        });

        self.batch_filebrowser_entries =
            dirs.into_iter().chain(files).collect();

        let sel = self
            .batch_filebrowser_list
            .selected()
            .unwrap_or(0)
            .min(self.batch_filebrowser_entries.len().saturating_sub(1));
        self.batch_filebrowser_list
            .select(if self.batch_filebrowser_entries.is_empty() {
                None
            } else {
                Some(sel)
            });
        self.batch_filebrowser_sync_path();

        // Auto-reveal the git panel when CWD is inside a git repo.
        let cwd = self.batch_filebrowser_cwd.clone();
        self.update_git_panel_for_cwd(&cwd);
    }

    fn batch_filebrowser_sync_path(&mut self) {
        if let Some(idx) = self.batch_filebrowser_list.selected() {
            if let Some(entry) = self.batch_filebrowser_entries.get(idx) {
                self.batch_input =
                    entry.path.to_string_lossy().into_owned();
            }
        }
    }

    pub fn batch_filebrowser_scroll_up(&mut self) {
        let i = self.batch_filebrowser_list.selected().unwrap_or(0);
        if i > 0 {
            self.batch_filebrowser_list.select(Some(i - 1));
            self.batch_filebrowser_sync_path();
        }
    }

    pub fn batch_filebrowser_scroll_down(&mut self) {
        let max =
            self.batch_filebrowser_entries.len().saturating_sub(1);
        let i = self.batch_filebrowser_list.selected().unwrap_or(0);
        self.batch_filebrowser_list.select(Some((i + 1).min(max)));
        self.batch_filebrowser_sync_path();
    }

    /// Enter a dir, or append a file to the batch queue.
    pub fn batch_filebrowser_enter(&mut self) {
        let Some(idx) = self.batch_filebrowser_list.selected() else {
            return;
        };
        let Some(entry) = self.batch_filebrowser_entries.get(idx) else {
            return;
        };
        if entry.is_dir {
            self.batch_filebrowser_cwd = entry.path.clone();
            self.batch_filebrowser_list.select(Some(0));
            self.batch_filebrowser_load();
        } else {
            let path = entry.path.to_string_lossy().into_owned();
            self.batch_input = path;
            self.add_batch_path();
        }
    }

    pub fn batch_filebrowser_parent(&mut self) {
        if let Some(parent) = self
            .batch_filebrowser_cwd
            .parent()
            .map(|p| p.to_path_buf())
        {
            self.batch_filebrowser_cwd = parent;
            self.batch_filebrowser_list.select(Some(0));
            self.batch_filebrowser_load();
        }
    }

    pub fn batch_filebrowser_activate(&mut self) {
        self.batch_filebrowser_active = true;
        if self.batch_filebrowser_entries.is_empty() {
            self.batch_filebrowser_load();
        } else {
            self.batch_filebrowser_sync_path();
        }
    }

    pub fn delete_selected(&mut self) {
        if self.secret_key.is_none() {
            self.notification = Some((
                "A secret key (--key / BLOSSOM_SECRET_KEY) is required for delete.".into(),
                true,
            ));
            return;
        }
        let Some(idx) = self.blobs_table.selected() else {
            return;
        };
        let Some(blob) = self.blobs.get(idx) else {
            return;
        };
        let server = self.server.clone();
        let key = self.secret_key.clone().unwrap();
        let sha256 = blob.sha256.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let signer = match Signer::from_secret_hex(&key) {
                Ok(s) => s,
                Err(e) => {
                    tx.send(AppMsg::DeleteError(format!("invalid key: {e}")))
                        .ok();
                    return;
                }
            };
            let client = BlossomClient::new(vec![server], signer);
            match client.delete(&sha256).await {
                Ok(true) => {
                    tx.send(AppMsg::DeleteDone(sha256)).ok();
                }
                Ok(false) => {
                    tx.send(AppMsg::DeleteError("not found".into())).ok();
                }
                Err(e) => {
                    tx.send(AppMsg::DeleteError(e)).ok();
                }
            }
        });
    }

    pub fn refresh_status(&mut self) {
        if self.status_loading {
            return;
        }
        self.status_loading = true;
        self.status_error = None;

        let server = self.server.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let url = format!("{}/status", server.trim_end_matches('/'));
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new());
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<serde_json::Value>().await {
                        Ok(data) => {
                            tx.send(AppMsg::StatusLoaded(data)).ok();
                        }
                        Err(e) => {
                            tx.send(AppMsg::StatusError(format!("parse: {e}"))).ok();
                        }
                    }
                }
                Ok(resp) => {
                    tx.send(AppMsg::StatusError(format!("HTTP {}", resp.status())))
                        .ok();
                }
                Err(e) => {
                    tx.send(AppMsg::StatusError(format!("request: {e}"))).ok();
                }
            }
        });
    }

    pub fn generate_keypair(&mut self) {
        let signer = Signer::generate();
        let hex_secret = signer.secret_key_hex();
        let nsec = nip19::seckey_to_nsec(&hex_secret).unwrap_or_else(|_| "?".into());
        let pubkey = signer.public_key_hex();
        let npub = nip19::pubkey_to_npub(&pubkey).unwrap_or_else(|_| "?".into());
        self.keygen_data = Some(KeygenResult {
            hex_secret,
            nsec,
            pubkey,
            npub,
        });
        self.keygen_copied = None;
    }

    pub fn next_tab(&mut self) {
        self.tab = (self.tab + 1) % TAB_NAMES.len();
        self.on_tab_enter();
    }

    pub fn prev_tab(&mut self) {
        self.tab = self.tab.checked_sub(1).unwrap_or(TAB_NAMES.len() - 1);
        self.on_tab_enter();
    }

    pub fn on_tab_enter(&mut self) {
        match self.tab {
            0 if self.blobs.is_empty() && !self.blobs_loading => self.refresh_blobs(),
            2 if self.status_data.is_none() && !self.status_loading => self.refresh_status(),
            _ => {}
        }
    }

    pub fn scroll_up(&mut self) {
        if self.tab == 0 {
            let i = self
                .blobs_table
                .selected()
                .map(|i| i.saturating_sub(1))
                .unwrap_or(0);
            self.blobs_table.select(Some(i));
        }
    }

    pub fn scroll_down(&mut self) {
        if self.tab == 0 && !self.blobs.is_empty() {
            let max = self.blobs.len() - 1;
            let i = self
                .blobs_table
                .selected()
                .map(|i| (i + 1).min(max))
                .unwrap_or(0);
            self.blobs_table.select(Some(i));
        }
    }

    /// Cycle sort field.
    pub fn cycle_sort(&mut self) {
        self.sort_field = self.sort_field.next();
        self.blobs_table.select(Some(0));
    }

    /// Enter filter mode.
    pub fn enter_filter_mode(&mut self) {
        self.filter_mode = true;
        self.blobs_table.select(Some(0));
    }

    /// Exit filter mode.
    pub fn exit_filter_mode(&mut self) {
        self.filter_mode = false;
    }

    /// Clear the current filter.
    pub fn clear_filter(&mut self) {
        self.filter_str.clear();
        self.filter_mode = false;
        self.blobs_table.select(Some(0));
    }

    /// Open the selected blob's URL in the system default application.
    pub fn open_selected_blob(&mut self) {
        let Some(idx) = self.blobs_table.selected() else {
            return;
        };
        let visible = self.visible_blobs();
        let Some(blob) = visible.get(idx) else {
            return;
        };
        let url = match &blob.url {
            Some(u) => u.clone(),
            None => {
                // Fall back to constructing the URL from server + sha256.
                format!(
                    "{}/{}",
                    self.server.trim_end_matches('/'),
                    blob.sha256
                )
            }
        };
        drop(visible);
        match open::that(&url) {
            Ok(()) => self.notification = Some((format!("Opened: {url}"), false)),
            Err(e) => self.notification = Some((format!("Open failed: {e}"), true)),
        }
    }

    /// Copy the full SHA-256 of the selected blob to the system clipboard.
    pub fn copy_selected_sha256(&mut self) {
        let Some(idx) = self.blobs_table.selected() else {
            return;
        };
        let visible = self.visible_blobs();
        let Some(blob) = visible.get(idx) else {
            return;
        };
        let sha = blob.sha256.clone();
        drop(visible);
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(sha.clone())) {
            Ok(()) => self.notification = Some((format!("Copied SHA256: {sha}"), false)),
            Err(e) => self.notification = Some((format!("Clipboard error: {e}"), true)),
        }
    }

    /// Copy the URL of the selected blob to the system clipboard.
    pub fn copy_selected_url(&mut self) {
        let Some(idx) = self.blobs_table.selected() else {
            return;
        };
        let visible = self.visible_blobs();
        let Some(blob) = visible.get(idx) else {
            return;
        };
        let url = match &blob.url {
            Some(u) => u.clone(),
            None => {
                self.notification = Some(("Selected blob has no URL.".into(), true));
                return;
            }
        };
        drop(visible);
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(url.clone())) {
            Ok(()) => self.notification = Some((format!("Copied URL: {url}"), false)),
            Err(e) => self.notification = Some((format!("Clipboard error: {e}"), true)),
        }
    }

    /// Copy a keygen field to the clipboard.
    /// field: 1=hex secret, 2=nsec, 3=pubkey hex, 4=npub
    pub fn copy_keygen_field(&mut self, field: u8) {
        let Some(kp) = &self.keygen_data else {
            self.notification = Some(("Press g to generate a keypair first.".into(), true));
            return;
        };
        let (label, value) = match field {
            1 => ("Secret (hex)", kp.hex_secret.clone()),
            2 => ("nsec", kp.nsec.clone()),
            3 => ("Public key (hex)", kp.pubkey.clone()),
            4 => ("npub", kp.npub.clone()),
            _ => return,
        };
        match arboard::Clipboard::new().and_then(|mut cb| cb.set_text(value.clone())) {
            Ok(()) => {
                self.keygen_copied = Some(field);
                self.notification = Some((format!("Copied {label}: {value}"), false))
            }
            Err(e) => self.notification = Some((format!("Clipboard error: {e}"), true)),
        }
    }

    /// Return the visible (filtered + sorted) blob list, mirroring draw_blobs_tab logic.
    fn visible_blobs(&self) -> Vec<&BlobDescriptor> {
        let filter_lc = self.filter_str.to_lowercase();
        let mut visible: Vec<&BlobDescriptor> = self
            .blobs
            .iter()
            .filter(|b| {
                if filter_lc.is_empty() {
                    return true;
                }
                b.sha256.to_lowercase().contains(&filter_lc)
                    || b.content_type
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&filter_lc)
            })
            .collect();
        match self.sort_field {
            SortField::Date => visible.sort_by_key(|b| std::cmp::Reverse(b.uploaded.unwrap_or(0))),
            SortField::Size => visible.sort_by_key(|b| std::cmp::Reverse(b.size)),
            SortField::Hash => visible.sort_by(|a, b| a.sha256.cmp(&b.sha256)),
            SortField::ContentType => visible.sort_by(|a, b| {
                a.content_type
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.content_type.as_deref().unwrap_or(""))
            }),
        }
        visible
    }

    /// Open the download path prompt for the selected blob.
    pub fn prompt_download(&mut self) {
        let Some(idx) = self.blobs_table.selected() else {
            return;
        };
        let Some(blob) = self.blobs.get(idx) else {
            return;
        };
        let sha256 = blob.sha256.clone();
        self.modal_input = sha256[..16.min(sha256.len())].to_string();
        self.modal = Some(Modal::Download { sha256 });
    }

    /// Execute the download using the path in `modal_input`.
    pub fn confirm_download(&mut self) {
        let Some(Modal::Download { sha256 }) = self.modal.take() else {
            return;
        };
        let path_str = self.modal_input.trim().to_string();
        self.modal_input.clear();
        if path_str.is_empty() {
            self.notification = Some(("Enter a file path.".into(), true));
            return;
        }
        let server = self.server.clone();
        let secret_key = self.secret_key.clone();
        let tx = self.tx.clone();
        let path = PathBuf::from(&path_str);

        tokio::spawn(async move {
            let signer = secret_key
                .as_deref()
                .and_then(|k| Signer::from_secret_hex(k).ok())
                .unwrap_or_else(Signer::generate);
            let client = BlossomClient::new(vec![server], signer);
            match client.download(&sha256).await {
                Ok(data) => match tokio::fs::write(&path, &data).await {
                    Ok(()) => tx.send(AppMsg::DownloadDone(path)).ok(),
                    Err(e) => tx.send(AppMsg::DownloadError(format!("write: {e}"))).ok(),
                },
                Err(e) => tx.send(AppMsg::DownloadError(e)).ok(),
            };
        });
    }

    /// Open the mirror URL prompt.
    pub fn prompt_mirror(&mut self) {
        if self.secret_key.is_none() {
            self.notification = Some(("A secret key is required to mirror.".into(), true));
            return;
        }
        self.modal_input.clear();
        self.modal = Some(Modal::Mirror);
    }

    /// Execute mirroring using the URL in `modal_input`.
    pub fn confirm_mirror(&mut self) {
        self.modal = None;
        let url = self.modal_input.trim().to_string();
        self.modal_input.clear();
        if url.is_empty() {
            self.notification = Some(("Enter a URL.".into(), true));
            return;
        }
        let server = self.server.clone();
        let secret_key = self.secret_key.clone();
        let tx = self.tx.clone();

        tokio::spawn(async move {
            let signer = secret_key
                .as_deref()
                .and_then(|k| Signer::from_secret_hex(k).ok())
                .unwrap_or_else(Signer::generate);
            let auth_event =
                blossom_rs::auth::build_blossom_auth(&signer, "upload", None, None, "");
            let auth_header = blossom_rs::auth::auth_header_value(&auth_event);
            let endpoint = format!("{}/mirror", server.trim_end_matches('/'));
            let http = reqwest::Client::new();
            match http
                .put(&endpoint)
                .header("Authorization", auth_header)
                .json(&serde_json::json!({"url": url}))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<BlobDescriptor>().await {
                        Ok(desc) => tx.send(AppMsg::MirrorDone(desc)).ok(),
                        Err(e) => tx.send(AppMsg::MirrorError(format!("parse: {e}"))).ok(),
                    }
                }
                Ok(resp) => {
                    let text = resp.text().await.unwrap_or_default();
                    tx.send(AppMsg::MirrorError(format!("server: {text}"))).ok()
                }
                Err(e) => tx.send(AppMsg::MirrorError(format!("request: {e}"))).ok(),
            };
        });
    }

    /// Fetch admin stats and users.
    pub fn refresh_admin(&mut self) {
        let server = self.server.clone();
        let tx = self.tx.clone();

        if !self.admin_stats_loading {
            self.admin_stats_loading = true;
            let server2 = server.clone();
            let tx2 = tx.clone();
            tokio::spawn(async move {
                let url = format!("{}/admin/stats", server2.trim_end_matches('/'));
                match reqwest::get(&url).await {
                    Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                        Ok(v) => tx2.send(AppMsg::AdminStatsLoaded(v)).ok(),
                        Err(e) => tx2
                            .send(AppMsg::AdminStatsError(format!("parse: {e}")))
                            .ok(),
                    },
                    Ok(r) => {
                        let t = r.text().await.unwrap_or_default();
                        tx2.send(AppMsg::AdminStatsError(format!("server: {t}")))
                            .ok()
                    }
                    Err(e) => tx2
                        .send(AppMsg::AdminStatsError(format!("request: {e}")))
                        .ok(),
                };
            });
        }

        if !self.admin_users_loading {
            self.admin_users_loading = true;
            tokio::spawn(async move {
                let url = format!("{}/admin/users", server.trim_end_matches('/'));
                match reqwest::get(&url).await {
                    Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                        Ok(v) => tx.send(AppMsg::AdminUsersLoaded(v)).ok(),
                        Err(e) => tx.send(AppMsg::AdminUsersError(format!("parse: {e}"))).ok(),
                    },
                    Ok(r) => {
                        let t = r.text().await.unwrap_or_default();
                        tx.send(AppMsg::AdminUsersError(format!("server: {t}")))
                            .ok()
                    }
                    Err(e) => tx
                        .send(AppMsg::AdminUsersError(format!("request: {e}")))
                        .ok(),
                };
            });
        }
    }

    /// Fetch relay policy.
    pub fn refresh_relay(&mut self) {
        if self.relay_policy_loading {
            return;
        }
        self.relay_policy_loading = true;
        let server = self.server.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let url = format!("{}/relay/admin/policy", server.trim_end_matches('/'));
            match reqwest::get(&url).await {
                Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                    Ok(v) => tx.send(AppMsg::RelayPolicyLoaded(v)).ok(),
                    Err(e) => tx
                        .send(AppMsg::RelayPolicyError(format!("parse: {e}")))
                        .ok(),
                },
                Ok(r) => {
                    let t = r.text().await.unwrap_or_default();
                    tx.send(AppMsg::RelayPolicyError(format!("server: {t}")))
                        .ok()
                }
                Err(e) => tx
                    .send(AppMsg::RelayPolicyError(format!("request: {e}")))
                    .ok(),
            };
        });
    }

    /// Fetch NIP-96 server info and file list.
    pub fn refresh_nip96(&mut self) {
        if self.nip96_info_loading {
            return;
        }
        self.nip96_info_loading = true;
        self.nip96_files_loading = true;
        let server = self.server.clone();
        let secret_key = self.secret_key.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let base = server.trim_end_matches('/');
            // Fetch /.well-known/nostr/nip96.json
            let info_url = format!("{}/.well-known/nostr/nip96.json", base);
            match reqwest::get(&info_url).await {
                Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                    Ok(v) => tx.send(AppMsg::Nip96InfoLoaded(v)).ok(),
                    Err(e) => tx.send(AppMsg::Nip96InfoError(format!("parse: {e}"))).ok(),
                },
                Ok(r) => tx
                    .send(AppMsg::Nip96InfoError(format!("HTTP {}", r.status())))
                    .ok(),
                Err(e) => tx
                    .send(AppMsg::Nip96InfoError(format!("request: {e}")))
                    .ok(),
            };

            // Fetch /n96?page=1&count=50 (requires auth if server enforces it)
            let files_url = format!("{}/n96?page=1&count=50", base);
            let client = reqwest::Client::new();
            let mut req = client.get(&files_url);
            if let Some(key) = &secret_key {
                if let Ok(signer) = blossom_rs::auth::Signer::from_secret_hex(key) {
                    let auth_event = blossom_rs::auth::build_nip98_auth(&signer, &files_url, "GET");
                    let token = blossom_rs::auth::auth_header_value(&auth_event);
                    req = req.header("Authorization", token);
                }
            }
            match req.send().await {
                Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                    Ok(v) => tx.send(AppMsg::Nip96FilesLoaded(v)).ok(),
                    Err(e) => tx.send(AppMsg::Nip96FilesError(format!("parse: {e}"))).ok(),
                },
                Ok(r) => tx
                    .send(AppMsg::Nip96FilesError(format!("HTTP {}", r.status())))
                    .ok(),
                Err(e) => tx
                    .send(AppMsg::Nip96FilesError(format!("request: {e}")))
                    .ok(),
            };
        });
    }

    /// Connect to a NIP-34 Nostr relay via WebSocket and subscribe to NIP-34
    /// events.
    pub fn connect_nip34_relay(&mut self) {
        let relay = self.nip34_relay.trim().to_string();
        if relay.is_empty() {
            self.nip34_status = "Enter a relay URL first (press 'r' to edit).".into();
            return;
        }
        self.nip34_connected = false;
        self.nip34_status = format!("Connecting to {relay}…");
        let tx = self.tx.clone();
        tokio::spawn(async move {
            use futures_util::{SinkExt, StreamExt};
            let ws_url = relay
                .replace("http://", "ws://")
                .replace("https://", "wss://");
            let conn = tokio_tungstenite::connect_async(&ws_url).await;
            let (mut ws, _) = match conn {
                Ok(pair) => pair,
                Err(e) => {
                    tx.send(AppMsg::Nip34Error(format!("connect failed: {e}")))
                        .ok();
                    return;
                }
            };
            tx.send(AppMsg::Nip34Connected(ws_url.clone())).ok();

            // Send REQ for NIP-34 kinds
            let kinds: Vec<u64> = vec![
                30617, 30618, 1617, 1618, 1619, 1621, 1630, 1631, 1632, 1633, 10317,
            ];
            let req = serde_json::json!([
                "REQ",
                "nip34-sub",
                {"kinds": kinds, "limit": 100}
            ]);
            if ws
                .send(WsMessage::Text(req.to_string().into()))
                .await
                .is_err()
            {
                tx.send(AppMsg::Nip34Error("failed to send REQ".into()))
                    .ok();
                return;
            }

            // Receive events (run for up to 60 seconds then reconnect on next 'c')
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                if std::time::Instant::now() > deadline {
                    tx.send(AppMsg::Nip34Error(
                        "session timeout — press 'c' to reconnect".into(),
                    ))
                    .ok();
                    break;
                }
                match tokio::time::timeout(std::time::Duration::from_secs(5), ws.next()).await {
                    Ok(Some(Ok(WsMessage::Text(text)))) => {
                        // NIP-01 messages: ["EVENT", sub_id, event] or ["EOSE", sub_id]
                        if let Ok(arr) = serde_json::from_str::<serde_json::Value>(&text) {
                            if arr.get(0).and_then(|v| v.as_str()) == Some("EVENT") {
                                if let Some(ev) = arr.get(2) {
                                    let kind = ev["kind"].as_u64().unwrap_or(0);
                                    let id = ev["id"].as_str().unwrap_or("").to_string();
                                    let pubkey = ev["pubkey"].as_str().unwrap_or("").to_string();
                                    let created_at = ev["created_at"].as_u64().unwrap_or(0);
                                    // Try to get d-tag or first content chars as preview
                                    let d_tag = ev["tags"]
                                        .as_array()
                                        .and_then(|tags| {
                                            tags.iter().find(|t| {
                                                t.get(0).and_then(|v| v.as_str()) == Some("d")
                                            })
                                        })
                                        .and_then(|t| t.get(1))
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("");
                                    let content = ev["content"].as_str().unwrap_or("");
                                    let preview = if !d_tag.is_empty() {
                                        format!("d={d_tag}")
                                    } else {
                                        content.chars().take(80).collect()
                                    };
                                    tx.send(AppMsg::Nip34EventReceived(Nip34EventItem {
                                        kind,
                                        id,
                                        pubkey,
                                        created_at,
                                        content_preview: preview,
                                    }))
                                    .ok();
                                }
                            } else if arr.get(0).and_then(|v| v.as_str()) == Some("EOSE") {
                                // End of stored events — keep connection alive
                                // for live updates
                            } else if arr.get(0).and_then(|v| v.as_str()) == Some("NOTICE") {
                                let notice = arr
                                    .get(1)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                tx.send(AppMsg::Nip34Error(format!("relay notice: {notice}")))
                                    .ok();
                            }
                        }
                    }
                    Ok(Some(Ok(WsMessage::Close(_)))) => {
                        tx.send(AppMsg::Nip34Error("relay closed connection".into()))
                            .ok();
                        break;
                    }
                    Ok(Some(Err(e))) => {
                        tx.send(AppMsg::Nip34Error(format!("ws error: {e}"))).ok();
                        break;
                    }
                    Ok(None) => {
                        tx.send(AppMsg::Nip34Error("relay disconnected".into()))
                            .ok();
                        break;
                    }
                    Err(_) => {} // timeout, continue loop
                    _ => {}
                }
            }
        });
    }

    /// Add a path to the batch queue.
    pub fn add_batch_path(&mut self) {
        let path = self.batch_input.trim().to_string();
        if path.is_empty() {
            return;
        }
        self.batch_items.push(BatchItem {
            path,
            status: BatchStatus::Pending,
        });
        self.batch_input.clear();
    }

    /// Remove the last batch item.
    pub fn remove_last_batch_item(&mut self) {
        if !self.batch_running {
            self.batch_items.pop();
        }
    }

    /// Start uploading all pending items with concurrency limit 4.
    pub fn start_batch_upload(&mut self) {
        if self.batch_running || self.batch_items.is_empty() {
            return;
        }
        if self.secret_key.is_none() {
            self.notification = Some(("A secret key is required to upload.".into(), true));
            return;
        }
        self.batch_running = true;
        // Mark all pending
        for item in &mut self.batch_items {
            if matches!(item.status, BatchStatus::Pending | BatchStatus::Failed(_)) {
                item.status = BatchStatus::Running;
            }
        }

        let server = self.server.clone();
        let secret_key = self.secret_key.clone().unwrap();
        let tx = self.tx.clone();
        let paths: Vec<(usize, String)> = self
            .batch_items
            .iter()
            .enumerate()
            .map(|(i, item)| (i, item.path.clone()))
            .collect();

        tokio::spawn(async move {
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
            let mut handles = Vec::new();

            for (idx, path) in paths {
                let permit = sem.clone().acquire_owned().await.ok();
                let server = server.clone();
                let secret_key = secret_key.clone();
                let tx = tx.clone();
                let path = path.clone();

                handles.push(tokio::spawn(async move {
                    let _permit = permit;
                    let signer = match Signer::from_secret_hex(&secret_key) {
                        Ok(s) => s,
                        Err(e) => {
                            tx.send(AppMsg::BatchItemError(idx, e)).ok();
                            return;
                        }
                    };
                    let client = BlossomClient::new(vec![server], signer);
                    let p = std::path::Path::new(&path);
                    let mime = crate::mime_from_path(p);
                    match client.upload_file(p, &mime).await {
                        Ok(desc) => tx.send(AppMsg::BatchItemDone(idx, desc)).ok(),
                        Err(e) => tx.send(AppMsg::BatchItemError(idx, e)).ok(),
                    };
                }));
            }
            for h in handles {
                let _ = h.await;
            }
        });
    }
}

// ── Drawing
// ───────────────────────────────────────────────────────────────────

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Length(3), // tab bar
            Constraint::Min(0),    // content
            Constraint::Length(1), // status bar
        ])
        .split(area);

    draw_title_bar(f, app, chunks[0]);
    draw_tabs(f, app, chunks[1]);

    match app.tab {
        0 => draw_blobs_tab(f, app, chunks[2]),
        1 => draw_upload_tab(f, app, chunks[2]),
        2 => draw_batch_tab(f, app, chunks[2]),
        3 => draw_admin_tab(f, app, chunks[2]),
        4 => draw_relay_tab(f, app, chunks[2]),
        5 => draw_nips_tab(f, app, chunks[2]),
        6 => draw_status_tab(f, app, chunks[2]),
        7 => draw_keygen_tab(f, app, chunks[2]),
        _ => {}
    }

    draw_status_bar(f, app, chunks[3]);

    if app.show_help {
        draw_help_popup(f, area, app.tab, app.nip_tab);
    }

    if app.modal.is_some() {
        draw_modal_input(f, app, area);
    }
}

pub fn draw_title_bar(f: &mut Frame, app: &App, area: Rect) {
    let pubkey_info = match &app.pubkey {
        Some(pk) => format!("  pubkey: {}…", &pk[..16]),
        None => "  no key set".into(),
    };
    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", APP_TITLE),
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
                .bg(COLOR_TITLE_BG),
        ),
        Span::styled(
            format!(" {}", app.server),
            Style::default()
                .fg(Color::White)
                .bg(COLOR_TITLE_BG),
        ),
        Span::styled(
            pubkey_info,
            Style::default()
                .fg(Color::Rgb(140, 140, 180)) // soft lavender on navy
                .bg(COLOR_TITLE_BG),
        ),
    ]))
    .style(Style::default().bg(COLOR_TITLE_BG));
    f.render_widget(title, area);
}

pub fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<Line> = TAB_NAMES.iter().map(|&t| Line::from(t)).collect();
    let tabs = Tabs::new(titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" blossom-tui ")
                .style(Style::default().bg(COLOR_TITLE_BG)),
        )
        .select(app.tab)
        .style(Style::default().fg(COLOR_DIM).bg(COLOR_TITLE_BG))
        .highlight_style(
            Style::default()
                .fg(COLOR_ACCENT)
                .bg(COLOR_TITLE_BG)
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(tabs, area);
}

pub fn draw_blobs_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let loading_suffix = if app.blobs_loading {
        " (loading…)"
    } else {
        ""
    };
    let pubkey_label = app
        .pubkey
        .as_deref()
        .map(|pk| format!(" — {}", &pk[..16.min(pk.len())]))
        .unwrap_or_default();
    let sort_label = app.sort_field.label();
    let filter_label = if app.filter_str.is_empty() {
        String::new()
    } else {
        format!(" [filter: {}]", app.filter_str)
    };
    let title = format!(" Blobs{pubkey_label}{loading_suffix} │ sort:{sort_label}{filter_label} ");

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(COLOR_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.filter_mode {
        let filter_area = Rect {
            x: inner.x,
            y: inner.y + inner.height.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        let filter_bar = Paragraph::new(format!("/{}", app.filter_str))
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(filter_bar, filter_area);
    }

    if let Some(err) = app.blobs_error.clone() {
        let msg = Paragraph::new(Span::styled(
            format!("Error: {err}\n\nPress 'r' to retry."),
            Style::default().fg(COLOR_ERR),
        ))
        .wrap(Wrap { trim: false });
        f.render_widget(msg, inner);
        return;
    }

    if app.blobs.is_empty() && !app.blobs_loading {
        let msg = Paragraph::new(Line::from(vec![
            Span::styled("No blobs found. Press ", Style::default().fg(COLOR_DIM)),
            Span::styled(
                "r",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to refresh.", Style::default().fg(COLOR_DIM)),
        ]));
        f.render_widget(msg, inner);
        return;
    }

    // Apply filter
    let filter_lc = app.filter_str.to_lowercase();
    let mut visible: Vec<&BlobDescriptor> = app
        .blobs
        .iter()
        .filter(|b| {
            if filter_lc.is_empty() {
                return true;
            }
            b.sha256.to_lowercase().contains(&filter_lc)
                || b.content_type
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&filter_lc)
        })
        .collect();

    // Apply sort
    match app.sort_field {
        SortField::Date => visible.sort_by_key(|b| Reverse(b.uploaded.unwrap_or(0))),
        SortField::Size => visible.sort_by_key(|b| Reverse(b.size)),
        SortField::Hash => visible.sort_by(|a, b| a.sha256.cmp(&b.sha256)),
        SortField::ContentType => visible.sort_by(|a, b| {
            a.content_type
                .as_deref()
                .unwrap_or("")
                .cmp(b.content_type.as_deref().unwrap_or(""))
        }),
    }

    let sha_header_label = format!("SHA256");
    let size_header_label = if app.sort_field == SortField::Size {
        "Size ▲".to_string()
    } else {
        "Size".to_string()
    };
    let type_header_label = if app.sort_field == SortField::ContentType {
        "Content-Type ▲".to_string()
    } else {
        "Content-Type".to_string()
    };
    let date_header_label = if app.sort_field == SortField::Date {
        "Uploaded ▼".to_string()
    } else {
        "Uploaded".to_string()
    };

    let header = Row::new(vec![
        Cell::from(sha_header_label).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(COLOR_ACCENT),
        ),
        Cell::from(size_header_label).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(COLOR_ACCENT),
        ),
        Cell::from(type_header_label).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(COLOR_ACCENT),
        ),
        Cell::from(date_header_label).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(COLOR_ACCENT),
        ),
    ])
    .bottom_margin(1);

    let rows: Vec<Row> = visible
        .iter()
        .map(|b| {
            let sha = if b.sha256.len() > 20 {
                format!("{}…{}", &b.sha256[..16], &b.sha256[b.sha256.len() - 4..])
            } else {
                b.sha256.clone()
            };
            let size = format_size(b.size);
            let ctype = b.content_type.clone().unwrap_or_else(|| "—".into());
            let uploaded = b.uploaded.map(format_unix_ts).unwrap_or_else(|| "—".into());

            Row::new(vec![
                Cell::from(sha),
                Cell::from(size),
                Cell::from(ctype),
                Cell::from(uploaded),
            ])
        })
        .collect();

    let widths = [
        Constraint::Min(24),
        Constraint::Length(10),
        Constraint::Min(20),
        Constraint::Length(19),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(
            Style::default()
                .bg(COLOR_SELECTED_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(table, inner, &mut app.blobs_table);
}

pub fn draw_upload_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .title(" Upload File ")
        .border_style(Style::default().fg(COLOR_ACCENT));
    let outer_inner = outer.inner(area);
    f.render_widget(outer, area);

    // Horizontal split: left = file browser (40%), right = controls (60%).
    let h_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(outer_inner);

    // ── Left: file browser panel ─────────────────────────────────────────────
    draw_upload_filebrowser(f, app, h_split[0]);

    // ── Right: git panel (when git_mode) or controls ──────────────────────
    if app.git_mode {
        draw_git_panel(f, app, h_split[1]);
        return;
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // file path input
            Constraint::Length(3), // nip-94 publish row
            Constraint::Length(3), // controls hint
            Constraint::Min(3),    // result
        ])
        .split(h_split[1]);

    let input_border_style = if app.input_mode {
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let input_title = if app.input_mode {
        " File Path [editing] "
    } else {
        " File Path "
    };
    let input_block = Block::default()
        .borders(Borders::ALL)
        .title(input_title)
        .border_style(input_border_style);
    let input_para = Paragraph::new(app.upload_path.as_str())
        .block(input_block)
        .style(Style::default().fg(Color::White));
    f.render_widget(input_para, chunks[0]);

    if app.input_mode {
        f.set_cursor_position((
            chunks[0].x + app.upload_path.len() as u16 + 1,
            chunks[0].y + 1,
        ));
    }

    // NIP-94 publish row
    let nip94_toggle = if app.publish_nip94 { "[x]" } else { "[ ]" };
    let relay_label = if app.publish_relay.is_empty() {
        "(set relay URL)".to_string()
    } else {
        app.publish_relay.clone()
    };
    let relay_style = if app.publish_relay_edit {
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let nip94_line = Line::from(vec![
        Span::styled(
            "  p",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(": Publish NIP-94 event {nip94_toggle}  relay: ")),
        Span::styled(relay_label, relay_style),
        Span::raw("  "),
        Span::styled(
            "R",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(": edit relay URL"),
    ]);
    f.render_widget(
        Paragraph::new(nip94_line).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" NIP-94 Publish "),
        ),
        chunks[1],
    );
    if app.publish_relay_edit {
        f.set_cursor_position((
            chunks[1].x
                + 1
                + "  p: Publish NIP-94 event [x]  relay: ".len() as u16
                + app.publish_relay.len() as u16,
            chunks[1].y + 1,
        ));
    }

    let hints = if app.publish_relay_edit {
        Line::from(vec![
            Span::styled(
                "Enter/Esc",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": confirm relay URL"),
        ])
    } else if app.input_mode {
        Line::from(vec![
            Span::styled(
                "Enter",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": upload    "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": stop editing"),
        ])
    } else {
        Line::from(vec![
            Span::styled(
                "f",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": browse    "),
            Span::styled(
                "i",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": edit path    "),
            Span::styled(
                "Enter",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": upload    "),
            Span::styled(
                "Esc",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(": clear"),
        ])
    };
    let hints_para =
        Paragraph::new(hints).block(Block::default().borders(Borders::ALL).title(" Controls "));
    f.render_widget(hints_para, chunks[2]);

    if app.upload_loading {
        let loading = Paragraph::new("Uploading…")
            .block(Block::default().borders(Borders::ALL).title(" Result "))
            .style(Style::default().fg(COLOR_DIM));
        f.render_widget(loading, chunks[3]);
    } else if let Some(msg) = &app.upload_msg {
        let style = if app.upload_ok {
            Style::default().fg(COLOR_OK)
        } else {
            Style::default().fg(COLOR_ERR)
        };
        let result_para = Paragraph::new(msg.as_str())
            .block(Block::default().borders(Borders::ALL).title(" Result "))
            .style(style)
            .wrap(Wrap { trim: false });
        f.render_widget(result_para, chunks[3]);
    } else {
        let placeholder = Paragraph::new("No upload yet.")
            .block(Block::default().borders(Borders::ALL).title(" Result "))
            .style(Style::default().fg(COLOR_DIM));
        f.render_widget(placeholder, chunks[3]);
    }
}

/// Render the file-browser tree panel on the left side of the upload tab.
/// Git operations panel — replaces the right-hand controls pane
/// when `app.git_mode` is true.
fn draw_git_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let repo_name = app
        .git_repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.git_repo_path.to_string_lossy().into_owned());

    // Show branch info from GitRepoInfo when available.
    let branch_label = app
        .git_repo_info
        .as_ref()
        .and_then(|i| i.branch.as_deref())
        .map(|b| format!(" [{b}]"))
        .unwrap_or_default();

    let running_marker = if app.git_action_running { " …" } else { "" };
    let title = format!(" git — {repo_name}{branch_label}{running_marker} ");

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split: top = action menu bar, [optional commit input], bottom = output
    let menu_height: u16 = if app.git_commit_edit { 5 } else { 3 };
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(menu_height),
            Constraint::Min(1),
        ])
        .split(inner);

    // Action menu
    let accent = Style::default()
        .fg(COLOR_ACCENT)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(COLOR_DIM);

    let menu_lines = vec![
        Line::from(vec![
            Span::styled("s", accent), Span::raw(":status  "),
            Span::styled("l", accent), Span::raw(":log  "),
            Span::styled("d", accent), Span::raw(":diff  "),
            Span::styled("f", accent), Span::raw(":fetch"),
        ]),
        Line::from(vec![
            Span::styled("p", accent), Span::raw(":pull  "),
            Span::styled("P", accent), Span::raw(":push  "),
            Span::styled("a", accent), Span::raw(":add -A  "),
            Span::styled("c", accent), Span::raw(":commit  "),
            Span::styled("Esc", accent), Span::raw(":close"),
        ]),
    ];

    if app.git_commit_edit {
        let commit_lines: Vec<Line> = menu_lines
            .into_iter()
            .chain(std::iter::once(Line::from(vec![
                Span::styled("msg: ", dim),
                Span::styled(
                    app.git_commit_msg.as_str(),
                    Style::default().fg(Color::White),
                ),
                Span::styled("█", Style::default().fg(COLOR_ACCENT)),
            ])))
            .collect();
        f.render_widget(
            Paragraph::new(commit_lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Commit message (Enter: commit, Esc: cancel) "),
            ),
            split[0],
        );
        // cursor inside the commit block
        f.set_cursor_position((
            split[0].x + 1 + "msg: ".len() as u16
                + app.git_commit_msg.len() as u16,
            split[0].y + 3,
        ));
    } else {
        f.render_widget(
            Paragraph::new(menu_lines).block(
                Block::default().borders(Borders::ALL).title(" Actions "),
            ),
            split[0],
        );
    }

    // Output area
    let output_area = split[1];
    let visible_height = output_area.height.saturating_sub(2) as usize;
    let scroll = app.git_output_scroll;
    let lines: Vec<Line> = app
        .git_output
        .iter()
        .skip(scroll)
        .take(visible_height)
        .map(|l| {
            // Colour-code common git prefixes.
            let style = if l.starts_with('+') && !l.starts_with("+++") {
                Style::default().fg(COLOR_OK)
            } else if l.starts_with('-') && !l.starts_with("---") {
                Style::default().fg(COLOR_ERR)
            } else if l.starts_with('M') || l.starts_with("modified") {
                Style::default().fg(Color::Yellow)
            } else if l.starts_with('?') || l.starts_with("Untracked") {
                Style::default().fg(COLOR_DIM)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(l.as_str(), style))
        })
        .collect();

    let total = app.git_output.len();
    let scroll_hint = if total > visible_height {
        format!(
            " Output [{}/{}] ↑/↓ scroll ",
            scroll + 1,
            total
        )
    } else {
        " Output ".into()
    };

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(scroll_hint),
        ),
        output_area,
    );
}

fn draw_upload_filebrowser(f: &mut Frame, app: &mut App, area: Rect) {
    // Border colour: accent when active, dim otherwise.
    let border_style = if app.filebrowser_active {
        Style::default().fg(COLOR_ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    let cwd_label = app.filebrowser_cwd.to_string_lossy().into_owned();
    let max_cwd = area.width.saturating_sub(4) as usize;
    let cwd_display = if cwd_label.len() > max_cwd {
        format!(
            "…{}",
            &cwd_label[cwd_label.len().saturating_sub(max_cwd)..]
        )
    } else {
        cwd_label
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", cwd_display))
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if !app.filebrowser_active && app.filebrowser_entries.is_empty() {
        let hint = Paragraph::new("  Press f to browse files")
            .style(Style::default().fg(COLOR_DIM));
        f.render_widget(hint, inner);
        return;
    }

    // Check if selected entry is a git repo (to show g hint).
    let selected_is_git = app
        .filebrowser_list
        .selected()
        .and_then(|i| app.filebrowser_entries.get(i))
        .and_then(|e| e.git.as_ref())
        .is_some();

    // Reserve bottom line for git hint when applicable.
    let (list_area, hint_area) = if selected_is_git && app.filebrowser_active {
        let s = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        (s[0], Some(s[1]))
    } else {
        (inner, None)
    };

    let items: Vec<ListItem> = app
        .filebrowser_entries
        .iter()
        .map(|e| filebrowser_list_item(e))
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(COLOR_SELECTED_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");

    f.render_stateful_widget(list, list_area, &mut app.filebrowser_list);

    if let Some(ha) = hint_area {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "  g",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    ": open git panel",
                    Style::default().fg(COLOR_DIM),
                ),
            ])),
            ha,
        );
    }
}

pub fn draw_batch_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let running = if app.batch_running {
        " (running…)"
    } else {
        ""
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Batch Upload{running} "))
        .border_style(Style::default().fg(COLOR_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Horizontal split: left = file browser (40%), right = controls (60%).
    let h_split = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .split(inner);

    // ── Left: file browser ───────────────────────────────────────────────────
    draw_batch_filebrowser(f, app, h_split[0]);

    // ── Right: git panel (when git_mode) or controls ──────────────────────
    if app.git_mode {
        draw_git_panel(f, app, h_split[1]);
        return;
    }

    // ── Right: controls ──────────────────────────────────────────────────────
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // path input
            Constraint::Length(1), // hints
            Constraint::Min(1),    // queue
        ])
        .split(h_split[1]);

    // Path input
    let input_style = if app.batch_input_mode {
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_DIM)
    };
    let input_title = if app.batch_input_mode {
        " Path (Esc: cancel) "
    } else {
        " Path (f: browse  i: edit  Enter: add) "
    };
    let input = Paragraph::new(app.batch_input.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(input_title)
                .border_style(input_style),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(input, chunks[0]);

    if app.batch_input_mode {
        f.set_cursor_position((
            chunks[0].x + app.batch_input.len() as u16 + 1,
            chunks[0].y + 1,
        ));
    }

    // Hints
    let done = app
        .batch_items
        .iter()
        .filter(|i| matches!(i.status, BatchStatus::Done(_)))
        .count();
    let failed = app
        .batch_items
        .iter()
        .filter(|i| matches!(i.status, BatchStatus::Failed(_)))
        .count();
    let hint = format!(
        " {} queued  {} done  {} failed  \
         │  Enter: start upload  x: remove last",
        app.batch_items.len(),
        done,
        failed,
    );
    f.render_widget(
        Paragraph::new(hint.as_str()).style(Style::default().fg(COLOR_DIM)),
        chunks[1],
    );

    // Queue list
    let rows: Vec<Row> = app
        .batch_items
        .iter()
        .map(|item| {
            let (status_text, status_style) = match &item.status {
                BatchStatus::Pending => {
                    ("pending", Style::default().fg(COLOR_DIM))
                }
                BatchStatus::Running => {
                    ("running…", Style::default().fg(Color::Yellow))
                }
                BatchStatus::Done(_) => {
                    ("✓ done", Style::default().fg(COLOR_OK))
                }
                BatchStatus::Failed(e) => {
                    let _ = e;
                    ("✗ failed", Style::default().fg(COLOR_ERR))
                }
            };
            let path_display = if item.path.len() > 60 {
                format!("…{}", &item.path[item.path.len() - 57..])
            } else {
                item.path.clone()
            };
            Row::new(vec![
                Cell::from(path_display),
                Cell::from(status_text).style(status_style),
            ])
        })
        .collect();

    let widths = [Constraint::Min(40), Constraint::Length(12)];
    let table = Table::new(rows, widths).header(
        Row::new(vec![
            Cell::from("Path").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Cell::from("Status").style(
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
        .bottom_margin(1),
    );
    f.render_widget(table, chunks[2]);
}

/// File browser panel for the batch tab.
/// Shared helper: build a ListItem for a file browser entry.
fn filebrowser_list_item(e: &FileBrowserEntry) -> ListItem<'static> {
    let (icon, base_style) = if e.is_dir {
        ("▶ ", Style::default().fg(Color::Cyan))
    } else {
        ("  ", Style::default().fg(Color::White))
    };
    let mut spans =
        vec![Span::styled(format!("{icon}{}", e.name), base_style)];
    match &e.git {
        Some(info) => {
            let (badge, color) = match info.kind {
                GitRepoKind::Repo => (" git", Color::Yellow),
                GitRepoKind::Bare => (" bare", Color::Magenta),
            };
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                badge,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
            // show branch when known
            if let Some(branch) = &info.branch {
                spans.push(Span::styled(
                    format!(":{branch}"),
                    Style::default().fg(COLOR_DIM),
                ));
            }
            // show in-progress state badge
            if let Some(state) = &info.state {
                spans.push(Span::styled(
                    format!(" [{}]", state.label()),
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD),
                ));
            }
        }
        None => {}
    }
    ListItem::new(Line::from(spans))
}

/// Render the bottom git-hint line inside a file browser panel.
fn render_git_hint(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "  g",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                ": open git panel",
                Style::default().fg(COLOR_DIM),
            ),
        ])),
        area,
    );
}

fn draw_batch_filebrowser(f: &mut Frame, app: &mut App, area: Rect) {
    let border_style = if app.batch_filebrowser_active {
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(COLOR_DIM)
    };

    let cwd_label =
        app.batch_filebrowser_cwd.to_string_lossy().into_owned();
    let max_cwd = area.width.saturating_sub(4) as usize;
    let cwd_display = if cwd_label.len() > max_cwd {
        format!(
            "…{}",
            &cwd_label[cwd_label.len().saturating_sub(max_cwd)..]
        )
    } else {
        cwd_label
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", cwd_display))
        .border_style(border_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if !app.batch_filebrowser_active
        && app.batch_filebrowser_entries.is_empty()
    {
        f.render_widget(
            Paragraph::new("  Press f to browse files")
                .style(Style::default().fg(COLOR_DIM)),
            inner,
        );
        return;
    }

    let selected_is_git = app
        .batch_filebrowser_list
        .selected()
        .and_then(|i| app.batch_filebrowser_entries.get(i))
        .and_then(|e| e.git.as_ref())
        .is_some();

    let (list_area, hint_area) =
        if selected_is_git && app.batch_filebrowser_active {
            let s = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(1),
                ])
                .split(inner);
            (s[0], Some(s[1]))
        } else {
            (inner, None)
        };

    let items: Vec<ListItem> = app
        .batch_filebrowser_entries
        .iter()
        .map(filebrowser_list_item)
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(COLOR_SELECTED_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");

    f.render_stateful_widget(
        list,
        list_area,
        &mut app.batch_filebrowser_list,
    );

    if let Some(ha) = hint_area {
        render_git_hint(f, ha);
    }
}

pub fn draw_admin_tab(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Admin ")
        .border_style(Style::default().fg(COLOR_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);

    // Stats panel
    let stats_text = if app.admin_stats_loading {
        "Loading stats…".to_string()
    } else if let Some(e) = &app.admin_stats_error {
        format!("Error: {e}\n\nPress 'r' to retry.")
    } else if let Some(stats) = &app.admin_stats {
        serde_json::to_string_pretty(stats).unwrap_or_else(|_| stats.to_string())
    } else {
        "Press 'r' to load admin stats.".to_string()
    };

    let stats_block = Block::default()
        .borders(Borders::ALL)
        .title(" Stats ")
        .border_style(Style::default().fg(COLOR_DIM));
    f.render_widget(
        Paragraph::new(stats_text.as_str())
            .block(stats_block)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        chunks[0],
    );

    // Users panel
    let users_text = if app.admin_users_loading {
        "Loading users…".to_string()
    } else if let Some(e) = &app.admin_users_error {
        format!("Error: {e}")
    } else if let Some(users) = &app.admin_users {
        serde_json::to_string_pretty(users).unwrap_or_else(|_| users.to_string())
    } else {
        "Press 'r' to load users.".to_string()
    };

    let users_block = Block::default()
        .borders(Borders::ALL)
        .title(" Users ")
        .border_style(Style::default().fg(COLOR_DIM));
    f.render_widget(
        Paragraph::new(users_text.as_str())
            .block(users_block)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        chunks[1],
    );
}

pub fn draw_relay_tab(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Relay Admin ")
        .border_style(Style::default().fg(COLOR_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let text = if app.relay_policy_loading {
        "Loading relay policy…".to_string()
    } else if let Some(e) = &app.relay_policy_error {
        format!(
            "Error: {e}\n\nPress 'r' to retry.\n\nNote: relay endpoints require blossom-nip34 to be running."
        )
    } else if let Some(policy) = &app.relay_policy {
        serde_json::to_string_pretty(policy).unwrap_or_else(|_| policy.to_string())
    } else {
        "Press 'r' to load relay policy.\n\nNote: requires blossom-nip34 server.".to_string()
    };

    f.render_widget(
        Paragraph::new(text.as_str())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        inner,
    );
}

/// Container tab that renders the NIP sub-tab bar and dispatches to the
/// individual NIP draw functions based on `app.nip_tab`.
pub fn draw_nips_tab(f: &mut Frame, app: &mut App, area: Rect) {
    // Split: top 3 rows = sub-tab bar, rest = NIP content.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Draw the secondary tab bar.
    let nip_titles: Vec<Line> = NIP_TAB_NAMES
        .iter()
        .map(|&t| Line::from(t))
        .collect();
    let sub_tabs = Tabs::new(nip_titles)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" NIPs  [ / ] navigate ")
                .style(Style::default().bg(Color::Rgb(16, 32, 16))),
        )
        .select(app.nip_tab)
        .style(
            Style::default()
                .fg(COLOR_DIM)
                .bg(Color::Rgb(16, 32, 16)),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Green)
                .bg(Color::Rgb(16, 32, 16))
                .add_modifier(Modifier::BOLD),
        );
    f.render_widget(sub_tabs, split[0]);

    // Dispatch to the selected NIP tab draw function.
    match app.nip_tab {
        0 => draw_nip65_tab(f, app, split[1]),
        1 => draw_nip96_tab(f, app, split[1]),
        2 => draw_nip34_tab(f, app, split[1]),
        3 => draw_nipb7_tab(f, app, split[1]),
        4 => draw_profile_tab(f, app, split[1]),
        _ => {}
    }
}

pub fn draw_nip65_tab(f: &mut Frame, app: &App, area: Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // relay bar
            Constraint::Min(0),    // list
            Constraint::Length(3), // input
            Constraint::Length(2), // hints
        ])
        .split(area);

    // ── Publish relay bar ─────────────────────────────────────────────────────
    let relay_display = if app.nip65_nostr_relay.is_empty() {
        "<none — press 'R' to set>".to_string()
    } else {
        app.nip65_nostr_relay.clone()
    };
    let relay_style = if app.nip65_relay_edit {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(COLOR_DIM)
    };
    f.render_widget(
        Paragraph::new(format!(
            "{}: {relay_display}",
            if app.nip65_relay_edit { "Relay [editing]" } else { "Publish to relay" }
        ))
        .style(relay_style)
        .block(Block::default().borders(Borders::ALL).title(
            " Nostr Relay (for publishing kind:10002) ",
        )),
        outer[0],
    );

    // ── Relay list ────────────────────────────────────────────────────────────
    let items: Vec<ListItem> = app
        .nip65_relays
        .iter()
        .enumerate()
        .map(|(i, (url, marker))| {
            let marker_label = if marker.is_empty() {
                " [both] "
            } else if marker == "read" {
                " [read] "
            } else {
                "[write] "
            };
            let style = if i == app.nip65_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(COLOR_SELECTED_BG)
            } else {
                Style::default()
            };
            let marker_color = match marker.as_str() {
                "read"  => Color::Green,
                "write" => Color::Yellow,
                _       => Color::Cyan,
            };
            ListItem::new(Line::from(vec![
                Span::styled(marker_label, Style::default().fg(marker_color)),
                Span::styled(url.clone(), style),
            ]))
        })
        .collect();

    let list_title = format!(
        " NIP-65 Relay List — {} relays ",
        app.nip65_relays.len()
    );
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(list_title)
                .border_style(Style::default().fg(COLOR_ACCENT)),
        );
    f.render_widget(list, outer[1]);

    // ── Input bar ─────────────────────────────────────────────────────────────
    let input_text = if app.nip65_input_mode {
        format!("Add relay: {}█", app.nip65_input)
    } else {
        let marker_str = match app.nip65_marker_idx {
            0 => "both",
            1 => "read",
            _ => "write",
        };
        format!("New relay marker: [{marker_str}]  (press 'a' to add)")
    };
    let input_style = if app.nip65_input_mode {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(COLOR_DIM)
    };
    f.render_widget(
        Paragraph::new(input_text)
            .style(input_style)
            .block(Block::default().borders(Borders::ALL)),
        outer[2],
    );

    // ── Hints ─────────────────────────────────────────────────────────────────
    let hints = if app.nip65_input_mode {
        "Enter: confirm add   Esc: cancel"
    } else if app.nip65_relay_edit {
        "Enter/Esc: done   Type publish-relay URL"
    } else {
        "a:add  d:delete  m:cycle-marker  R:relay  P:publish  ↑↓:move"
    };
    f.render_widget(
        Paragraph::new(hints).style(Style::default().fg(COLOR_DIM)),
        outer[3],
    );
}

pub fn draw_nipb7_tab(f: &mut Frame, app: &App, area: Rect) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // relay bar
            Constraint::Min(0),    // list
            Constraint::Length(3), // input
            Constraint::Length(2), // hints
        ])
        .split(area);

    // ── Publish relay bar ─────────────────────────────────────────────────────
    let relay_display = if app.nipb7_nostr_relay.is_empty() {
        "<none — press 'R' to set>".to_string()
    } else {
        app.nipb7_nostr_relay.clone()
    };
    f.render_widget(
        Paragraph::new(format!(
            "{}: {relay_display}",
            if app.nipb7_relay_edit { "Relay [editing]" } else { "Publish to" }
        ))
        .style(if app.nipb7_relay_edit {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(COLOR_DIM)
        })
        .block(Block::default().borders(Borders::ALL).title(
            " Nostr Relay (for publishing kind:10063) ",
        )),
        outer[0],
    );

    // ── Server list ───────────────────────────────────────────────────────────
    let items: Vec<ListItem> = app
        .nipb7_servers
        .iter()
        .enumerate()
        .map(|(i, url)| {
            let style = if i == app.nipb7_selected {
                Style::default().fg(Color::Black).bg(COLOR_SELECTED_BG)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![
                Span::styled(" 🌸 ", Style::default().fg(Color::Magenta)),
                Span::styled(url.clone(), style),
            ]))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " NIP-B7 Blossom Server List — {} servers ",
                app.nipb7_servers.len()
            ))
            .border_style(Style::default().fg(COLOR_ACCENT)),
    );
    f.render_widget(list, outer[1]);

    // ── Input bar ─────────────────────────────────────────────────────────────
    let input_text = if app.nipb7_input_mode {
        format!("Add server: {}█", app.nipb7_input)
    } else {
        "(press 'a' to add a Blossom server URL)".to_string()
    };
    f.render_widget(
        Paragraph::new(input_text)
            .style(if app.nipb7_input_mode {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(COLOR_DIM)
            })
            .block(Block::default().borders(Borders::ALL)),
        outer[2],
    );

    // ── Hints ─────────────────────────────────────────────────────────────────
    let hints = if app.nipb7_input_mode {
        "Enter: confirm add   Esc: cancel"
    } else if app.nipb7_relay_edit {
        "Enter/Esc: done   Type Nostr relay URL"
    } else {
        "a:add  d:delete  R:relay  P:publish kind:10063  ↑↓:move"
    };
    f.render_widget(
        Paragraph::new(hints).style(Style::default().fg(COLOR_DIM)),
        outer[3],
    );
}

pub fn draw_nip96_tab(f: &mut Frame, app: &App, area: Rect) {
    // Split into top (server info) and bottom (file list)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    // ── NIP-96 server capabilities ────────────────────────────────────────────
    let info_block = Block::default()
        .borders(Borders::ALL)
        .title(" NIP-96 Server Info (.well-known/nostr/nip96.json) ")
        .border_style(Style::default().fg(COLOR_ACCENT));
    let info_inner = info_block.inner(chunks[0]);
    f.render_widget(info_block, chunks[0]);

    let info_text = if app.nip96_info_loading {
        "Loading…".to_string()
    } else if let Some(e) = &app.nip96_info_error {
        format!("Error: {e}\n\nPress 'r' to retry.")
    } else if let Some(info) = &app.nip96_info {
        // Pretty-print key fields
        let mut lines = Vec::new();
        if let Some(api) = info.get("api_url").and_then(|v| v.as_str()) {
            lines.push(format!("api_url:      {api}"));
        }
        if let Some(dl) = info.get("download_url").and_then(|v| v.as_str()) {
            lines.push(format!("download_url: {dl}"));
        }
        if let Some(nips) = info.get("supported_nips") {
            lines.push(format!("supported:    {nips}"));
        }
        if let Some(max) = info.get("max_byte_size").and_then(|v| v.as_u64()) {
            let mb = max / (1024 * 1024);
            lines.push(format!("max_size:     {max} bytes ({mb} MB)"));
        }
        if let Some(types) = info.get("content_types") {
            lines.push(format!("content_types:{types}"));
        }
        if let Some(plans) = info.get("plans") {
            lines.push(format!(
                "\nPlans:\n{}",
                serde_json::to_string_pretty(plans).unwrap_or_default()
            ));
        }
        if lines.is_empty() {
            serde_json::to_string_pretty(info).unwrap_or_else(|_| info.to_string())
        } else {
            lines.join("\n")
        }
    } else {
        "Press 'r' to load NIP-96 server info.".to_string()
    };

    f.render_widget(
        Paragraph::new(info_text.as_str())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        info_inner,
    );

    // ── NIP-96 file list ──────────────────────────────────────────────────────
    let files_title = if app.nip96_files_loading {
        " NIP-96 Files (loading…) ".to_string()
    } else {
        " NIP-96 Files (/n96) ".to_string()
    };
    let files_block = Block::default()
        .borders(Borders::ALL)
        .title(files_title.as_str())
        .border_style(Style::default().fg(COLOR_ACCENT));
    let files_inner = files_block.inner(chunks[1]);
    f.render_widget(files_block, chunks[1]);

    let files_text = if let Some(e) = &app.nip96_files_error {
        format!("Error: {e}\n\nNote: file listing requires authentication.")
    } else if let Some(files) = &app.nip96_files {
        // Try to extract file list from NIP-96 response
        let items = files
            .get("files")
            .or_else(|| files.get("data"))
            .and_then(|v| v.as_array());
        if let Some(arr) = items {
            if arr.is_empty() {
                "(no files)".to_string()
            } else {
                arr.iter()
                    .take(20)
                    .map(|f| {
                        let hash = f
                            .get("ox")
                            .or_else(|| f.get("x"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("?");
                        let mime = f.get("m").and_then(|v| v.as_str()).unwrap_or("?");
                        let url = f.get("url").and_then(|v| v.as_str()).unwrap_or("");
                        format!("{:.16}  {:<20}  {}", hash, mime, url)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        } else {
            serde_json::to_string_pretty(files).unwrap_or_else(|_| files.to_string())
        }
    } else if !app.nip96_files_loading {
        "(no data — press 'r' to load)".to_string()
    } else {
        String::new()
    };

    f.render_widget(
        Paragraph::new(files_text.as_str())
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(Color::White)),
        files_inner,
    );
}

pub fn draw_nip34_tab(f: &mut Frame, app: &mut App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // relay URL bar
            Constraint::Length(1), // status line
            Constraint::Min(1),    // events table
        ])
        .split(area);

    // ── Relay URL bar ─────────────────────────────────────────────────────────
    let relay_display = if app.nip34_relay.is_empty() {
        "(enter relay URL)".to_string()
    } else {
        app.nip34_relay.clone()
    };
    let relay_style = if app.nip34_relay_edit {
        Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let relay_title = if app.nip34_relay_edit {
        " Relay URL [editing — Enter/Esc to confirm] "
    } else {
        " Relay URL (r to edit, c to connect) "
    };
    let relay_block = Block::default()
        .borders(Borders::ALL)
        .title(relay_title)
        .border_style(if app.nip34_relay_edit {
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_DIM)
        });
    f.render_widget(
        Paragraph::new(relay_display.as_str())
            .block(relay_block)
            .style(relay_style),
        chunks[0],
    );
    if app.nip34_relay_edit {
        f.set_cursor_position((
            chunks[0].x + 1 + app.nip34_relay.len() as u16,
            chunks[0].y + 1,
        ));
    }

    // ── Status line ───────────────────────────────────────────────────────────
    let status_color = if app.nip34_connected {
        COLOR_OK
    } else {
        COLOR_DIM
    };
    f.render_widget(
        Paragraph::new(app.nip34_status.as_str()).style(Style::default().fg(status_color)),
        chunks[1],
    );

    // ── Events table ──────────────────────────────────────────────────────────
    let header = Row::new(vec![
        Cell::from("Kind").style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("ID").style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Pubkey").style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Created").style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Cell::from("Preview").style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
    ])
    .height(1)
    .bottom_margin(0);

    let rows: Vec<Row> = app
        .nip34_events
        .iter()
        .map(|ev| {
            let ts = chrono_fmt_unix(ev.created_at);
            Row::new(vec![
                Cell::from(ev.kind_name()),
                Cell::from(format!("{:.8}", ev.id)),
                Cell::from(format!("{:.8}", ev.pubkey)),
                Cell::from(ts),
                Cell::from(ev.content_preview.as_str()),
            ])
        })
        .collect();

    let total = rows.len();
    let table = Table::new(
        rows,
        [
            Constraint::Length(13), // kind name
            Constraint::Length(9),  // id prefix
            Constraint::Length(9),  // pubkey prefix
            Constraint::Length(11), // created_at
            Constraint::Min(20),    // preview
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" NIP-34 Events ({total}) "))
            .border_style(Style::default().fg(COLOR_ACCENT)),
    )
    .row_highlight_style(
        Style::default()
            .bg(COLOR_SELECTED_BG)
            .add_modifier(Modifier::BOLD),
    );

    f.render_stateful_widget(table, chunks[2], &mut app.nip34_events_table);
}

pub fn draw_status_tab(f: &mut Frame, app: &App, area: Rect) {
    let loading_suffix = if app.status_loading {
        " (loading…)"
    } else {
        ""
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Status — {}{loading_suffix} ", app.server))
        .border_style(Style::default().fg(COLOR_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(err) = &app.status_error {
        let msg = Paragraph::new(Span::styled(
            format!("Error: {err}\n\nPress 'r' to retry."),
            Style::default().fg(COLOR_ERR),
        ))
        .wrap(Wrap { trim: false });
        f.render_widget(msg, inner);
        return;
    }

    if let Some(data) = &app.status_data {
        let text = serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string());
        let para = Paragraph::new(text)
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: false });
        f.render_widget(para, inner);
    } else if !app.status_loading {
        let msg = Paragraph::new(Span::styled(
            "Press 'r' to fetch server status.",
            Style::default().fg(COLOR_DIM),
        ));
        f.render_widget(msg, inner);
    }
}

pub fn draw_keygen_tab(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Key Generation ")
        .border_style(Style::default().fg(COLOR_ACCENT));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // hint
            Constraint::Length(2), // secret hex
            Constraint::Length(2), // nsec
            Constraint::Length(2), // pubkey hex
            Constraint::Length(2), // npub
            Constraint::Length(2), // copy hints
            Constraint::Min(0),    // warning / padding
        ])
        .split(inner);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled(
            "g",
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(": generate a new BIP-340 keypair"),
    ]));
    f.render_widget(hint, chunks[0]);

    if let Some(kp) = &app.keygen_data {
        let label = Style::default()
            .fg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD);
        let val = Style::default().fg(Color::White);
        let key_hint = Style::default().fg(COLOR_DIM);
        let copied_style = Style::default().fg(COLOR_OK).add_modifier(Modifier::BOLD);
        let copied_badge = |field: u8| -> Span<'static> {
            if app.keygen_copied == Some(field) {
                Span::styled(" ✓ in clipboard", copied_style)
            } else {
                Span::raw("")
            }
        };

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[1] Secret (hex):  ", label),
                Span::styled(kp.hex_secret.clone(), val),
                copied_badge(1),
            ])),
            chunks[1],
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[2] Secret (nsec): ", label),
                Span::styled(kp.nsec.clone(), val),
                copied_badge(2),
            ])),
            chunks[2],
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[3] Pubkey  (hex): ", label),
                Span::styled(kp.pubkey.clone(), val),
                copied_badge(3),
            ])),
            chunks[3],
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[4] Pubkey  (npub):", label),
                Span::raw(" "),
                Span::styled(kp.npub.clone(), val),
                copied_badge(4),
            ])),
            chunks[4],
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                "    Press 1 / 2 / 3 / 4 to copy the corresponding value \
                 to the clipboard.",
                key_hint,
            )),
            chunks[5],
        );
        f.render_widget(
            Paragraph::new(Span::styled(
                "⚠  Keep the secret key safe — it is not stored anywhere.",
                Style::default().fg(Color::Yellow),
            )),
            chunks[6],
        );
    }
}

pub fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some((msg, is_err)) = &app.notification {
        Line::from(Span::styled(
            format!(" {msg}"),
            Style::default()
                .fg(if *is_err { COLOR_ERR } else { COLOR_OK })
                .bg(COLOR_TITLE_BG),
        ))
    } else if app.show_help {
        Line::from(Span::styled(
            " ?:close  Tab/S-Tab:switch-tabs  q:quit",
            Style::default().fg(Color::White).bg(COLOR_TITLE_BG),
        ))
    } else {
        let hints = match app.tab {
            0 => " r:refresh  d:delete  o:download  m:mirror  s:sort  /:filter  y:copy-sha  u:copy-url  Enter:open  ↑↓/jk  Tab  ?  q",
            1 => {
                " i:edit-path  p:toggle-nip94  R:relay-url  Enter:upload  Esc:clear  Tab:next  ?:help  q:quit"
            }
            2 => " i:edit  Enter:add/start  x:remove-last  Tab:next  ?:help  q:quit",
            3 => " r:refresh  Tab:next  ?:help  q:quit",
            4 => " r:refresh  Tab:next  ?:help  q:quit",
            5 => match app.nip_tab {
                0 => " a:add  d:delete  m:marker  R:relay  P:publish  [ ]:switch-nip  Tab  ?  q",
                1 => " r:refresh  [ ]:switch-nip  Tab  ?  q",
                2 => " r:edit-relay  c:connect  ↑↓:scroll  [ ]:switch-nip  Tab  ?  q",
                3 => " a:add  d:delete  R:relay  P:publish  [ ]:switch-nip  Tab  ?  q",
                _ => " ↑↓:navigate  e:edit  r:relay  P:publish-kind0  [ ]:switch-nip  Tab  ?  q",
            },
            6 => " r:refresh  Tab:next  ?:help  q:quit",
            7 => " g:generate  1:hex  2:nsec  3:pubkey  4:npub  Tab:next  ?:help  q:quit",
            _ => " Tab:next  ?:help  q:quit",
        };
        Line::from(Span::styled(
            hints,
            Style::default().fg(Color::White).bg(COLOR_TITLE_BG),
        ))
    };
    f.render_widget(
        Paragraph::new(content).style(Style::default().bg(COLOR_TITLE_BG)),
        area,
    );
}

pub fn draw_modal_input(f: &mut Frame, app: &App, area: Rect) {
    let (title, label) = match &app.modal {
        Some(Modal::Download { sha256 }) => (
            " Download Blob ",
            format!(
                "Save path for {}…{}:",
                &sha256[..8.min(sha256.len())],
                &sha256[sha256.len().saturating_sub(4)..]
            ),
        ),
        Some(Modal::Mirror) => (" Mirror Blob ", "Remote URL to mirror:".to_string()),
        None => return,
    };

    let popup_w = 60u16.min(area.width.saturating_sub(4));
    let popup_h = 7u16;
    let popup_x = (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    f.render_widget(Clear, popup_area);

    let inner_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .margin(1)
        .split(popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(COLOR_ACCENT));
    f.render_widget(block, popup_area);

    f.render_widget(
        Paragraph::new(label.as_str()).style(Style::default().fg(COLOR_DIM)),
        inner_chunks[0],
    );

    let input = Paragraph::new(app.modal_input.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_ACCENT)),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(input, inner_chunks[1]);

    f.set_cursor_position((
        inner_chunks[1].x + app.modal_input.len() as u16 + 1,
        inner_chunks[1].y + 1,
    ));

    f.render_widget(
        Paragraph::new("Enter: confirm   Esc: cancel").style(Style::default().fg(COLOR_DIM)),
        inner_chunks[2],
    );
}

pub fn draw_profile_tab(f: &mut Frame, app: &mut App, area: Rect) {
    use ratatui::layout::Flex;

    let fields = [
        ("Name",       app.profile_name.as_str()),
        ("About",      app.profile_about.as_str()),
        ("Picture URL", app.profile_picture.as_str()),
        ("NIP-05",     app.profile_nip05.as_str()),
        ("Website",    app.profile_website.as_str()),
        ("LUD-16",     app.profile_lud16.as_str()),
    ];

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // relay bar
            Constraint::Min(0),     // fields
            Constraint::Length(2),  // hints
        ])
        .split(area);

    // ── Relay bar ────────────────────────────────────────────────────────────
    let relay_label = if app.profile_relay_edit { "Relay [editing]: " } else { "Relay: " };
    let relay_display = if app.profile_nostr_relay.is_empty() {
        "<none — press 'r' to set>".to_string()
    } else {
        app.profile_nostr_relay.clone()
    };
    let relay_style = if app.profile_relay_edit {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(COLOR_DIM)
    };
    f.render_widget(
        Paragraph::new(format!("{relay_label}{relay_display}"))
            .style(relay_style)
            .block(Block::default().borders(Borders::ALL).title(" Nostr Relay ")),
        outer[0],
    );

    // ── Field rows ───────────────────────────────────────────────────────────
    let rows_area = outer[1];
    let row_h = 3u16;
    let max_fields = ((rows_area.height as usize) / row_h as usize).min(fields.len());

    let constraints: Vec<Constraint> = (0..max_fields)
        .map(|_| Constraint::Length(row_h))
        .collect();
    let row_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(rows_area);

    for (i, ((label, value), chunk)) in
        fields.iter().take(max_fields).zip(row_chunks.iter()).enumerate()
    {
        let is_active = i == app.profile_edit_field && app.profile_editing;
        let border_style = if i == app.profile_edit_field {
            if is_active {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(COLOR_ACCENT)
            }
        } else {
            Style::default()
        };
        let display = if is_active {
            format!("{value}█") // cursor indicator
        } else if value.is_empty() {
            format!("<{}>", label.to_lowercase().replace(' ', "_"))
        } else {
            value.to_string()
        };
        f.render_widget(
            Paragraph::new(display)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(border_style)
                        .title(format!(" [{}] {} ", i + 1, label)),
                )
                .wrap(ratatui::widgets::Wrap { trim: true }),
            *chunk,
        );
    }

    // ── Hints ────────────────────────────────────────────────────────────────
    let (hint_left, hint_right) = if app.profile_editing {
        ("Enter: next field  Esc: finish  Backspace: delete", "")
    } else if app.profile_relay_edit {
        ("Enter/Esc: finish  Type relay URL", "")
    } else {
        (
            "↑↓: navigate  1-6: jump  e/Enter: edit  r: relay  F: fetch  P: publish",
            if app.profile_loading { "Loading…" } else { "" },
        )
    };
    let hints = Paragraph::new(format!("{hint_left}  {hint_right}"))
        .style(Style::default().fg(COLOR_DIM));
    f.render_widget(hints, outer[2]);

    // ── Error banner ─────────────────────────────────────────────────────────
    if let Some(err) = &app.profile_error {
        let err_msg = err.clone();
        let err_area = Rect {
            x: area.x + 1,
            y: area.y + area.height.saturating_sub(3),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        f.render_widget(
            Paragraph::new(format!("⚠ {err_msg}"))
                .style(Style::default().fg(COLOR_ERR)),
            err_area,
        );
    }
}

pub fn draw_help_popup(f: &mut Frame, area: Rect, tab: usize, nip_tab: usize) {
    let key = Style::default().fg(Color::Yellow);
    let heading = Style::default()
        .fg(COLOR_ACCENT)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(COLOR_DIM);

    // Helper closures to keep line construction concise.
    let kv = |k: &'static str, v: &'static str| -> Line<'static> {
        Line::from(vec![
            Span::styled(k, key),
            Span::styled(v, Style::default()),
        ])
    };

    // Global bindings, always shown.
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled("  Global", heading)),
        Line::from(""),
        kv("  Tab / Shift+Tab  ", "Switch main tabs"),
        kv("  q / Ctrl+C       ", "Quit"),
        kv("  ?                ", "Toggle this help"),
    ];

    // Tab-specific bindings.
    let (tab_title, tab_lines): (&str, Vec<Line>) = match tab {
        // Blobs
        0 => (
            " Blobs ",
            vec![
                kv("  ↑ / k            ", "Navigate up"),
                kv("  ↓ / j            ", "Navigate down"),
                kv("  r                ", "Refresh blob list"),
                kv("  d                ", "Delete selected blob"),
                kv("  o                ", "Download selected blob"),
                kv("  m                ", "Mirror blob from URL"),
                kv("  s                ", "Cycle sort (Date / Size / Hash / Type)"),
                kv("  /                ", "Filter (Enter confirm, Esc clear)"),
                kv("  y                ", "Copy SHA-256 to clipboard"),
                kv("  u                ", "Copy URL to clipboard"),
                kv("  Enter            ", "Open in system default app"),
            ],
        ),
        // Upload
        1 => (
            " Upload ",
            vec![
                kv("  f                ", "Browse file tree"),
                kv("  i                ", "Enter file-path edit mode"),
                kv("  p                ", "Toggle NIP-94 publish"),
                kv("  R                ", "Edit relay URL"),
                kv("  Enter            ", "Start upload"),
                kv("  Esc              ", "Exit edit mode / clear path"),
                Line::from(""),
                Line::from(Span::styled(
                    "  File browser (f to open)",
                    Style::default().fg(COLOR_DIM),
                )),
                kv("  ↑ / k            ", "Navigate up"),
                kv("  ↓ / j            ", "Navigate down"),
                kv("  Enter            ", "Enter dir / accept file"),
                kv("  Backspace / h / -", "Go to parent directory"),
                kv("  Esc              ", "Go up (close at root)"),
                kv("  g                ", "Open git panel (on git repos)"),
                kv("  f                ", "Close file browser"),
                Line::from(""),
                Line::from(Span::styled(
                    "  Git panel (g on repo entry)",
                    Style::default().fg(COLOR_DIM),
                )),
                kv("  s                ", "git status"),
                kv("  l                ", "git log --oneline -20"),
                kv("  d                ", "git diff"),
                kv("  f                ", "git fetch --all"),
                kv("  p                ", "git pull"),
                kv("  P                ", "git push"),
                kv("  a                ", "git add -A"),
                kv("  c                ", "git commit (enter message)"),
                kv("  ↑ / k  ↓ / j     ", "Scroll output"),
                kv("  Esc / q          ", "Close git panel"),
            ],
        ),
        // Batch
        2 => (
            " Batch ",
            vec![
                kv("  f                ", "Browse file tree"),
                kv("  i                ", "Add a file path to the queue"),
                kv("  x                ", "Remove last queued item"),
                kv("  Enter            ", "Start batch upload"),
                Line::from(""),
                Line::from(Span::styled(
                    "  File browser (f to open)",
                    Style::default().fg(COLOR_DIM),
                )),
                kv("  ↑ / k            ", "Navigate up"),
                kv("  ↓ / j            ", "Navigate down"),
                kv(
                    "  Enter            ",
                    "Enter dir / add file to queue",
                ),
                kv(
                    "  Backspace / h / -",
                    "Go to parent directory",
                ),
                kv("  Esc              ", "Go up (close at root)"),
                kv("  g                ", "Open git panel (on git repos)"),
                kv("  f                ", "Close file browser"),
                Line::from(""),
                Line::from(Span::styled(
                    "  Git panel (g on repo entry)",
                    Style::default().fg(COLOR_DIM),
                )),
                kv("  s / l / d / f    ", "status / log / diff / fetch"),
                kv("  p / P            ", "pull / push"),
                kv("  a / c            ", "add -A / commit"),
                kv("  ↑ / k  ↓ / j     ", "Scroll output"),
                kv("  Esc / q          ", "Close git panel"),
            ],
        ),
        // Admin
        3 => (
            " Admin ",
            vec![kv("  r                ", "Refresh admin stats & user list")],
        ),
        // Relay
        4 => (
            " Relay ",
            vec![kv("  r                ", "Refresh relay policy")],
        ),
        // NIPs container — show sub-tab-specific help
        5 => match nip_tab {
            0 => (
                " NIPs › NIP-65 Relay List ",
                vec![
                    kv("  [ / ]            ", "Switch NIP sub-tab"),
                    kv("  a                ", "Add new relay URL"),
                    kv("  d / Delete       ", "Remove selected relay"),
                    kv("  m                ", "Cycle marker (both/read/write)"),
                    kv("  R                ", "Set publish relay URL"),
                    kv("  P                ", "Publish kind:10002 relay list"),
                    kv("  ↑ / ↓            ", "Move selection"),
                ],
            ),
            1 => (
                " NIPs › NIP-96 ",
                vec![
                    kv("  [ / ]            ", "Switch NIP sub-tab"),
                    kv("  r                ", "Refresh NIP-96 server info"),
                ],
            ),
            2 => (
                " NIPs › NIP-34 ",
                vec![
                    kv("  [ / ]            ", "Switch NIP sub-tab"),
                    kv("  r                ", "Edit relay URL"),
                    kv("  c                ", "Connect and subscribe"),
                    kv("  ↑ / k            ", "Scroll event list up"),
                    kv("  ↓ / j            ", "Scroll event list down"),
                ],
            ),
            3 => (
                " NIPs › NIP-B7 Server List ",
                vec![
                    kv("  [ / ]            ", "Switch NIP sub-tab"),
                    kv("  a                ", "Add server URL"),
                    kv("  d / Delete       ", "Remove selected server"),
                    kv("  R                ", "Set publish relay URL"),
                    kv("  P                ", "Publish kind:10063 server list"),
                    kv("  ↑ / ↓            ", "Move selection"),
                ],
            ),
            _ => (
                " NIPs › Profile (NIP-01) ",
                vec![
                    kv("  [ / ]            ", "Switch NIP sub-tab"),
                    kv("  ↑ / ↓            ", "Navigate fields"),
                    kv("  1-6              ", "Jump to field"),
                    kv("  e / Enter        ", "Edit selected field"),
                    kv("  r                ", "Set Nostr relay URL"),
                    kv("  P                ", "Publish kind:0 metadata event"),
                    kv("  Esc              ", "Stop editing current field"),
                ],
            ),
        },
        // Status
        6 => (
            " Status ",
            vec![kv("  r                ", "Refresh server status")],
        ),
        // Keygen
        7 => (
            " Keygen ",
            vec![
                kv("  g                ", "Generate new BIP-340 keypair"),
                kv("  1                ", "Copy secret key (hex) to clipboard"),
                kv("  2                ", "Copy nsec (NIP-19 bech32) to clipboard"),
                kv("  3                ", "Copy public key (hex) to clipboard"),
                kv("  4                ", "Copy npub (NIP-19 bech32) to clipboard"),
            ],
        ),
        _ => ("", vec![]),
    };

    if !tab_lines.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  {tab_title}tab"),
            heading,
        )));
        lines.push(Line::from(""));
        lines.extend(tab_lines);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Press ? or Esc to close",
        dim,
    )));

    let popup_h = (lines.len() as u16 + 2).min(area.height.saturating_sub(4));
    let popup_w = 62u16.min(area.width.saturating_sub(4));
    let popup_x = (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_w, popup_h);

    f.render_widget(Clear, popup_area);
    let help = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Help —{tab_title}press ? to close "))
                .border_style(Style::default().fg(COLOR_ACCENT)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(help, popup_area);
}

// ── Main event loop
// ───────────────────────────────────────────────────────────

pub async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<AppMsg>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        while let Ok(msg) = rx.try_recv() {
            app.apply(msg);
        }

        terminal.draw(|f| draw(f, app))?;

        let has_event = tokio::task::block_in_place(|| event::poll(Duration::from_millis(100)))?;
        if !has_event {
            continue;
        }

        let evt = tokio::task::block_in_place(event::read)?;

        match evt {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                // Modal input intercepts all keys when active
                if app.modal.is_some() {
                    match key.code {
                        KeyCode::Esc => {
                            app.modal = None;
                            app.modal_input.clear();
                        }
                        KeyCode::Enter => match &app.modal {
                            Some(Modal::Download { .. }) => app.confirm_download(),
                            Some(Modal::Mirror) => app.confirm_mirror(),
                            None => {}
                        },
                        KeyCode::Backspace => {
                            app.modal_input.pop();
                        }
                        KeyCode::Char(c) => app.modal_input.push(c),
                        _ => {}
                    }
                    continue;
                }

                // Filter mode intercepts keys in Blobs tab
                if app.filter_mode && app.tab == 0 {
                    match key.code {
                        KeyCode::Esc => app.clear_filter(),
                        KeyCode::Enter => app.exit_filter_mode(),
                        KeyCode::Backspace => {
                            app.filter_str.pop();
                        }
                        KeyCode::Char(c) => {
                            app.filter_str.push(c);
                            app.blobs_table.select(Some(0));
                        }
                        _ => {}
                    }
                    continue;
                }

                if (!app.input_mode && key.code == KeyCode::Char('q'))
                    || (key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c'))
                {
                    return Ok(());
                }

                if !app.input_mode && key.code == KeyCode::Char('?') {
                    app.show_help = !app.show_help;
                    app.notification = None;
                    continue;
                }

                if app.show_help {
                    app.show_help = false;
                    continue;
                }

                app.notification = None;

                if !app.input_mode {
                    match key.code {
                        KeyCode::Tab => {
                            app.next_tab();
                            continue;
                        }
                        KeyCode::BackTab => {
                            app.prev_tab();
                            continue;
                        }
                        _ => {}
                    }
                }

                if app.input_mode {
                    match key.code {
                        KeyCode::Esc => app.input_mode = false,
                        KeyCode::Enter => {
                            app.input_mode = false;
                            app.start_upload();
                        }
                        KeyCode::Backspace => {
                            app.upload_path.pop();
                        }
                        KeyCode::Char(c) => app.upload_path.push(c),
                        _ => {}
                    }
                    continue;
                }

                if app.publish_relay_edit {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => app.publish_relay_edit = false,
                        KeyCode::Backspace => {
                            app.publish_relay.pop();
                        }
                        KeyCode::Char(c) => app.publish_relay.push(c),
                        _ => {}
                    }
                    continue;
                }

                if app.nip34_relay_edit {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter => app.nip34_relay_edit = false,
                        KeyCode::Backspace => {
                            app.nip34_relay.pop();
                        }
                        KeyCode::Char(c) => app.nip34_relay.push(c),
                        _ => {}
                    }
                    continue;
                }

                if app.git_commit_edit {
                    match key.code {
                        KeyCode::Esc => {
                            app.git_commit_edit = false;
                        }
                        KeyCode::Enter => {
                            app.git_commit_edit = false;
                            app.run_git_action(GitAction::Commit);
                        }
                        KeyCode::Backspace => {
                            app.git_commit_msg.pop();
                        }
                        KeyCode::Char(c) => app.git_commit_msg.push(c),
                        _ => {}
                    }
                    continue;
                }

                if app.batch_input_mode {
                    match key.code {
                        KeyCode::Esc => app.batch_input_mode = false,
                        KeyCode::Enter => {
                            app.add_batch_path();
                            app.batch_input_mode = false;
                        }
                        KeyCode::Backspace => {
                            app.batch_input.pop();
                        }
                        KeyCode::Char(c) => app.batch_input.push(c),
                        _ => {}
                    }
                    continue;
                }

                match app.tab {
                    0 => match key.code {
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                        KeyCode::Char('r') => app.refresh_blobs(),
                        KeyCode::Char('d') => app.delete_selected(),
                        KeyCode::Char('o') => app.prompt_download(),
                        KeyCode::Char('m') => app.prompt_mirror(),
                        KeyCode::Char('s') => app.cycle_sort(),
                        KeyCode::Char('/') => app.enter_filter_mode(),
                        KeyCode::Char('y') => app.copy_selected_sha256(),
                        KeyCode::Char('u') => app.copy_selected_url(),
                        KeyCode::Enter => app.open_selected_blob(),
                        _ => {}
                    },
                    1 => {
                        // Git panel takes highest priority.
                        if app.git_mode {
                            match key.code {
                                KeyCode::Char('s') => {
                                    app.run_git_action(GitAction::Status)
                                }
                                KeyCode::Char('l') => {
                                    app.run_git_action(GitAction::Log)
                                }
                                KeyCode::Char('d') => {
                                    app.run_git_action(GitAction::Diff)
                                }
                                KeyCode::Char('f') => {
                                    app.run_git_action(GitAction::Fetch)
                                }
                                KeyCode::Char('p') => {
                                    app.run_git_action(GitAction::Pull)
                                }
                                KeyCode::Char('P') => {
                                    app.run_git_action(GitAction::Push)
                                }
                                KeyCode::Char('a') => {
                                    app.run_git_action(GitAction::Add)
                                }
                                KeyCode::Char('c') => {
                                    app.git_commit_edit = true;
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.git_scroll_up()
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    app.git_scroll_down(20)
                                }
                                KeyCode::Esc | KeyCode::Char('q') => {
                                    app.git_mode = false
                                }
                                _ => {}
                            }
                        // File browser takes next priority when active.
                        } else if app.filebrowser_active {
                            match key.code {
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.filebrowser_scroll_up()
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    app.filebrowser_scroll_down()
                                }
                                KeyCode::Enter => app.filebrowser_enter(),
                                KeyCode::Backspace
                                | KeyCode::Char('h')
                                | KeyCode::Char('-')
                                | KeyCode::Esc => {
                                    if app.filebrowser_cwd.parent().is_some() {
                                        app.filebrowser_parent();
                                    } else {
                                        app.filebrowser_active = false;
                                    }
                                }
                                KeyCode::Char('g') => {
                                    let selected_path = app
                                        .filebrowser_list
                                        .selected()
                                        .and_then(|i| {
                                            app.filebrowser_entries.get(i)
                                        })
                                        .filter(|e| e.git.is_some())
                                        .map(|e| e.path.clone());
                                    if let Some(path) = selected_path {
                                        app.git_open(path);
                                    }
                                }
                                KeyCode::Char('f') => {
                                    app.filebrowser_active = false
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('f') => {
                                    app.filebrowser_activate()
                                }
                                KeyCode::Char('i') => app.input_mode = true,
                                KeyCode::Char('p') => {
                                    app.publish_nip94 = !app.publish_nip94
                                }
                                KeyCode::Char('R') => {
                                    app.publish_relay_edit = true
                                }
                                KeyCode::Enter => app.start_upload(),
                                KeyCode::Esc => {
                                    app.upload_path.clear();
                                    app.upload_msg = None;
                                }
                                _ => {}
                            }
                        }
                    }
                    2 => {
                        // Git panel takes highest priority.
                        if app.git_mode {
                            match key.code {
                                KeyCode::Char('s') => {
                                    app.run_git_action(GitAction::Status)
                                }
                                KeyCode::Char('l') => {
                                    app.run_git_action(GitAction::Log)
                                }
                                KeyCode::Char('d') => {
                                    app.run_git_action(GitAction::Diff)
                                }
                                KeyCode::Char('f') => {
                                    app.run_git_action(GitAction::Fetch)
                                }
                                KeyCode::Char('p') => {
                                    app.run_git_action(GitAction::Pull)
                                }
                                KeyCode::Char('P') => {
                                    app.run_git_action(GitAction::Push)
                                }
                                KeyCode::Char('a') => {
                                    app.run_git_action(GitAction::Add)
                                }
                                KeyCode::Char('c') => {
                                    app.git_commit_edit = true;
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.git_scroll_up()
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    app.git_scroll_down(20)
                                }
                                KeyCode::Esc | KeyCode::Char('q') => {
                                    app.git_mode = false
                                }
                                _ => {}
                            }
                        } else if app.batch_filebrowser_active {
                            match key.code {
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.batch_filebrowser_scroll_up()
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    app.batch_filebrowser_scroll_down()
                                }
                                KeyCode::Enter => {
                                    app.batch_filebrowser_enter()
                                }
                                KeyCode::Backspace
                                | KeyCode::Char('h')
                                | KeyCode::Char('-')
                                | KeyCode::Esc => {
                                    if app
                                        .batch_filebrowser_cwd
                                        .parent()
                                        .is_some()
                                    {
                                        app.batch_filebrowser_parent();
                                    } else {
                                        app.batch_filebrowser_active =
                                            false;
                                    }
                                }
                                KeyCode::Char('g') => {
                                    let selected_path = app
                                        .batch_filebrowser_list
                                        .selected()
                                        .and_then(|i| {
                                            app.batch_filebrowser_entries
                                                .get(i)
                                        })
                                        .filter(|e| e.git.is_some())
                                        .map(|e| e.path.clone());
                                    if let Some(path) = selected_path {
                                        app.git_open(path);
                                    }
                                }
                                KeyCode::Char('f') => {
                                    app.batch_filebrowser_active = false
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('f') => {
                                    app.batch_filebrowser_activate()
                                }
                                KeyCode::Char('i') => {
                                    app.batch_input_mode = true
                                }
                                KeyCode::Enter => {
                                    app.start_batch_upload()
                                }
                                KeyCode::Char('x') => {
                                    app.remove_last_batch_item()
                                }
                                _ => {}
                            }
                        }
                    }
                    3 => {
                        if key.code == KeyCode::Char('r') {
                            app.refresh_admin();
                        }
                    }
                    4 => {
                        if key.code == KeyCode::Char('r') {
                            app.refresh_relay();
                        }
                    }
                    5 => {
                        if key.code == KeyCode::Char('r') {
                            app.refresh_nip96();
                        }
                    }
                    6 => match key.code {
                        KeyCode::Char('r') => app.nip34_relay_edit = true,
                        KeyCode::Char('c') => app.connect_nip34_relay(),
                        KeyCode::Up | KeyCode::Char('k') => {
                            let i = app.nip34_events_table.selected().unwrap_or(0);
                            if i > 0 {
                                app.nip34_events_table.select(Some(i - 1));
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let i = app.nip34_events_table.selected().unwrap_or(0);
                            let max = app.nip34_events.len().saturating_sub(1);
                            app.nip34_events_table.select(Some((i + 1).min(max)));
                        }
                        _ => {}
                    },
                    7 => {
                        if key.code == KeyCode::Char('r') {
                            app.refresh_status();
                        }
                    }
                    5 => {
                        // NIP-65 Relay List tab
                        if app.nip65_relay_edit {
                            match key.code {
                                KeyCode::Enter | KeyCode::Esc => {
                                    app.nip65_relay_edit = false;
                                }
                                KeyCode::Char(c) => {
                                    app.nip65_nostr_relay.push(c);
                                }
                                KeyCode::Backspace => {
                                    app.nip65_nostr_relay.pop();
                                }
                                _ => {}
                            }
                        } else if app.nip65_input_mode {
                            match key.code {
                                KeyCode::Enter => {
                                    let url =
                                        app.nip65_input.trim().to_string();
                                    if !url.is_empty() {
                                        let marker =
                                            app.nip65_marker.clone();
                                        app.nip65_relays
                                            .push((url, marker));
                                    }
                                    app.nip65_input.clear();
                                    app.nip65_input_mode = false;
                                }
                                KeyCode::Esc => {
                                    app.nip65_input.clear();
                                    app.nip65_input_mode = false;
                                }
                                KeyCode::Char(c) => {
                                    app.nip65_input.push(c);
                                }
                                KeyCode::Backspace => {
                                    app.nip65_input.pop();
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('a') => {
                                    app.nip65_input_mode = true;
                                    app.nip65_input.clear();
                                }
                                KeyCode::Char('d') | KeyCode::Delete => {
                                    let sel = app.nip65_selected;
                                    if sel < app.nip65_relays.len() {
                                        app.nip65_relays.remove(sel);
                                        if app.nip65_selected
                                            >= app.nip65_relays.len()
                                            && !app.nip65_relays.is_empty()
                                        {
                                            app.nip65_selected =
                                                app.nip65_relays.len() - 1;
                                        }
                                    }
                                }
                                KeyCode::Char('m') => {
                                    app.nip65_marker_idx =
                                        (app.nip65_marker_idx + 1) % 3;
                                    app.nip65_marker = match app
                                        .nip65_marker_idx
                                    {
                                        0 => "".into(),
                                        1 => "read".into(),
                                        _ => "write".into(),
                                    };
                                    // update selected relay marker
                                    let sel = app.nip65_selected;
                                    if let Some(r) =
                                        app.nip65_relays.get_mut(sel)
                                    {
                                        r.1 = app.nip65_marker.clone();
                                    }
                                }
                                KeyCode::Char('R') => {
                                    app.nip65_relay_edit = true;
                                }
                                KeyCode::Char('P') => {
                                    if let Some(sk) = &app.secret_key {
                                        let relays =
                                            app.nip65_relays.clone();
                                        match crate::nostr_sign::kind10002_relay_list(sk, &relays) {
                                            Ok(ev) => {
                                                app.notification = Some((
                                                    format!(
                                                        "Relay list event: {}…",
                                                        &ev["id"]
                                                            .as_str()
                                                            .unwrap_or("")[..8]
                                                    ),
                                                    false,
                                                ));
                                            }
                                            Err(e) => {
                                                app.notification = Some((
                                                    format!(
                                                        "Sign error: {e}"
                                                    ),
                                                    true,
                                                ));
                                            }
                                        }
                                    } else {
                                        app.notification = Some((
                                            "No key — go to Keygen first"
                                                .into(),
                                            true,
                                        ));
                                    }
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.nip65_selected =
                                        app.nip65_selected.saturating_sub(1);
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if !app.nip65_relays.is_empty() {
                                        app.nip65_selected = (app
                                            .nip65_selected
                                            + 1)
                                        .min(
                                            app.nip65_relays.len() - 1,
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    8 => {
                        // NIP-B7 Blossom Server List tab
                        if app.nipb7_relay_edit {
                            match key.code {
                                KeyCode::Enter | KeyCode::Esc => {
                                    app.nipb7_relay_edit = false;
                                }
                                KeyCode::Char(c) => {
                                    app.nipb7_nostr_relay.push(c);
                                }
                                KeyCode::Backspace => {
                                    app.nipb7_nostr_relay.pop();
                                }
                                _ => {}
                            }
                        } else if app.nipb7_input_mode {
                            match key.code {
                                KeyCode::Enter => {
                                    let url =
                                        app.nipb7_input.trim().to_string();
                                    if !url.is_empty() {
                                        app.nipb7_servers.push(url);
                                    }
                                    app.nipb7_input.clear();
                                    app.nipb7_input_mode = false;
                                }
                                KeyCode::Esc => {
                                    app.nipb7_input.clear();
                                    app.nipb7_input_mode = false;
                                }
                                KeyCode::Char(c) => {
                                    app.nipb7_input.push(c);
                                }
                                KeyCode::Backspace => {
                                    app.nipb7_input.pop();
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('a') => {
                                    app.nipb7_input_mode = true;
                                    app.nipb7_input.clear();
                                }
                                KeyCode::Char('d') | KeyCode::Delete => {
                                    let sel = app.nipb7_selected;
                                    if sel < app.nipb7_servers.len() {
                                        app.nipb7_servers.remove(sel);
                                        if app.nipb7_selected
                                            >= app.nipb7_servers.len()
                                            && !app.nipb7_servers.is_empty()
                                        {
                                            app.nipb7_selected =
                                                app.nipb7_servers.len() - 1;
                                        }
                                    }
                                }
                                KeyCode::Char('R') => {
                                    app.nipb7_relay_edit = true;
                                }
                                KeyCode::Char('P') => {
                                    if let Some(sk) = &app.secret_key {
                                        let servers =
                                            app.nipb7_servers.clone();
                                        match crate::nostr_sign::kind10063_server_list(sk, &servers) {
                                            Ok(ev) => {
                                                app.notification = Some((
                                                    format!(
                                                        "Server list event: {}…",
                                                        &ev["id"]
                                                            .as_str()
                                                            .unwrap_or("")[..8]
                                                    ),
                                                    false,
                                                ));
                                            }
                                            Err(e) => {
                                                app.notification = Some((
                                                    format!("Sign error: {e}"),
                                                    true,
                                                ));
                                            }
                                        }
                                    } else {
                                        app.notification = Some((
                                            "No key — go to Keygen first".into(),
                                            true,
                                        ));
                                    }
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.nipb7_selected =
                                        app.nipb7_selected.saturating_sub(1);
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if !app.nipb7_servers.is_empty() {
                                        app.nipb7_selected = (app
                                            .nipb7_selected
                                            + 1)
                                        .min(
                                            app.nipb7_servers.len() - 1,
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    10 => match key.code {
                        KeyCode::Char('g') => app.generate_keypair(),
                        KeyCode::Char('1') => app.copy_keygen_field(1),
                        KeyCode::Char('2') => app.copy_keygen_field(2),
                        KeyCode::Char('3') => app.copy_keygen_field(3),
                        KeyCode::Char('4') => app.copy_keygen_field(4),
                        _ => {}
                    },
                    11 => {
                        // Profile tab
                        if app.profile_relay_edit {
                            match key.code {
                                KeyCode::Enter | KeyCode::Esc => {
                                    app.profile_relay_edit = false;
                                }
                                KeyCode::Char(c) => {
                                    app.profile_nostr_relay.push(c);
                                }
                                KeyCode::Backspace => {
                                    app.profile_nostr_relay.pop();
                                }
                                _ => {}
                            }
                        } else if app.profile_editing {
                            match key.code {
                                KeyCode::Esc => {
                                    app.profile_editing = false;
                                }
                                KeyCode::Enter => {
                                    // Commit edit, advance to next field.
                                    app.profile_editing = false;
                                    app.profile_edit_field =
                                        (app.profile_edit_field + 1).min(5);
                                }
                                KeyCode::Char(c) => {
                                    match app.profile_edit_field {
                                        0 => app.profile_name.push(c),
                                        1 => app.profile_about.push(c),
                                        2 => app.profile_picture.push(c),
                                        3 => app.profile_nip05.push(c),
                                        4 => app.profile_website.push(c),
                                        5 => app.profile_lud16.push(c),
                                        _ => {}
                                    }
                                }
                                KeyCode::Backspace => {
                                    match app.profile_edit_field {
                                        0 => { app.profile_name.pop(); }
                                        1 => { app.profile_about.pop(); }
                                        2 => { app.profile_picture.pop(); }
                                        3 => { app.profile_nip05.pop(); }
                                        4 => { app.profile_website.pop(); }
                                        5 => { app.profile_lud16.pop(); }
                                        _ => {}
                                    }
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Up => {
                                    app.profile_edit_field =
                                        app.profile_edit_field
                                            .saturating_sub(1);
                                }
                                KeyCode::Down => {
                                    app.profile_edit_field =
                                        (app.profile_edit_field + 1).min(5);
                                }
                                KeyCode::Char('1') => app.profile_edit_field = 0,
                                KeyCode::Char('2') => app.profile_edit_field = 1,
                                KeyCode::Char('3') => app.profile_edit_field = 2,
                                KeyCode::Char('4') => app.profile_edit_field = 3,
                                KeyCode::Char('5') => app.profile_edit_field = 4,
                                KeyCode::Char('6') => app.profile_edit_field = 5,
                                KeyCode::Char('e') | KeyCode::Enter => {
                                    app.profile_editing = true;
                                }
                                KeyCode::Char('r') => {
                                    app.profile_relay_edit = true;
                                }
                                KeyCode::Char('P') => {
                                    // Publish kind:0
                                    if let Some(sk) = &app.secret_key {
                                        let mut meta =
                                            serde_json::Map::new();
                                        macro_rules! ins {
                                            ($k:expr, $v:expr) => {
                                                if !$v.is_empty() {
                                                    meta.insert(
                                                        $k.into(),
                                                        serde_json::Value::String($v.clone()),
                                                    );
                                                }
                                            };
                                        }
                                        ins!("name",    app.profile_name);
                                        ins!("about",   app.profile_about);
                                        ins!("picture", app.profile_picture);
                                        ins!("nip05",   app.profile_nip05);
                                        ins!("website", app.profile_website);
                                        ins!("lud16",   app.profile_lud16);
                                        match crate::nostr_sign::kind0_metadata(sk, &meta) {
                                            Ok(ev) => {
                                                app.notification = Some((
                                                    format!(
                                                        "Profile event id: {}",
                                                        &ev["id"].as_str().unwrap_or("")[..8]
                                                    ),
                                                    false,
                                                ));
                                            }
                                            Err(e) => {
                                                app.notification = Some((
                                                    format!("Sign error: {e}"),
                                                    true,
                                                ));
                                            }
                                        }
                                    } else {
                                        app.notification = Some((
                                            "No private key — go to Keygen tab first".into(),
                                            true,
                                        ));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

// ── Helpers
// ───────────────────────────────────────────────────────────────────

/// Format a Unix timestamp as a compact date string (YYYY-MM-DD HH:MM).
pub fn chrono_fmt_unix(ts: u64) -> String {
    // Simple manual formatting without chrono dependency
    let secs = ts as i64;
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hh = time_of_day / 3600;
    let mm = (time_of_day % 3600) / 60;
    // Approximate date calculation (good enough for display)
    let mut y = 1970i64;
    let mut d = days_since_epoch;
    loop {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let days_in_year = if leap { 366 } else { 365 };
        if d < days_in_year {
            break;
        }
        d -= days_in_year;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let months = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 0usize;
    for &dim in &months {
        if d < dim {
            break;
        }
        d -= dim;
        mo += 1;
    }
    format!("{y}-{:02}-{:02} {:02}:{:02}", mo + 1, d + 1, hh, mm)
}

/// Guess a MIME type from the file extension.
pub fn mime_from_path(path: &std::path::Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("pdf") => "application/pdf",
        Some("txt") | Some("md") => "text/plain",
        Some("json") => "application/json",
        Some("html") | Some("htm") => "text/html",
        _ => "application/octet-stream",
    }
    .into()
}

/// Human-readable byte size.
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else if bytes < GB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    }
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM`.
pub fn format_unix_ts(ts: u64) -> String {
    let secs = ts;
    let mins = secs / 60;
    let hours = mins / 60;
    let days_total = hours / 24;

    let minute = mins % 60;
    let hour = hours % 24;

    let (year, month, day) = days_to_ymd(days_total);

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Convert days since 1970-01-01 to (year, month, day).
pub fn days_to_ymd(d: u64) -> (u64, u64, u64) {
    // Algorithm from https://howardhinnant.github.io/date_algorithms.html
    let z = d + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

/// Encode a hex secret key as `nsec1` bech32.
/// Delegates to [`nip19::seckey_to_nsec`].
pub fn encode_nsec(hex_key: &str) -> Result<String, String> {
    nip19::seckey_to_nsec(hex_key).map_err(|e| e.to_string())
}

/// Decode a secret key from hex or `nsec1` bech32.
/// Delegates to [`nip19::nsec_to_seckey`] for bech32 input.
pub fn decode_secret_key(input: &str) -> Result<String, String> {
    if input.starts_with("nsec1") {
        nip19::nsec_to_seckey(input).map_err(|e| e.to_string())
    } else {
        if input.len() != 64 || !input.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("invalid hex key: expected 64 hex characters".into());
        }
        Ok(input.to_string())
    }
}

// ── Persistent state
// ──────────────────────────────────────────────────────────

/// User-facing configuration and UI preferences persisted between sessions.
///
/// All fields are `Option` so missing keys in a saved file are treated as
/// "not set" and fall back gracefully to env-vars or compiled defaults.
///
/// The file is written to `~/.config/blossom-tui/state.json` (respecting
/// `$XDG_CONFIG_HOME`) on clean exit and loaded on startup.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TuiState {
    /// Blossom server URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<String>,
    /// Secret key in hex (64 chars). Stored as-is; protect the file
    /// appropriately (mode 0600).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key: Option<String>,
    /// Last active tab index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tab: Option<usize>,
    /// Blob list sort preference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_field: Option<SortField>,
    /// Active blob filter string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_str: Option<String>,
    /// Whether to publish a NIP-94 event after upload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_nip94: Option<bool>,
    /// Relay URL used for NIP-94 publishing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publish_relay: Option<String>,
    /// NIP-34 relay URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nip34_relay: Option<String>,
}

impl App {
    /// Snapshot the persistent fields into a [`TuiState`].
    pub fn to_state(&self) -> TuiState {
        TuiState {
            server: Some(self.server.clone()),
            secret_key: self.secret_key.clone(),
            tab: Some(self.tab),
            sort_field: Some(self.sort_field),
            filter_str: if self.filter_str.is_empty() {
                None
            } else {
                Some(self.filter_str.clone())
            },
            publish_nip94: Some(self.publish_nip94),
            publish_relay: if self.publish_relay.is_empty() {
                None
            } else {
                Some(self.publish_relay.clone())
            },
            nip34_relay: if self.nip34_relay.is_empty() {
                None
            } else {
                Some(self.nip34_relay.clone())
            },
        }
    }

    /// Apply saved state fields that were not explicitly overridden by the
    /// caller. Call this right after `App::new` before the first render.
    pub fn apply_state(&mut self, state: &TuiState) {
        if let Some(t) = state.tab {
            self.tab = t.min(TAB_NAMES.len().saturating_sub(1));
        }
        if let Some(sf) = state.sort_field {
            self.sort_field = sf;
        }
        if let Some(f) = &state.filter_str {
            self.filter_str = f.clone();
        }
        if let Some(v) = state.publish_nip94 {
            self.publish_nip94 = v;
        }
        if let Some(r) = &state.publish_relay {
            self.publish_relay = r.clone();
        }
        if let Some(r) = &state.nip34_relay {
            self.nip34_relay = r.clone();
        }
    }
}

/// Return the path to the state file, honouring `$XDG_CONFIG_HOME`.
pub fn state_path() -> Option<PathBuf> {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .ok()?;
    Some(config_dir.join("blossom-tui").join("state.json"))
}

/// Load [`TuiState`] from disk. Returns a default (empty) state on any error.
pub fn load_state() -> TuiState {
    let Some(path) = state_path() else {
        return TuiState::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return TuiState::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Persist [`TuiState`] to disk, creating the config directory if needed.
pub fn save_state(state: &TuiState) -> Result<(), Box<dyn std::error::Error>> {
    let path = state_path().ok_or("cannot determine state file path")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(state)?;
    // Write to a temp file then rename for atomicity.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
