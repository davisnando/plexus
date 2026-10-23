use sha2::{Digest, Sha256};

/// Claude (and most clients) only accept `^[a-zA-Z0-9_-]{1,64}$` tool names.
pub const MAX_TOOL_NAME: usize = 64;
pub const SEPARATOR: &str = "__";
pub const MAX_ALIAS: usize = 24;

pub fn validate_alias(alias: &str) -> Result<(), String> {
    let mut chars = alias.chars();
    let valid = matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
        && alias.len() <= MAX_ALIAS
        && !alias.contains(SEPARATOR)
        && !alias.ends_with('_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "alias must be 1-{MAX_ALIAS} chars of a-z, 0-9 and single underscores, starting with a letter"
        ))
    }
}

pub fn validate_slug(slug: &str) -> Result<(), String> {
    let mut chars = slug.chars();
    let valid = matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
        && slug.len() <= 32;
    if valid {
        Ok(())
    } else {
        Err("slug must be 1-32 chars of a-z, 0-9 and dashes".into())
    }
}

/// `{alias}__{tool}`, sanitized and hash-truncated to fit [`MAX_TOOL_NAME`].
pub fn exposed_tool_name(alias: &str, tool: &str) -> String {
    let sanitized: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let full = format!("{alias}{SEPARATOR}{sanitized}");
    if full.len() <= MAX_TOOL_NAME && sanitized == tool {
        return full;
    }
    let digest = Sha256::digest(tool.as_bytes());
    let hash: String = digest[..3].iter().map(|b| format!("{b:02x}")).collect();
    let budget = MAX_TOOL_NAME - alias.len() - SEPARATOR.len() - hash.len() - 1;
    let keep = &sanitized[..sanitized.len().min(budget)];
    format!("{alias}{SEPARATOR}{keep}_{hash}")
}

pub fn resource_uri(alias: &str, upstream_uri: &str) -> String {
    let encoded =
        percent_encoding::utf8_percent_encode(upstream_uri, percent_encoding::NON_ALPHANUMERIC);
    format!("plexus://{alias}/{encoded}")
}

/// Inverse of [`resource_uri`]: `(alias, upstream uri)`.
pub fn parse_resource_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("plexus://")?;
    let (alias, encoded) = rest.split_once('/')?;
    let upstream = percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .ok()?
        .into_owned();
    Some((alias.to_string(), upstream))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases() {
        assert!(validate_alias("jira_acme").is_ok());
        assert!(validate_alias("sentry2").is_ok());
        assert!(validate_alias("Jira").is_err());
        assert!(validate_alias("jira__x").is_err());
        assert!(validate_alias("_jira").is_err());
        assert!(validate_alias("jira_").is_err());
        assert!(validate_alias("").is_err());
    }

    #[test]
    fn short_names_pass_through() {
        assert_eq!(
            exposed_tool_name("jira_acme", "create_issue"),
            "jira_acme__create_issue"
        );
    }

    #[test]
    fn long_and_odd_names_fit() {
        let long = "a".repeat(80);
        let name = exposed_tool_name("jira_acme", &long);
        assert_eq!(name.len(), MAX_TOOL_NAME);
        assert!(name.starts_with("jira_acme__aaa"));

        let dotted = exposed_tool_name("gh", "repos.list");
        assert!(dotted.starts_with("gh__repos_list_"));
        assert_ne!(dotted, exposed_tool_name("gh", "repos_list"));
    }

    #[test]
    fn resource_uri_roundtrip() {
        let uri = resource_uri("sentry_prod", "sentry://issue/123?x=y");
        assert_eq!(
            parse_resource_uri(&uri),
            Some(("sentry_prod".into(), "sentry://issue/123?x=y".into()))
        );
    }
}
