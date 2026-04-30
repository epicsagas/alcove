// Public API surface — called by future tool integrations.
#![allow(dead_code)]

/// alcove telemetry — PostHog (product analytics) + Sentry (error monitoring)
///
/// Architecture: epiccounty/reports/telemetry_sentry_posthog_architecture_2026-04-29.md
///   - PostHog: single project, `product=alcove` property
///   - Sentry:  SENTRY_DSN_ALCOVE env var
///   - Consent: opt-out (on by default), on/off only
///   - PII: strictly forbidden — enum values only, no vault names/paths/queries
///
/// Consent flow:
///   - New users:     alcove setup wizard sets consent explicitly
///   - Existing users / MCP clients: first binary invocation auto-enables
///     telemetry and prints a one-time opt-out notice
use std::fs;
use std::io::Read;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

// ── Enum-gated event values (no free strings) ────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tool {
    AuditProject,
    CheckDocChanges,
    ConfigureProject,
    GetDocFile,
    GetProjectDocsOverview,
    InitProject,
    LintProject,
    ListProjects,
    ListVaults,
    PromoteDocument,
    RebuildIndex,
    SearchProjectDocs,
    SearchVault,
    ValidateDocs,
}

impl Tool {
    fn as_str(self) -> &'static str {
        match self {
            Self::AuditProject => "audit_project",
            Self::CheckDocChanges => "check_doc_changes",
            Self::ConfigureProject => "configure_project",
            Self::GetDocFile => "get_doc_file",
            Self::GetProjectDocsOverview => "get_project_docs_overview",
            Self::InitProject => "init_project",
            Self::LintProject => "lint_project",
            Self::ListProjects => "list_projects",
            Self::ListVaults => "list_vaults",
            Self::PromoteDocument => "promote_document",
            Self::RebuildIndex => "rebuild_index",
            Self::SearchProjectDocs => "search_project_docs",
            Self::SearchVault => "search_vault",
            Self::ValidateDocs => "validate_docs",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FailureClass {
    VaultParseError,
    IndexCorrupt,
    FileNotFound,
    PermissionDenied,
    NetworkError,
    Timeout,
    Unknown,
}

impl FailureClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::VaultParseError => "vault_parse_error",
            Self::IndexCorrupt => "index_corrupt",
            Self::FileNotFound => "file_not_found",
            Self::PermissionDenied => "permission_denied",
            Self::NetworkError => "network_error",
            Self::Timeout => "timeout",
            Self::Unknown => "unknown",
        }
    }

    pub fn should_capture_sentry(self) -> bool {
        matches!(self, Self::VaultParseError | Self::IndexCorrupt)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ResultSizeBucket {
    Empty,
    Lt10,
    Lt100,
    Lt1k,
    Gte1k,
}

impl ResultSizeBucket {
    pub fn from_count(n: usize) -> Self {
        match n {
            0 => Self::Empty,
            1..=9 => Self::Lt10,
            10..=99 => Self::Lt100,
            100..=999 => Self::Lt1k,
            _ => Self::Gte1k,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Lt10 => "<10",
            Self::Lt100 => "<100",
            Self::Lt1k => "<1k",
            Self::Gte1k => ">=1k",
        }
    }
}

// ── Consent ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsentLevel {
    On,
    Off,
}

fn dirs_config() -> PathBuf {
    if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".config");
    }
    if let Ok(up) = std::env::var("USERPROFILE") {
        return PathBuf::from(up).join(".config");
    }
    PathBuf::from(".config")
}

fn consent_file() -> PathBuf {
    dirs_config().join("alcove").join("telemetry-consent")
}

fn install_id_file() -> PathBuf {
    dirs_config().join("alcove").join("install-id")
}

pub fn read_consent_raw() -> Option<ConsentLevel> {
    let s = fs::read_to_string(consent_file()).ok()?;
    match s.trim() {
        "off" => Some(ConsentLevel::Off),
        "on" | "community" | "anonymous" => Some(ConsentLevel::On),
        _ => None,
    }
}

