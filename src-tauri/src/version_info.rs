//! The two version numbers a tester can read off the app: the release label (what we told them
//! to install, e.g. `v0.1.0-rc12`) and the git commit it was built from.
//!
//! The release label exists because the installer's numeric version cannot carry `-rcN`, so
//! `Cargo.toml`'s version alone cannot tell rc10 from rc11. Every field falls back to a literal
//! (`dev`, `unknown`) rather than an empty string: a blank in a bug report looks like a bug in
//! the report.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    /// Numeric version from the package manifest (what the MSI/NSIS metadata carries).
    pub app_version: String,
    /// The tag users were given (`v0.1.0-rc12`), or `dev` for a local build.
    pub release_label: String,
    pub git_sha: String,
    pub git_sha_short: String,
    /// True when the build came from a working tree with uncommitted changes.
    pub dirty: bool,
    /// Seconds since the Unix epoch when the binary was compiled, 0 if unknown.
    pub build_time: u64,
}

impl VersionInfo {
    pub fn from_parts(
        app_version: Option<&str>,
        release_tag: Option<&str>,
        git_sha: Option<&str>,
        dirty: Option<&str>,
        build_time: Option<&str>,
    ) -> Self {
        let clean = |v: Option<&str>| v.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
        let sha = clean(git_sha).unwrap_or_else(|| "unknown".to_string());
        let short: String = sha.chars().take(7).collect();
        VersionInfo {
            app_version: clean(app_version).unwrap_or_else(|| "unknown".to_string()),
            release_label: clean(release_tag).unwrap_or_else(|| "dev".to_string()),
            git_sha: sha,
            git_sha_short: short,
            dirty: dirty.map(|d| d.trim() == "true").unwrap_or(false),
            build_time: build_time.and_then(|t| t.trim().parse().ok()).unwrap_or(0),
        }
    }

    pub fn current() -> Self {
        Self::from_parts(
            Some(env!("CARGO_PKG_VERSION")),
            option_env!("PLUGABLE_CHAT_RELEASE_TAG"),
            option_env!("PLUGABLE_CHAT_GIT_SHA"),
            option_env!("PLUGABLE_CHAT_GIT_DIRTY"),
            option_env!("PLUGABLE_CHAT_BUILD_TIME"),
        )
    }

    /// `v0.1.0-rc12 (a1b2c3d)`; a dirty local build says so.
    pub fn display(&self) -> String {
        let dirty = if self.dirty { "+dirty" } else { "" };
        format!("{} ({}{})", self.release_label, self.git_sha_short, dirty)
    }

    pub fn window_title(&self) -> String {
        format!("Plugable Chat {} - Microsoft Foundry", self.display())
    }

    /// Multi-line block for crash logs, support text and `--version`.
    pub fn report_lines(&self) -> String {
        format!(
            "Release: {}\nApp version: {}\nGit commit: {}{}\nBuilt (unix time): {}",
            self.release_label,
            self.app_version,
            self.git_sha,
            if self.dirty { " (uncommitted changes)" } else { "" },
            self.build_time
        )
    }
}

/// `--version` output. clap wants a `'static` string, so the text is built once and kept.
pub fn cli_version() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        let v = VersionInfo::current();
        format!("{}\n{}", v.display(), v.report_lines())
    })
}

#[tauri::command]
pub fn get_version_info() -> VersionInfo {
    VersionInfo::current()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_candidates_are_distinguishable() {
        let a = VersionInfo::from_parts(Some("0.1.0"), Some("v0.1.0-rc10"), Some("70c9277c218d"), None, None);
        let b = VersionInfo::from_parts(Some("0.1.0"), Some("v0.1.0-rc11"), Some("a2199159d07b"), None, None);
        assert_eq!(a.app_version, b.app_version);
        assert_ne!(a.window_title(), b.window_title());
        assert!(a.window_title().contains("v0.1.0-rc10"));
        assert!(a.window_title().contains("70c9277"));
    }

    #[test]
    fn missing_or_blank_fields_never_render_empty() {
        let v = VersionInfo::from_parts(None, Some("  "), Some(""), None, Some("garbage"));
        assert_eq!(v.release_label, "dev");
        assert_eq!(v.git_sha, "unknown");
        assert_eq!(v.app_version, "unknown");
        assert_eq!(v.build_time, 0);
        assert!(!v.display().contains("()"));
    }

    #[test]
    fn dirty_builds_say_so() {
        let v = VersionInfo::from_parts(Some("0.1.0"), None, Some("abcdef1234"), Some("true"), None);
        assert!(v.display().contains("+dirty"));
        assert!(v.report_lines().contains("uncommitted"));
    }
}
