//! Plugin-free agent activity from terminal screens (Herdr-style).
//!
//! For each tmux pane running a recognized agent *without* a verified
//! extension binding, the worker captures the live bottom of the pane
//! (`tmux capture-pane`, read-only) and matches it against a small ordered
//! rule manifest. The result is `working | idle | blocked | unknown`.
//!
//! This is inference from visible text, not proof. Rules are fixed
//! substrings (no model, no regex crate). `blocked` is deliberately
//! strict: only known approval/question markers count. Anything unmatched
//! falls back per Herdr: `idle` for known agents, `unknown` for Codex
//! without a title signal.
//!
//! Manifests are bundled TOML (`assets/activity/<agent>.toml`, embedded
//! with `include_str!`) with optional local overrides in
//! `~/.config/agent-monitor/agent-detection/<agent>.toml`. Overrides win;
//! invalid files are ignored with a warning. No network fetches, ever.
//!
//! Captured screen text is matched in memory and dropped: never logged,
//! never persisted. Activity is ephemeral per presence tick.

use crate::{model::Provider, paths};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Inferred pane activity. `Unknown` is a normal outcome, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivity {
    Working,
    Idle,
    Blocked,
    Unknown,
}

impl AgentActivity {
    /// Lowercase state word for display, Herdr-style.
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}

/// Why a classification came out the way it did. Travels with the result
/// so `agent explain` (and the detail line) can show provenance instead of
/// asking the user to trust a guess.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MatchEvidence {
    /// Rule id that matched, or empty on fallback.
    pub rule_id: String,
    /// `bundled` or `override`.
    pub manifest_source: String,
    pub manifest_version: u64,
    /// Set when no rule matched (e.g. `default_known_agent_idle_fallback`).
    pub fallback_reason: String,
    /// Whether the terminal title contributed a signal (Codex only).
    pub title_signal: bool,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    manifest_version: u64,
    #[serde(default)]
    rule: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
struct Rule {
    id: String,
    state: AgentActivity,
    #[serde(default)]
    title_any: Vec<String>,
    #[serde(default)]
    screen_any: Vec<String>,
    /// All must appear somewhere in the tail.
    #[serde(default)]
    screen_all: Vec<String>,
    /// None may appear in the tail.
    #[serde(default)]
    screen_none: Vec<String>,
    /// Only the last N lines count for this rule (0 = whole tail).
    #[serde(default)]
    bottom_lines: usize,
}

const CODEX: &str = include_str!("../assets/activity/codex.toml");
const CLAUDE: &str = include_str!("../assets/activity/claude.toml");
const OPENCODE: &str = include_str!("../assets/activity/opencode.toml");
const PI: &str = include_str!("../assets/activity/pi.toml");

/// Spinner/activity glyphs some agents put in the tmux pane title.
const SPINNER_GLYPHS: &[char] = &[
    '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '◐', '◓', '◑', '◒', '…',
];

fn bundled(provider: Provider) -> (&'static str, &'static str) {
    match provider {
        Provider::Codex => (CODEX, "codex"),
        Provider::Claude => (CLAUDE, "claude"),
        Provider::Opencode => (OPENCODE, "opencode"),
        Provider::Pi => (PI, "pi"),
    }
}

/// Load the active manifest: local override wins, bundled is the fallback.
/// An invalid override is ignored (warning returned) — never fatal.
fn load(provider: Provider) -> (Manifest, String, u64, Option<String>) {
    let (text, name) = bundled(provider);
    if let Some(path) = override_path(name)
        && let Ok(raw) = std::fs::read_to_string(&path)
    {
        match toml::from_str::<Manifest>(&raw) {
            Ok(manifest) => {
                let version = manifest.manifest_version;
                return (manifest, "override".into(), version, None);
            }
            Err(e) => {
                let fallback = bundled_manifest(text);
                let version = fallback.manifest_version;
                return (
                    fallback,
                    "bundled".into(),
                    version,
                    Some(format!(
                        "Ignoring invalid override {}: {}",
                        path.display(),
                        paths::line(&e.to_string())
                    )),
                );
            }
        }
    }
    let manifest = bundled_manifest(text);
    let version = manifest.manifest_version;
    (manifest, "bundled".into(), version, None)
}

fn bundled_manifest(text: &str) -> Manifest {
    toml::from_str(text).expect("bundled activity manifest must parse")
}

/// Local override path, if the file exists. Parent dirs are created 0700;
/// symlinks are refused; missing file means "no override".
fn override_path(agent: &str) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = home.join(".config/agent-monitor/agent-detection");
    std::fs::create_dir_all(&dir).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(format!("{agent}.toml"));
    if path.is_file() && !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink())
    {
        Some(path)
    } else {
        None
    }
}