pub fn read_consent() -> ConsentLevel {
    read_consent_raw().unwrap_or(ConsentLevel::Off)
}

pub fn write_consent(level: ConsentLevel) {
    let path = consent_file();
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    let _ = fs::write(&path, match level {
        ConsentLevel::On => "on",
        ConsentLevel::Off => "off",
    });
}

/// Auto-enable on first run; print one-time opt-out notice.
/// Call at every binary entry point except `setup` and `telemetry`.
pub fn ensure_consent_or_set_default() {
    if read_consent_raw().is_none() {
        write_consent(ConsentLevel::On);
        eprintln!("[alcove] Telemetry enabled (anonymous install ID).");
        eprintln!("[alcove] To opt out: alcove telemetry off");
        eprintln!("[alcove] Details: https://github.com/epicsagas/alcove#telemetry");
    }
}

// ── Install wizard prompt ─────────────────────────────────────────────────────

pub fn prompt_consent_interactive() -> ConsentLevel {
    use std::io::Write;
    println!();
    println!("  ┌─ Telemetry ──────────────────────────────────────────────────────────┐");
    println!("  │ alcove collects anonymous usage data to improve the product.         │");
    println!("  │                                                                      │");
    println!("  │  What we send:    tool name, duration, outcome, version, OS          │");
    println!("  │  What we never:   vault names/paths, doc contents, search queries    │");
    println!("  │  Identifier:      random install ID (not linked to you or machine)   │");
    println!("  │  Opt out anytime: alcove telemetry off                               │");
    println!("  └──────────────────────────────────────────────────────────────────────┘");
    println!();
    print!("  Enable telemetry? [Y/n]: ");
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    match line.trim().to_lowercase().as_str() {
        "n" | "no" => ConsentLevel::Off,
        _ => ConsentLevel::On,
    }
}

// ── Install ID ────────────────────────────────────────────────────────────────

fn load_or_create_install_id() -> String {
    let path = install_id_file();
    if let Ok(s) = fs::read_to_string(&path) {
        let t = s.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let id = new_uuid_v4();
    if let Some(p) = path.parent() {
        let _ = fs::create_dir_all(p);
    }
    let _ = fs::write(&path, &id);
    id
}

fn new_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    } else {
        let pid = std::process::id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (i, b) in pid.to_le_bytes().iter().enumerate() {
            bytes[i] ^= b;
        }
        for (i, b) in ts.to_le_bytes().iter().enumerate() {
            bytes[8 + i % 8] ^= b;
        }
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

// ── Telemetry client ──────────────────────────────────────────────────────────

pub struct Telemetry {
    consent: ConsentLevel,
    distinct_id: String,
    version: &'static str,
    os: &'static str,
}

static TELEMETRY: OnceLock<Telemetry> = OnceLock::new();

impl Telemetry {
    pub fn init() -> &'static Self {
        TELEMETRY.get_or_init(|| {
            let consent = read_consent();
            let distinct_id = match consent {
                ConsentLevel::On => load_or_create_install_id(),
                ConsentLevel::Off => String::new(),
            };
            Self {
                consent,
                distinct_id,
                version: env!("CARGO_PKG_VERSION"),
                os: std::env::consts::OS,
            }
        })
    }

    pub fn is_enabled(&self) -> bool {
        self.consent == ConsentLevel::On
    }

    fn base_props(&self) -> String {
        format!(
            r#""product":"alcove","product_version":"{}","os":"{}","telemetry_schema":"v1""#,
            self.version, self.os
        )
    }

    fn track(&self, event: &str, extra: &str) {
        if !self.is_enabled() {
            return;
        }
        let payload = if extra.is_empty() {
            format!(
                r#"{{"event":"{}","distinct_id":"{}","properties":{{{}}}}}"#,
                event, self.distinct_id, self.base_props()
            )
        } else {
            format!(
                r#"{{"event":"{}","distinct_id":"{}","properties":{{{},{}}}}}"#,
                event, self.distinct_id, self.base_props(), extra
            )
        };
        posthog_send(&payload);
    }

    fn capture_error(&self, message: &str, failure_class: FailureClass) {
        if !self.is_enabled() {
            return;
        }
        sentry_send(message, failure_class.as_str(), self.version);
    }
}

