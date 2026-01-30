use std::fs;

use crate::config::Config;

fn load_config_for_test(path: &str) -> anyhow::Result<Config> {
    if path.is_empty() {
        return Ok(Config::default_agent());
    }

    let p = std::path::Path::new(path);

    if p.is_dir() {
        let candidate_yaml = p.join("agent.yaml");
        let candidate_yml = p.join("agent.yml");

        if candidate_yaml.is_file() {
            return Ok(Config::load(candidate_yaml)?);
        }
        if candidate_yml.is_file() {
            return Ok(Config::load(candidate_yml)?);
        }

        anyhow::bail!(
            "{} is a directory but contains no agent.yaml or agent.yml",
            p.display()
        );
    }

    Ok(Config::load(p)?)
}

#[test]
fn load_config_accepts_directory_with_agent_yaml() {
    let dir = tempfile::tempdir().unwrap();

    let yaml = r#"version: \"3\"
agents:
  root:
    model: openai/gpt-4o
"#;

    fs::write(dir.path().join("agent.yaml"), yaml).unwrap();

    let cfg = load_config_for_test(dir.path().to_string_lossy().as_ref()).unwrap();
    assert!(cfg.agents.contains_key("root"));
}

#[test]
fn load_config_accepts_directory_with_agent_yml() {
    let dir = tempfile::tempdir().unwrap();

    let yaml = r#"version: \"3\"
agents:
  root:
    model: openai/gpt-4o
"#;

    fs::write(dir.path().join("agent.yml"), yaml).unwrap();

    let cfg = load_config_for_test(dir.path().to_string_lossy().as_ref()).unwrap();
    assert!(cfg.agents.contains_key("root"));
}

#[test]
fn load_config_errors_on_directory_without_agent_yaml() {
    let dir = tempfile::tempdir().unwrap();

    let err = load_config_for_test(dir.path().to_string_lossy().as_ref()).unwrap_err();
    assert!(
        err.to_string()
            .contains("contains no agent.yaml or agent.yml"),
        "unexpected error: {err:?}"
    );
}
