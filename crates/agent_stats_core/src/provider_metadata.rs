use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
    Cursor,
    #[serde(rename = "opencode")]
    OpenCode,
    Factory,
    Gemini,
    Antigravity,
    Copilot,
    #[serde(rename = "zai")]
    Zai,
    #[serde(rename = "minimax")]
    MiniMax,
    Kimi,
    Kiro,
    #[serde(rename = "vertexai")]
    VertexAI,
    Augment,
    #[serde(rename = "jetbrains")]
    JetBrains,
    #[serde(rename = "kimik2")]
    KimiK2,
    Amp,
    Synthetic,
}

impl ProviderId {
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Codex => "Codex",
            ProviderId::Claude => "Claude",
            ProviderId::Cursor => "Cursor",
            ProviderId::OpenCode => "OpenCode",
            ProviderId::Factory => "Factory",
            ProviderId::Gemini => "Gemini",
            ProviderId::Antigravity => "Antigravity",
            ProviderId::Copilot => "Copilot",
            ProviderId::Zai => "z.ai",
            ProviderId::MiniMax => "MiniMax",
            ProviderId::Kimi => "Kimi",
            ProviderId::Kiro => "Kiro",
            ProviderId::VertexAI => "Vertex AI",
            ProviderId::Augment => "Augment",
            ProviderId::JetBrains => "JetBrains AI",
            ProviderId::KimiK2 => "Kimi K2",
            ProviderId::Amp => "Amp",
            ProviderId::Synthetic => "Synthetic",
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            ProviderId::Codex => "codex",
            ProviderId::Claude => "claude",
            ProviderId::Cursor => "cursor",
            ProviderId::OpenCode => "opencode",
            ProviderId::Factory => "factory",
            ProviderId::Gemini => "gemini",
            ProviderId::Antigravity => "antigravity",
            ProviderId::Copilot => "copilot",
            ProviderId::Zai => "zai",
            ProviderId::MiniMax => "minimax",
            ProviderId::Kimi => "kimi",
            ProviderId::Kiro => "kiro",
            ProviderId::VertexAI => "vertexai",
            ProviderId::Augment => "augment",
            ProviderId::JetBrains => "jetbrains",
            ProviderId::KimiK2 => "kimik2",
            ProviderId::Amp => "amp",
            ProviderId::Synthetic => "synthetic",
        };
        formatter.write_str(value)
    }
}

impl FromStr for ProviderId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codex" => Ok(ProviderId::Codex),
            "claude" => Ok(ProviderId::Claude),
            "cursor" => Ok(ProviderId::Cursor),
            "opencode" => Ok(ProviderId::OpenCode),
            "factory" => Ok(ProviderId::Factory),
            "gemini" => Ok(ProviderId::Gemini),
            "antigravity" => Ok(ProviderId::Antigravity),
            "copilot" => Ok(ProviderId::Copilot),
            "zai" => Ok(ProviderId::Zai),
            "minimax" => Ok(ProviderId::MiniMax),
            "kimi" => Ok(ProviderId::Kimi),
            "kiro" => Ok(ProviderId::Kiro),
            "vertexai" => Ok(ProviderId::VertexAI),
            "augment" => Ok(ProviderId::Augment),
            "jetbrains" => Ok(ProviderId::JetBrains),
            "kimik2" => Ok(ProviderId::KimiK2),
            "amp" => Ok(ProviderId::Amp),
            "synthetic" => Ok(ProviderId::Synthetic),
            other => Err(format!("Unknown provider id: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProviderMetadata {
    pub id: ProviderId,
    pub display_name: &'static str,
    pub default_enabled: bool,
}

static PROVIDERS: Lazy<Vec<ProviderMetadata>> = Lazy::new(|| {
    vec![
        ProviderMetadata { id: ProviderId::Codex, display_name: "Codex", default_enabled: true },
        ProviderMetadata { id: ProviderId::Claude, display_name: "Claude", default_enabled: true },
        ProviderMetadata { id: ProviderId::Cursor, display_name: "Cursor", default_enabled: true },
        ProviderMetadata { id: ProviderId::OpenCode, display_name: "OpenCode", default_enabled: false },
        ProviderMetadata { id: ProviderId::Factory, display_name: "Factory", default_enabled: false },
        ProviderMetadata { id: ProviderId::Gemini, display_name: "Gemini", default_enabled: true },
        ProviderMetadata { id: ProviderId::Antigravity, display_name: "Antigravity", default_enabled: false },
        ProviderMetadata { id: ProviderId::Copilot, display_name: "Copilot", default_enabled: false },
        ProviderMetadata { id: ProviderId::Zai, display_name: "z.ai", default_enabled: false },
        ProviderMetadata { id: ProviderId::MiniMax, display_name: "MiniMax", default_enabled: false },
        ProviderMetadata { id: ProviderId::Kimi, display_name: "Kimi", default_enabled: false },
        ProviderMetadata { id: ProviderId::Kiro, display_name: "Kiro", default_enabled: false },
        ProviderMetadata { id: ProviderId::VertexAI, display_name: "Vertex AI", default_enabled: false },
        ProviderMetadata { id: ProviderId::Augment, display_name: "Augment", default_enabled: false },
        ProviderMetadata { id: ProviderId::JetBrains, display_name: "JetBrains AI", default_enabled: false },
        ProviderMetadata { id: ProviderId::KimiK2, display_name: "Kimi K2", default_enabled: false },
        ProviderMetadata { id: ProviderId::Amp, display_name: "Amp", default_enabled: false },
        ProviderMetadata { id: ProviderId::Synthetic, display_name: "Synthetic", default_enabled: false },
    ]
});

pub fn provider_metadata() -> &'static [ProviderMetadata] {
    &PROVIDERS
}