/// Classify one pane snapshot. `title` is the tmux pane title (Codex
/// spinner signal); `tail` is the sanitized `capture-pane` output.
/// Pure function: no I/O except the manifest load, fully testable.
pub fn classify(provider: Provider, title: &str, tail: &str) -> (AgentActivity, MatchEvidence) {
    classify_with_override(provider, title, tail, None)
}

/// Same as `classify` with an injectable manifest TOML for tests and
/// `agent explain --file`. `None` loads the active manifest.
pub fn classify_with_override(
    provider: Provider,
    title: &str,
    tail: &str,
    manifest_toml: Option<&str>,
) -> (AgentActivity, MatchEvidence) {
    let (manifest, source, version, warning) = match manifest_toml {
        Some(raw) => match toml::from_str::<Manifest>(raw) {
            Ok(manifest) => {
                let version = manifest.manifest_version;
                (manifest, "override".to_string(), version, None)
            }
            Err(e) => {
                let (text, _) = bundled(provider);
                let fallback = bundled_manifest(text);
                let version = fallback.manifest_version;
                (
                    fallback,
                    "bundled".to_string(),
                    version,
                    Some(format!("invalid manifest: {}", paths::line(&e.to_string()))),
                )
            }
        },
        None => load(provider),
    };
    let _ = warning;

    let bottom = |n: usize| -> &str {
        if n == 0 {
            return tail;
        }
        let lines: Vec<&str> = tail.lines().collect();
        let start = lines.len().saturating_sub(n);
        let offset: usize = lines[..start].iter().map(|l| l.len() + 1).sum();
        &tail[offset.min(tail.len())..]
    };

    let title_hit = |needles: &[String]| needles.iter().any(|n| title.contains(n.as_str()));

    for rule in &manifest.rule {
        if !rule.title_any.is_empty() && !title_hit(&rule.title_any) {
            continue;
        }
        let scope = bottom(rule.bottom_lines);
        if !rule.screen_any.is_empty()
            && !rule.screen_any.iter().any(|n| scope.contains(n.as_str()))
        {
            continue;
        }
        if !rule.screen_all.iter().all(|n| scope.contains(n.as_str())) {
            continue;
        }
        if rule.screen_none.iter().any(|n| scope.contains(n.as_str())) {
            continue;
        }
        return (
            rule.state,
            MatchEvidence {
                rule_id: rule.id.clone(),
                manifest_source: source,
                manifest_version: version,
                fallback_reason: String::new(),
                title_signal: false,
            },
        );
    }

    // No rule matched. Codex consults the terminal title: a spinner means
    // working, a plain title means idle, no title means unknown.
    if provider == Provider::Codex {
        if SPINNER_GLYPHS.iter().any(|g| title.contains(*g)) {
            return (
                AgentActivity::Working,
                MatchEvidence {
                    rule_id: "codex-title-spinner".into(),
                    manifest_source: source,
                    manifest_version: version,
                    fallback_reason: String::new(),
                    title_signal: true,
                },
            );
        }
        if !title.trim().is_empty() {
            return (
                AgentActivity::Idle,
                MatchEvidence {
                    manifest_source: source,
                    manifest_version: version,
                    fallback_reason: "codex-title-without-spinner".into(),
                    title_signal: true,
                    ..Default::default()
                },
            );
        }
        return (
            AgentActivity::Unknown,
            MatchEvidence {
                manifest_source: source,
                manifest_version: version,
                fallback_reason: "no-rule-matched-no-title".into(),
                ..Default::default()
            },
        );
    }

    // Herdr parity: known non-Codex agents fall back to idle, labeled.
    (
        AgentActivity::Idle,
        MatchEvidence {
            manifest_source: source,
            manifest_version: version,
            fallback_reason: "default_known_agent_idle_fallback".into(),
            ..Default::default()
        },
    )
}

