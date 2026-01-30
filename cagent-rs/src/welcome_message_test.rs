use crate::config::Config;

#[test]
fn config_parses_welcome_message() {
    let yaml = r#"
version: "3"
agents:
  root:
    model: openai/gpt-4o
    welcome_message: "Hello there"
"#;

    let cfg = Config::from_yaml(yaml).unwrap();
    assert_eq!(
        cfg.agents["root"].welcome_message.as_deref(),
        Some("Hello there")
    );
}
