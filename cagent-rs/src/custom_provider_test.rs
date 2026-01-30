use std::env;

use crate::config::Config;
use crate::model::{create_provider_from_parts, ProviderError, ProviderOptions};

#[test]
fn custom_provider_uses_base_url_and_token_key() {
    // Ensure we don't depend on external env.
    env::remove_var("CUSTOM_API_KEY");
    env::set_var("CUSTOM_API_KEY", "test-token");

    let yaml = r#"
version: "3"
providers:
  myopenai:
    api_type: openai
    base_url: http://localhost:1234/v1/chat/completions
    token_key: CUSTOM_API_KEY
models:
  mymodel:
    provider: myopenai
    model: gpt-4o-mini
agents:
  root:
    model: mymodel
"#;

    let cfg = Config::from_yaml(yaml).unwrap();
    let model_cfg = cfg.models.get("mymodel").unwrap();

    let provider = create_provider_from_parts(
        &model_cfg.provider,
        &model_cfg.model,
        ProviderOptions::default(),
        cfg.providers.get(&model_cfg.provider),
    )
    .unwrap();

    assert_eq!(provider.id(), "myopenai/gpt-4o-mini");
}

#[test]
fn custom_provider_requires_token_env_var() {
    env::remove_var("CUSTOM_API_KEY_MISSING");

    let yaml = r#"
version: "3"
providers:
  myopenai:
    api_type: openai
    base_url: http://localhost:1234/v1/chat/completions
    token_key: CUSTOM_API_KEY_MISSING
models:
  mymodel:
    provider: myopenai
    model: gpt-4o-mini
agents:
  root:
    model: mymodel
"#;

    let cfg = Config::from_yaml(yaml).unwrap();
    let model_cfg = cfg.models.get("mymodel").unwrap();

    let err = create_provider_from_parts(
        &model_cfg.provider,
        &model_cfg.model,
        ProviderOptions::default(),
        cfg.providers.get(&model_cfg.provider),
    )
    .unwrap_err();

    assert!(matches!(err, ProviderError::MissingApiKey(_)));
}
