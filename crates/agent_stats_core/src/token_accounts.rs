use crate::provider_metadata::ProviderId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenAccountInjection {
    CookieHeader,
    Environment { key: &'static str },
}

#[derive(Debug, Clone, Copy)]
pub struct TokenAccountSupport {
    pub title: &'static str,
    pub subtitle: &'static str,
    pub placeholder: &'static str,
    pub injection: TokenAccountInjection,
    pub requires_manual_cookie_source: bool,
    pub cookie_name: Option<&'static str>,
}

const CLAUDE_SUPPORT: TokenAccountSupport = TokenAccountSupport {
    title: "Session tokens",
    subtitle: "Store Claude sessionKey cookies or OAuth access tokens.",
    placeholder: "Paste sessionKey or OAuth token...",
    injection: TokenAccountInjection::CookieHeader,
    requires_manual_cookie_source: true,
    cookie_name: Some("sessionKey"),
};

const ZAI_SUPPORT: TokenAccountSupport = TokenAccountSupport {
    title: "API tokens",
    subtitle: "Stored in the Agent Stats config file.",
    placeholder: "Paste token...",
    injection: TokenAccountInjection::Environment { key: "Z_AI_API_KEY" },
    requires_manual_cookie_source: false,
    cookie_name: None,
};

const CURSOR_SUPPORT: TokenAccountSupport = TokenAccountSupport {
    title: "Session tokens",
    subtitle: "Store multiple Cursor Cookie headers.",
    placeholder: "Cookie: ...",
    injection: TokenAccountInjection::CookieHeader,
    requires_manual_cookie_source: true,
    cookie_name: None,
};

const OPENCODE_SUPPORT: TokenAccountSupport = TokenAccountSupport {
    title: "Session tokens",
    subtitle: "Store multiple OpenCode Cookie headers.",
    placeholder: "Cookie: ...",
    injection: TokenAccountInjection::CookieHeader,
    requires_manual_cookie_source: true,
    cookie_name: None,
};

const FACTORY_SUPPORT: TokenAccountSupport = TokenAccountSupport {
    title: "Session tokens",
    subtitle: "Store multiple Factory Cookie headers.",
    placeholder: "Cookie: ...",
    injection: TokenAccountInjection::CookieHeader,
    requires_manual_cookie_source: true,
    cookie_name: None,
};

const MINIMAX_SUPPORT: TokenAccountSupport = TokenAccountSupport {
    title: "Session tokens",
    subtitle: "Store multiple MiniMax Cookie headers.",
    placeholder: "Cookie: ...",
    injection: TokenAccountInjection::CookieHeader,
    requires_manual_cookie_source: true,
    cookie_name: None,
};

const AUGMENT_SUPPORT: TokenAccountSupport = TokenAccountSupport {
    title: "Session tokens",
    subtitle: "Store multiple Augment Cookie headers.",
    placeholder: "Cookie: ...",
    injection: TokenAccountInjection::CookieHeader,
    requires_manual_cookie_source: true,
    cookie_name: None,
};

pub fn token_account_support(provider_id: ProviderId) -> Option<&'static TokenAccountSupport> {
    match provider_id {
        ProviderId::Claude => Some(&CLAUDE_SUPPORT),
        ProviderId::Zai => Some(&ZAI_SUPPORT),
        ProviderId::Cursor => Some(&CURSOR_SUPPORT),
        ProviderId::OpenCode => Some(&OPENCODE_SUPPORT),
        ProviderId::Factory => Some(&FACTORY_SUPPORT),
        ProviderId::MiniMax => Some(&MINIMAX_SUPPORT),
        ProviderId::Augment => Some(&AUGMENT_SUPPORT),
        _ => None,
    }
}

pub fn normalized_cookie_header(token: &str, support: &TokenAccountSupport) -> String {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Some(cookie_name) = support.cookie_name else {
        return trimmed.to_string();
    };

    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("cookie:") || trimmed.contains('=') {
        return trimmed.to_string();
    }

    format!("{}={}", cookie_name, trimmed)
}

pub fn normalized_claude_oauth_token(token: &str) -> Option<String> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lower = trimmed.to_ascii_lowercase();
    const BEARER_PREFIX: &str = "bearer ";
    if lower.starts_with(BEARER_PREFIX) {
        let remainder = trimmed.get(BEARER_PREFIX.len()..).unwrap_or("");
        let remainder = remainder.trim();
        if remainder.is_empty() {
            return None;
        }
        return Some(remainder.to_string());
    }

    Some(trimmed.to_string())
}

pub fn is_claude_oauth_token(token: &str) -> bool {
    let Some(trimmed) = normalized_claude_oauth_token(token) else {
        return false;
    };
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("cookie:") || trimmed.contains('=') {
        return false;
    }
    lower.starts_with("sk-ant-oat")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalized_cookie_header_with_cookie_name() {
        let normalized = normalized_cookie_header("sk-ant-abc", &CLAUDE_SUPPORT);
        assert_eq!(normalized, "sessionKey=sk-ant-abc");

        let normalized = normalized_cookie_header("Cookie: sessionKey=sk-ant-abc", &CLAUDE_SUPPORT);
        assert_eq!(normalized, "Cookie: sessionKey=sk-ant-abc");
    }

    #[test]
    fn test_normalized_cookie_header_without_cookie_name() {
        let normalized = normalized_cookie_header(" Cookie: foo=bar ", &CURSOR_SUPPORT);
        assert_eq!(normalized, "Cookie: foo=bar");
    }

    #[test]
    fn test_is_claude_oauth_token_detection() {
        assert!(is_claude_oauth_token("sk-ant-oat-xyz"));
        assert!(is_claude_oauth_token("Bearer sk-ant-oat-xyz"));
        assert!(!is_claude_oauth_token("sessionKey=sk-ant-oat-xyz"));
        assert!(!is_claude_oauth_token("cookie: sessionKey=sk-ant-oat-xyz"));
    }
}
