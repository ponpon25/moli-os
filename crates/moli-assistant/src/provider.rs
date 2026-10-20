//! The AI providers a person can choose from in the dashboard. They all speak
//! (more or less) the OpenAI protocol, so a provider is a preset: where it is,
//! which models Moli asks, and what it can do besides chatting.
//!
//! `moli.toml`'s `[assistant]` stays the house's own choice (a local Ollama,
//! other models): it is the provider named « custom », or one of these when
//! its `base_url` is theirs, and its models win there.

use crate::Config;
use crate::llm::Endpoint;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub base_url: &'static str,
    /// The model that talks (tools, short answers).
    pub model: &'static str,
    /// The one that drafts automations (rare, so a stronger one is affordable).
    pub builder_model: &'static str,
    /// Speech to text; empty when the provider does not offer it.
    pub transcribe_model: &'static str,
    /// Text to speech in the cloud (OpenAI's voices).
    pub speech: bool,
    /// Questions about the world answered from the web (`/responses`).
    pub search: bool,
    /// Where a person gets a key.
    pub keys_url: &'static str,
}

pub(crate) const PRESETS: [Preset; 4] = [
    Preset {
        id: "openai",
        name: "OpenAI",
        base_url: "https://api.openai.com/v1",
        model: "gpt-4.1-mini",
        builder_model: "gpt-4.1",
        transcribe_model: "gpt-4o-mini-transcribe",
        speech: true,
        search: true,
        keys_url: "https://platform.openai.com/api-keys",
    },
    Preset {
        id: "mistral",
        name: "Mistral",
        base_url: "https://api.mistral.ai/v1",
        model: "mistral-medium-latest",
        builder_model: "mistral-large-latest",
        transcribe_model: "voxtral-mini-latest",
        speech: false,
        search: false,
        keys_url: "https://console.mistral.ai/api-keys",
    },
    Preset {
        id: "gemini",
        name: "Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        model: "gemini-2.5-flash",
        builder_model: "gemini-2.5-pro",
        transcribe_model: "",
        speech: false,
        search: false,
        keys_url: "https://aistudio.google.com/apikey",
    },
    Preset {
        id: "anthropic",
        name: "Claude",
        base_url: "https://api.anthropic.com/v1",
        model: "claude-sonnet-5-5",
        builder_model: "claude-opus-5-5",
        transcribe_model: "",
        speech: false,
        search: false,
        keys_url: "https://console.anthropic.com/settings/keys",
    },
];

/// The provider whose preset `id` is.
pub(crate) fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// The id `moli.toml` is: a preset's when its `base_url` is that preset's,
/// else `custom`.
pub(crate) fn config_id(config: &Config) -> &'static str {
    PRESETS
        .iter()
        .find(|p| same_url(p.base_url, &config.base_url))
        .map_or("custom", |p| p.id)
}

/// Where a provider's key is filed in the vault: `api_key` for the one
/// `moli.toml` names (keys filed before the choice existed stay valid),
/// `api_key_<id>` for the others.
pub(crate) fn secret_name(id: &str, config: &Config) -> String {
    if id == config_id(config) {
        "api_key".into()
    } else {
        format!("api_key_{id}")
    }
}

/// What the assistant talks to now.
#[derive(Clone, Debug)]
pub(crate) struct Active {
    pub id: String,
    pub endpoint: Endpoint,
    pub model: String,
    pub builder_model: String,
    pub search_model: String,
    pub transcribe_model: String,
    pub speech_model: String,
}

impl Active {
    /// `moli.toml` as it is.
    pub(crate) fn from_config(config: &Config) -> anyhow::Result<Self> {
        Ok(Self {
            id: config_id(config).to_owned(),
            endpoint: Endpoint::parse(&config.base_url)?,
            model: config.model.clone(),
            builder_model: config.builder_model.clone(),
            search_model: config.search_model.clone(),
            transcribe_model: config.transcribe_model.clone(),
            speech_model: config.speech_model.clone(),
        })
    }

    /// A preset; `moli.toml` wins when it is that provider.
    pub(crate) fn from_preset(p: &Preset, config: &Config) -> anyhow::Result<Self> {
        if config_id(config) == p.id {
            return Self::from_config(config);
        }
        Ok(Self {
            id: p.id.to_owned(),
            endpoint: Endpoint::parse(p.base_url)?,
            model: p.model.to_owned(),
            builder_model: p.builder_model.to_owned(),
            search_model: if p.search {
                p.model.to_owned()
            } else {
                String::new()
            },
            transcribe_model: p.transcribe_model.to_owned(),
            speech_model: if p.speech {
                config.speech_model.clone()
            } else {
                String::new()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(base_url: &str) -> Config {
        serde_json::from_value(serde_json::json!({ "base_url": base_url })).unwrap()
    }

    #[test]
    fn the_house_config_is_a_preset_or_custom() {
        assert_eq!(config_id(&config("https://api.openai.com/v1")), "openai");
        assert_eq!(config_id(&config("https://api.mistral.ai/v1/")), "mistral");
        assert_eq!(config_id(&config("http://192.168.1.5:11434/v1")), "custom");
    }

    #[test]
    fn a_key_filed_before_the_choice_is_still_the_houses() {
        let c = config("https://api.openai.com/v1");
        assert_eq!(secret_name("openai", &c), "api_key");
        assert_eq!(secret_name("mistral", &c), "api_key_mistral");
        let local = config("http://x:11434/v1");
        assert_eq!(secret_name("custom", &local), "api_key");
        assert_eq!(secret_name("openai", &local), "api_key_openai");
    }

    #[test]
    fn every_preset_is_a_valid_https_endpoint_with_models() {
        for p in PRESETS {
            let e = Endpoint::parse(p.base_url).unwrap();
            assert!(!e.is_local(), "{}", p.id);
            assert!(
                !p.model.is_empty() && !p.builder_model.is_empty(),
                "{}",
                p.id
            );
            assert!(p.keys_url.starts_with("https://"), "{}", p.id);
        }
        let ids: std::collections::HashSet<_> = PRESETS.iter().map(|p| p.id).collect();
        assert_eq!(ids.len(), PRESETS.len());
    }

    #[test]
    fn a_provider_without_a_feature_switches_it_off() {
        let c = config("https://api.openai.com/v1");
        let mistral = Active::from_preset(preset("mistral").unwrap(), &c).unwrap();
        assert!(mistral.speech_model.is_empty() && mistral.search_model.is_empty());
        assert_eq!(mistral.transcribe_model, "voxtral-mini-latest");
        // OpenAI is the house's own: moli.toml's models stay.
        let openai = Active::from_preset(preset("openai").unwrap(), &c).unwrap();
        assert_eq!(openai.model, c.model);
        assert!(!openai.speech_model.is_empty());
    }
}