/// Human line for the detail panel / CLI, always with provenance.
pub fn describe(provider: Provider, activity: AgentActivity, evidence: &MatchEvidence) -> String {
    let agent = match provider {
        Provider::Pi => "pi",
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Opencode => "opencode",
    };
    if evidence.rule_id.is_empty() {
        format!(
            "{agent} · {} ({}; {} manifest v{})",
            activity.label(),
            evidence.fallback_reason,
            evidence.manifest_source,
            evidence.manifest_version
        )
    } else {
        format!(
            "{agent} · {} (rule {}; {} manifest v{})",
            activity.label(),
            evidence.rule_id,
            evidence.manifest_source,
            evidence.manifest_version
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_MANIFEST: &str = r#"
manifest_version = 7
[[rule]]
id = "approval"
state = "blocked"
screen_any = ["Approve", "(y/n)"]
[[rule]]
id = "spinner"
state = "working"
screen_any = ["Working", "⠼"]
screen_none = ["error"]
[[rule]]
id = "scoped-tail"
state = "working"
screen_any = ["done-marker"]
bottom_lines = 3
"#;

    #[test]
    fn ordered_rules_win_and_blocked_is_strict() {
        // Approval marker present: blocked, even with a spinner too.
        let (state, evidence) = classify_with_override(
            Provider::Codex,
            "",
            "⠼ Working\nApprove? (y/n)",
            Some(TEST_MANIFEST),
        );
        assert_eq!(state, AgentActivity::Blocked);
        assert_eq!(evidence.rule_id, "approval");

        // Spinner alone: working.
        let (state, evidence) =
            classify_with_override(Provider::Codex, "", "⠼ Working on it", Some(TEST_MANIFEST));
        assert_eq!(state, AgentActivity::Working);
        assert_eq!(evidence.rule_id, "spinner");

        // Novel prompt shape is NOT blocked (strict): Codex without title
        // falls to unknown, others to the idle fallback.
        let (state, _) = classify_with_override(
            Provider::Codex,
            "",
            "Something needs your okay-ish confirmation",
            Some(TEST_MANIFEST),
        );
        assert_eq!(state, AgentActivity::Unknown);
        let (state, evidence) = classify_with_override(
            Provider::Claude,
            "",
            "Something needs your okay-ish confirmation",
            Some(TEST_MANIFEST),
        );
        assert_eq!(state, AgentActivity::Idle);
        assert_eq!(
            evidence.fallback_reason,
            "default_known_agent_idle_fallback"
        );

        // screen_none vetoes the match.
        let (state, _) =
            classify_with_override(Provider::Codex, "", "Working error", Some(TEST_MANIFEST));
        assert_eq!(state, AgentActivity::Unknown);
    }

    #[test]
    fn bottom_lines_scopes_the_match() {
        let tail = "done-marker\nfiller\nfiller\nfiller\nfiller";
        let (state, _) = classify_with_override(Provider::Codex, "", tail, Some(TEST_MANIFEST));
        // Marker is 5 lines up: outside the last-3 scope, no title either.
        assert_eq!(state, AgentActivity::Unknown);
        let tail = "filler\nfiller\ndone-marker";
        let (state, evidence) =
            classify_with_override(Provider::Codex, "", tail, Some(TEST_MANIFEST));
        assert_eq!(state, AgentActivity::Working);
        assert_eq!(evidence.rule_id, "scoped-tail");
    }

    #[test]
    fn codex_title_signal_matrix() {
        // Spinner in title: working even with an empty screen.
        let (state, evidence) =
            classify_with_override(Provider::Codex, "⠼ codex", "", Some(TEST_MANIFEST));
        assert_eq!(state, AgentActivity::Working);
        assert!(evidence.title_signal);
        // Plain title: idle.
        let (state, _) = classify_with_override(Provider::Codex, "codex", "", Some(TEST_MANIFEST));
        assert_eq!(state, AgentActivity::Idle);
        // No title, no rule: unknown.
        let (state, _) = classify_with_override(Provider::Codex, "", "", Some(TEST_MANIFEST));
        assert_eq!(state, AgentActivity::Unknown);
    }

    #[test]
    fn invalid_manifest_falls_back_to_bundled() {
        let (state, evidence) =
            classify_with_override(Provider::Codex, "", "⠼ Working", Some("not = [valid"));
        assert_eq!(state, AgentActivity::Working);
        assert_eq!(evidence.manifest_source, "bundled");
    }

    #[test]
    fn bundled_manifests_all_parse() {
        for provider in [
            Provider::Pi,
            Provider::Claude,
            Provider::Codex,
            Provider::Opencode,
        ] {
            let (text, _) = bundled(provider);
            let manifest = bundled_manifest(text);
            assert!(manifest.manifest_version >= 1);
            assert!(!manifest.rule.is_empty());
        }
    }

    #[test]
    fn describe_always_shows_provenance() {
        let (_, evidence) =
            classify_with_override(Provider::Codex, "", "⠼ Working", Some(TEST_MANIFEST));
        let line = describe(Provider::Codex, AgentActivity::Working, &evidence);
        assert!(line.contains("rule spinner"));
        assert!(line.contains("manifest v7"));
    }
}
