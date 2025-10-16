use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent_command: String,
    pub agent_args: Vec<String>,
    pub merge_target: String,
    pub template_editor: String,
    pub agent_display_name: String,
    pub worktree_base_override: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent_command: "codex".to_string(),
            agent_args: vec!["{template_content}".to_string()],
            merge_target: "main".to_string(),
            template_editor: "vim".to_string(),
            agent_display_name: "Codex".to_string(),
            worktree_base_override: None,
        }
    }
}

pub struct ConfigState {
    pub config: Config,
    pub templates_dir: PathBuf,
}

impl ConfigState {
    pub fn load() -> Result<Self> {
        let project_dirs = ProjectDirs::from("dev", "AgentManager", "AgentManager")
            .context("Unable to locate the user configuration directory")?;
        let config_dir = project_dirs.config_dir();
        fs::create_dir_all(config_dir).with_context(|| {
            format!("Unable to create configuration directory {:?}", config_dir)
        })?;

        let config_file = config_dir.join("config.toml");
        let config = if config_file.exists() {
            let mut buf = String::new();
            File::open(&config_file)?.read_to_string(&mut buf)?;
            if buf.trim().is_empty() {
                Config::default()
            } else {
                toml::from_str(&buf).context("Configuration file is invalid")?
            }
        } else {
            let config = Config::default();
            write_config(&config_file, &config)?;
            config
        };

        let templates_dir = config_dir.join("templates");
        fs::create_dir_all(&templates_dir)
            .with_context(|| format!("Unable to create templates directory {:?}", templates_dir))?;

        ensure_default_template(&templates_dir)?;

        Ok(Self {
            config,
            templates_dir,
        })
    }
}

fn write_config(path: &Path, config: &Config) -> Result<()> {
    let body = toml::to_string_pretty(config)?;
    let mut file = File::create(path)?;
    file.write_all(body.as_bytes())?;
    Ok(())
}

fn ensure_default_template(templates_dir: &Path) -> Result<()> {
    let default_template = templates_dir.join("default.md");
    if !default_template.exists() {
        let mut file = File::create(&default_template)?;
        file.write_all(
            br#"# Feature goal

- Summarize the work at a high level.
- List the main constraints.

# Notes for the agent

- Stay concise.
- Suggest useful tests.

"#,
        )?;
    }
    Ok(())
}