pub fn provider_dashboard_url(provider_id: ProviderId) -> Option<&'static str> {
    match provider_id {
        ProviderId::Codex => Some("https://chatgpt.com/codex/settings/usage"),
        ProviderId::Claude => Some("https://console.anthropic.com/settings/billing"),
        ProviderId::Cursor => Some("https://cursor.com/dashboard?tab=usage"),
        ProviderId::OpenCode => Some("https://opencode.ai"),
        ProviderId::Factory => Some("https://app.factory.ai/settings/billing"),
        ProviderId::Gemini => Some("https://gemini.google.com"),
        ProviderId::Antigravity => None,
        ProviderId::Copilot => Some("https://github.com/settings/copilot"),
        ProviderId::Zai => Some("https://z.ai/manage-apikey/subscription"),
        ProviderId::MiniMax => {
            Some("https://platform.minimax.io/user-center/payment/coding-plan?cycle_type=3")
        }
        ProviderId::Kimi => Some("https://www.kimi.com/code/console"),
        ProviderId::Kiro => Some("https://app.kiro.dev/account/usage"),
        ProviderId::VertexAI => Some("https://console.cloud.google.com/vertex-ai"),
        ProviderId::Augment => Some("https://app.augmentcode.com/account/subscription"),
        ProviderId::JetBrains => None,
        ProviderId::KimiK2 => Some("https://kimi-k2.ai/my-credits"),
        ProviderId::Amp => Some("https://ampcode.com/settings"),
        ProviderId::Synthetic => None,
    }
}

pub fn provider_status_url(provider_id: ProviderId) -> Option<&'static str> {
    match provider_id {
        ProviderId::Codex => Some("https://status.openai.com/"),
        ProviderId::Claude => Some("https://status.claude.com/"),
        ProviderId::Cursor => Some("https://status.cursor.com"),
        ProviderId::Factory => Some("https://status.factory.ai"),
        ProviderId::Copilot => Some("https://www.githubstatus.com/"),
        _ => None,
    }
}

pub fn provider_icon_path(provider_id: ProviderId) -> &'static str {
    match provider_id {
        ProviderId::Codex => "providers/ProviderIcon-codex.svg",
        ProviderId::Claude => "providers/ProviderIcon-claude.svg",
        ProviderId::Cursor => "providers/ProviderIcon-cursor.svg",
        ProviderId::OpenCode => "providers/ProviderIcon-opencode.svg",
        ProviderId::Factory => "providers/ProviderIcon-factory.svg",
        ProviderId::Gemini => "providers/ProviderIcon-gemini.svg",
        ProviderId::Antigravity => "providers/ProviderIcon-antigravity.svg",
        ProviderId::Copilot => "providers/ProviderIcon-copilot.svg",
        ProviderId::Zai => "providers/ProviderIcon-zai.svg",
        ProviderId::MiniMax => "providers/ProviderIcon-minimax.svg",
        ProviderId::Kimi => "providers/ProviderIcon-kimi.svg",
        ProviderId::Kiro => "providers/ProviderIcon-kiro.svg",
        ProviderId::VertexAI => "providers/ProviderIcon-vertexai.svg",
        ProviderId::Augment => "providers/ProviderIcon-augment.svg",
        ProviderId::JetBrains => "providers/ProviderIcon-jetbrains.svg",
        ProviderId::KimiK2 => "providers/ProviderIcon-kimi.svg",
        ProviderId::Amp => "providers/ProviderIcon-amp.svg",
        ProviderId::Synthetic => "providers/ProviderIcon-synthetic.svg",
    }
}
