//! Folder rules that decide which connections a client session gets, based on
//! the folders (MCP roots) the client reports.
//!
//! A rule covers its folder and everything below it: `/work/acme`,
//! `/work/acme/` and `/work/acme/*` are the same rule. `*` and `?` match within
//! one path segment (`/work/client-*`), and `**` matches any number of segments
//! (`/work/**/frontend`). A leading `~` is the home directory.

use std::path::{Component, Path, PathBuf};

/// Splits a rule into normalized segments, or `None` if it is not an absolute path.
fn rule_segments(rule: &str) -> Option<Vec<String>> {
    let rule = rule.trim();
    let expanded = match rule.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{rest}", std::env::var("HOME").ok()?)
        }
        _ => rule.to_string(),
    };
    if !expanded.starts_with('/') {
        return None;
    }
    let mut segments: Vec<String> = expanded
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(String::from)
        .collect();
    // A trailing wildcard adds nothing: the folder itself and everything below already match.
    while segments.last().is_some_and(|s| s == "*" || s == "**") {
        segments.pop();
    }
    // Resolve symlinks (e.g. /tmp → /private/tmp) in the literal leading part.
    let literal = segments
        .iter()
        .take_while(|s| !s.contains(['*', '?']))
        .count();
    if literal > 0 {
        let prefix: PathBuf = std::iter::once("/".to_string())
            .chain(segments[..literal].iter().cloned())
            .collect();
        if let Ok(real) = std::fs::canonicalize(&prefix) {
            let mut resolved = path_segments(&real);
            resolved.extend(segments.drain(literal..));
            segments = resolved;
        }
    }
    Some(segments)
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

/// macOS and Windows file systems are case-insensitive by default.
fn same(a: &str, b: &str) -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn same_char(a: char, b: char) -> bool {
    if cfg!(any(target_os = "macos", windows)) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

/// `*` and `?` glob within one segment.
fn segment_matches(pattern: &str, text: &str) -> bool {
    if !pattern.contains(['*', '?']) {
        return same(pattern, text);
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || same_char(p[pi], t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Whether the rule's segments match a leading part of the path's segments.
fn prefix_matches(rule: &[String], path: &[String]) -> bool {
    match rule.split_first() {
        None => true,
        Some((first, rest)) if first == "**" => {
            (0..=path.len()).any(|skip| prefix_matches(rest, &path[skip..]))
        }
        Some((first, rest)) => path
            .split_first()
            .is_some_and(|(p, path)| segment_matches(first, p) && prefix_matches(rest, path)),
    }
}

/// Whether `folder` is covered by `rule`.
pub fn rule_matches(rule: &str, folder: &Path) -> bool {
    let Some(rule) = rule_segments(rule) else {
        return false;
    };
    if prefix_matches(&rule, &path_segments(folder)) {
        return true;
    }
    std::fs::canonicalize(folder).is_ok_and(|real| prefix_matches(&rule, &path_segments(&real)))
}

/// Checks a rule entered in the UI.
pub fn validate_rule(rule: &str) -> Result<(), String> {
    if rule_segments(rule).is_some() {
        Ok(())
    } else {
        Err(format!("folder rule `{rule}` must be an absolute path, like /Users/you/projects/app"))
    }
}

/// The local folder of a `file://` root URI.
pub fn root_path(uri: &str) -> Option<PathBuf> {
    reqwest::Url::parse(uri).ok()?.to_file_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(rule: &str, folder: &str) -> bool {
        rule_matches(rule, Path::new(folder))
    }

    #[test]
    fn folder_and_below() {
        for rule in ["/test-folder/testing", "/test-folder/testing/", "/test-folder/testing/*"] {
            assert!(m(rule, "/test-folder/testing"), "{rule}");
            assert!(m(rule, "/test-folder/testing/packages/api"), "{rule}");
            assert!(!m(rule, "/test-folder/other"), "{rule}");
            assert!(!m(rule, "/test-folder/testing-2"), "{rule}");
        }
        assert!(m("/test-folder/*", "/test-folder"));
        assert!(m("/test-folder/**", "/test-folder/x/y"));
        assert!(!m("/test-folder/*", "/elsewhere"));
    }

    #[test]
    fn wildcards() {
        assert!(m("/work/client-*", "/work/client-acme/app"));
        assert!(!m("/work/client-*", "/work/internal"));
        assert!(m("/work/*/frontend", "/work/acme/frontend/src"));
        assert!(!m("/work/*/frontend", "/work/acme/backend"));
        assert!(m("/work/**/frontend", "/work/a/b/frontend"));
        assert!(m("/work/app-?", "/work/app-1"));
    }

    #[test]
    fn rejects_relative_rules() {
        assert!(validate_rule("projects/app").is_err());
        assert!(validate_rule("/projects/app").is_ok());
        assert!(!m("projects", "/projects"));
    }

    #[test]
    fn root_uris() {
        assert_eq!(
            root_path("file:///Users/me/My%20Project"),
            Some(PathBuf::from("/Users/me/My Project"))
        );
        assert_eq!(root_path("https://example.com"), None);
    }
}
