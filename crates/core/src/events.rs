use serde::Serialize;

/// Events pushed to the UI (over server-sent events) as things happen.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Progress {
        task: String,
        current: u64,
        total: u64,
        message: String,
    },
    /// A line of output from a running game or dev client.
    Log { source: String, line: String },
    ProcessStarted { source: String },
    ProcessExited { source: String, code: Option<i32> },
    InstancesChanged,
    ModsChanged { instance_id: String },
    ProjectsChanged,
    AccountsChanged,
    /// Settings were saved.
    ConfigChanged,
    LoginCode {
        user_code: String,
        verification_uri: String,
        message: String,
    },
    LoginFailed { error: String },
    BuildFinished {
        project_id: String,
        success: bool,
        message: String,
    },
}
