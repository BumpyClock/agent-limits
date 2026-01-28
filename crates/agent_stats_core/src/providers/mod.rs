use crate::config::{ProviderConfig, ProviderSourceMode};
use crate::models::{ProviderErrorPayload, ProviderPayload};
use crate::provider_metadata::{ProviderId, ProviderMetadata, provider_metadata};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use time::OffsetDateTime;

mod antigravity;
mod amp;
mod augment;
mod claude;
mod codex;
mod copilot;
mod cursor;
mod factory;
mod gemini;
mod jetbrains;
mod kimi;
mod kimik2;
mod kiro;
mod minimax;
mod opencode;
mod synthetic;
mod zai;
mod vertexai;

pub struct ProviderFetchContext {
    pub provider_id: ProviderId,
    pub provider_config: ProviderConfig,
    pub source_override: Option<ProviderSourceMode>,
    pub include_status: bool,
    pub now: OffsetDateTime,
}

impl ProviderFetchContext {
    pub fn source_mode(&self) -> ProviderSourceMode {
        self.source_override
            .clone()
            .or(self.provider_config.source.clone())
            .unwrap_or(ProviderSourceMode::Auto)
    }
}

pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn display_name(&self) -> &'static str;
    fn fetch(&self, context: &ProviderFetchContext) -> Result<ProviderPayload, ProviderErrorPayload>;
}

struct StubProvider {
    id: ProviderId,
    display_name: &'static str,
}

impl Provider for StubProvider {
    fn id(&self) -> ProviderId {
        self.id
    }

    fn display_name(&self) -> &'static str {
        self.display_name
    }

    fn fetch(&self, _context: &ProviderFetchContext) -> Result<ProviderPayload, ProviderErrorPayload> {
        Err(ProviderErrorPayload {
            code: 1,
            message: format!("{} provider is not ported yet.", self.display_name),
            kind: Some("provider".to_string()),
        })
    }
}

pub struct ProviderRegistry {
    providers: HashMap<ProviderId, Arc<dyn Provider>>,
    metadata: HashMap<ProviderId, ProviderMetadata>,
}

impl ProviderRegistry {
    pub fn provider(&self, id: ProviderId) -> Arc<dyn Provider> {
        self.providers
            .get(&id)
            .cloned()
            .unwrap_or_else(|| {
                let display_name = self
                    .metadata
                    .get(&id)
                    .map(|metadata| metadata.display_name)
                    .unwrap_or("Unknown");
                Arc::new(StubProvider { id, display_name })
            })
    }
}

static REGISTRY: OnceLock<ProviderRegistry> = OnceLock::new();

pub fn registry() -> &'static ProviderRegistry {
    REGISTRY.get_or_init(|| {
        let mut providers: HashMap<ProviderId, Arc<dyn Provider>> = HashMap::new();
        providers.insert(ProviderId::Antigravity, Arc::new(antigravity::AntigravityProvider));
        providers.insert(ProviderId::Amp, Arc::new(amp::AmpProvider));
        providers.insert(ProviderId::Augment, Arc::new(augment::AugmentProvider));
        providers.insert(ProviderId::Claude, Arc::new(claude::ClaudeProvider));
        providers.insert(ProviderId::Codex, Arc::new(codex::CodexProvider));
        providers.insert(ProviderId::Copilot, Arc::new(copilot::CopilotProvider));
        providers.insert(ProviderId::Cursor, Arc::new(cursor::CursorProvider));
        providers.insert(ProviderId::Factory, Arc::new(factory::FactoryProvider));
        providers.insert(ProviderId::Gemini, Arc::new(gemini::GeminiProvider));
        providers.insert(ProviderId::JetBrains, Arc::new(jetbrains::JetBrainsProvider));
        providers.insert(ProviderId::Kimi, Arc::new(kimi::KimiProvider));
        providers.insert(ProviderId::KimiK2, Arc::new(kimik2::KimiK2Provider));
        providers.insert(ProviderId::Kiro, Arc::new(kiro::KiroProvider));
        providers.insert(ProviderId::MiniMax, Arc::new(minimax::MiniMaxProvider));
        providers.insert(ProviderId::OpenCode, Arc::new(opencode::OpenCodeProvider));
        providers.insert(ProviderId::Synthetic, Arc::new(synthetic::SyntheticProvider));
        providers.insert(ProviderId::VertexAI, Arc::new(vertexai::VertexAIProvider));
        providers.insert(ProviderId::Zai, Arc::new(zai::ZaiProvider));

        let metadata = provider_metadata()
            .iter()
            .map(|metadata| (metadata.id, metadata.clone()))
            .collect();

        ProviderRegistry { providers, metadata }
    })
}