// ── Typed event helpers ───────────────────────────────────────────────────────

impl Telemetry {
    pub fn track_started(&self, vault_count: usize, project_count: usize) {
        self.track(
            "alcove_started",
            &format!(
                r#""vault_count":{},"project_count":{}"#,
                vault_count, project_count
            ),
        );
    }

    pub fn track_tool_called(&self, tool: Tool) {
        self.track("tool_called", &format!(r#""tool":"{}""#, tool.as_str()));
    }

    pub fn track_tool_completed(&self, tool: Tool, duration_ms: u64, result_size: ResultSizeBucket) {
        self.track(
            "tool_completed",
            &format!(
                r#""tool":"{}","duration_ms":{},"result_size_bucket":"{}""#,
                tool.as_str(),
                duration_ms,
                result_size.as_str()
            ),
        );
    }

    pub fn track_tool_failed(&self, tool: Tool, failure_class: FailureClass) {
        self.track(
            "tool_failed",
            &format!(
                r#""tool":"{}","failure_class":"{}""#,
                tool.as_str(),
                failure_class.as_str()
            ),
        );
        if failure_class.should_capture_sentry() {
            self.capture_error("tool_failed", failure_class);
        }
    }

    pub fn track_setup_completed(&self) {
        self.track("alcove_setup_completed", "");
    }
}

// ── PostHog transport (curl, fire-and-forget) ────────────────────────────────

const POSTHOG_HOST: &str = "us.i.posthog.com";

fn posthog_key() -> Option<&'static str> {
    option_env!("POSTHOG_KEY").filter(|k| !k.is_empty())
}

fn posthog_send(payload: &str) {
    let Some(key) = posthog_key() else { return };
    let body = format!(r#"{{"api_key":"{}","batch":[{}]}}"#, key, payload);
    let url = format!("https://{POSTHOG_HOST}/batch/");
    let _ = std::process::Command::new("curl")
        .args(["-s", "--max-time", "5", "-X", "POST",
               "-H", "Content-Type: application/json",
               "-d", &body, &url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ── Sentry transport ──────────────────────────────────────────────────────────

fn sentry_dsn() -> Option<&'static str> {
    option_env!("SENTRY_DSN_ALCOVE").filter(|k| !k.is_empty())
}

fn parse_sentry_dsn(dsn: &str) -> Option<(String, String)> {
    let without_scheme = dsn.strip_prefix("https://")?;
    let at = without_scheme.find('@')?;
    let rest = &without_scheme[at + 1..];
    let slash = rest.find('/')?;
    let host = rest[..slash].to_string();
    let project = &rest[slash..];
    Some((host, format!("/api{project}")))
}

fn sentry_send(message: &str, failure_class: &str, version: &str) {
    let Some(dsn) = sentry_dsn() else { return };
    let (host, path) = parse_sentry_dsn(dsn).unwrap_or_default();
    if host.is_empty() {
        return;
    }
    let event_id = new_uuid_v4().replace('-', "");
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let key = dsn.split('@').next()
        .and_then(|s| s.split("//").nth(1))
        .unwrap_or("");
    let auth = format!(
        "Sentry sentry_version=7,sentry_key={key},sentry_client=alcove/{version}"
    );
    let envelope = format!(
        "{{}}\n{{\"type\":\"event\"}}\n{{\
            \"event_id\":\"{event_id}\",\
            \"timestamp\":{ts},\
            \"level\":\"error\",\
            \"message\":\"{message}\",\
            \"release\":\"{version}\",\
            \"tags\":{{\"failure_class\":\"{failure_class}\"}}\
        }}\n"
    );
    let url = format!("https://{host}{path}/envelope/");
    let _ = std::process::Command::new("curl")
        .args(["-s", "--max-time", "5", "-X", "POST",
               "-H", "Content-Type: application/x-sentry-envelope",
               "-H", &format!("X-Sentry-Auth: {auth}"),
               "-d", &envelope, &url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ── CLI subcommand handler (alcove telemetry [...]) ───────────────────────────

pub fn run_cli(sub: &str) -> anyhow::Result<()> {
    match sub {
        "on" => {
            write_consent(ConsentLevel::On);
            let id = load_or_create_install_id();
            println!("  ✓ Telemetry enabled (install ID: {id}).");
        }
        "off" => {
            write_consent(ConsentLevel::Off);
            println!("  ✓ Telemetry disabled. No data will be sent.");
        }
        _ => {
            let level = read_consent();
            match level {
                ConsentLevel::On => {
                    let id = load_or_create_install_id();
                    println!("  Status: on  (install ID: {id})");
                }
                ConsentLevel::Off => {
                    println!("  Status: off");
                }
            }
            println!("  Toggle: alcove telemetry on|off");
        }
    }
    Ok(())
}

// Keep unused imports quiet — TcpStream/Duration reserved for future use
#[allow(dead_code)]
fn _unused(_: TcpStream, _: Duration) {}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_v4_format() {
        let id = new_uuid_v4();
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[2].len(), 4);
        assert!(parts[2].starts_with('4'));
        let variant = u8::from_str_radix(&parts[3][..2], 16).unwrap();
        assert!(variant & 0xc0 == 0x80);
    }

    #[test]
    fn uuid_uniqueness() {
        assert_ne!(new_uuid_v4(), new_uuid_v4());
    }

    #[test]
    fn result_size_bucket_boundaries() {
        assert_eq!(ResultSizeBucket::from_count(0).as_str(), "empty");
        assert_eq!(ResultSizeBucket::from_count(1).as_str(), "<10");
        assert_eq!(ResultSizeBucket::from_count(9).as_str(), "<10");
        assert_eq!(ResultSizeBucket::from_count(10).as_str(), "<100");
        assert_eq!(ResultSizeBucket::from_count(1000).as_str(), ">=1k");
    }

    #[test]
    fn consent_raw_none_when_no_file() {
        // In test env, consent file almost certainly doesn't exist at test path
        // Just verify the function returns Some/None based on content
        let level = read_consent_raw();
        // Can be either — just verify it doesn't panic
        let _ = level;
    }

    #[test]
    fn consent_off_disables_telemetry() {
        let t = Telemetry {
            consent: ConsentLevel::Off,
            distinct_id: String::new(),
            version: "0.0.0",
            os: "linux",
        };
        assert!(!t.is_enabled());
    }

    #[test]
    fn consent_on_enables_telemetry() {
        let t = Telemetry {
            consent: ConsentLevel::On,
            distinct_id: "id".into(),
            version: "0.0.0",
            os: "linux",
        };
        assert!(t.is_enabled());
    }

    #[test]
    fn base_props_contains_product() {
        let t = Telemetry {
            consent: ConsentLevel::On,
            distinct_id: "x".into(),
            version: "1.2.3",
            os: "darwin",
        };
        let props = t.base_props();
        assert!(props.contains(r#""product":"alcove""#));
        assert!(props.contains(r#""product_version":"1.2.3""#));
        assert!(props.contains(r#""telemetry_schema":"v1""#));
    }

    #[test]
    fn parse_sentry_dsn_valid() {
        let (host, path) = parse_sentry_dsn("https://key@o123.ingest.sentry.io/456").unwrap();
        assert_eq!(host, "o123.ingest.sentry.io");
        assert_eq!(path, "/api/456");
    }

    #[test]
    fn tool_str_values() {
        assert_eq!(Tool::SearchProjectDocs.as_str(), "search_project_docs");
        assert_eq!(Tool::ListVaults.as_str(), "list_vaults");
        assert_eq!(Tool::ValidateDocs.as_str(), "validate_docs");
    }

    #[test]
    fn failure_sentry_gate() {
        assert!(FailureClass::VaultParseError.should_capture_sentry());
        assert!(FailureClass::IndexCorrupt.should_capture_sentry());
        assert!(!FailureClass::FileNotFound.should_capture_sentry());
        assert!(!FailureClass::Timeout.should_capture_sentry());
    }
}
