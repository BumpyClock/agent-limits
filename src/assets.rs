use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

pub struct Assets;

const PROVIDER_ICON_PATHS: &[&str] = &[
    "providers/ProviderIcon-amp.svg",
    "providers/ProviderIcon-antigravity.svg",
    "providers/ProviderIcon-augment.svg",
    "providers/ProviderIcon-claude.svg",
    "providers/ProviderIcon-codex.svg",
    "providers/ProviderIcon-copilot.svg",
    "providers/ProviderIcon-cursor.svg",
    "providers/ProviderIcon-factory.svg",
    "providers/ProviderIcon-gemini.svg",
    "providers/ProviderIcon-jetbrains.svg",
    "providers/ProviderIcon-kimi.svg",
    "providers/ProviderIcon-kiro.svg",
    "providers/ProviderIcon-minimax.svg",
    "providers/ProviderIcon-opencode.svg",
    "providers/ProviderIcon-synthetic.svg",
    "providers/ProviderIcon-vertexai.svg",
    "providers/ProviderIcon-zai.svg",
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &[u8] = match path {
            "providers/ProviderIcon-amp.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-amp.svg")[..]
            }
            "providers/ProviderIcon-antigravity.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-antigravity.svg")[..]
            }
            "providers/ProviderIcon-augment.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-augment.svg")[..]
            }
            "providers/ProviderIcon-claude.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-claude.svg")[..]
            }
            "providers/ProviderIcon-codex.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-codex.svg")[..]
            }
            "providers/ProviderIcon-copilot.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-copilot.svg")[..]
            }
            "providers/ProviderIcon-cursor.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-cursor.svg")[..]
            }
            "providers/ProviderIcon-factory.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-factory.svg")[..]
            }
            "providers/ProviderIcon-gemini.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-gemini.svg")[..]
            }
            "providers/ProviderIcon-jetbrains.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-jetbrains.svg")[..]
            }
            "providers/ProviderIcon-kimi.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-kimi.svg")[..]
            }
            "providers/ProviderIcon-kiro.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-kiro.svg")[..]
            }
            "providers/ProviderIcon-minimax.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-minimax.svg")[..]
            }
            "providers/ProviderIcon-opencode.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-opencode.svg")[..]
            }
            "providers/ProviderIcon-synthetic.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-synthetic.svg")[..]
            }
            "providers/ProviderIcon-vertexai.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-vertexai.svg")[..]
            }
            "providers/ProviderIcon-zai.svg" => {
                &include_bytes!("../assets/providers/ProviderIcon-zai.svg")[..]
            }
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(PROVIDER_ICON_PATHS
            .iter()
            .filter(|entry| entry.starts_with(path))
            .map(|entry| SharedString::from(*entry))
            .collect())
    }
}
