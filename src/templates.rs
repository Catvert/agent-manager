use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use console::style;
use dialoguer::{Input, theme::ColorfulTheme};
use regex::Regex;

use crate::config::ConfigState;
use crate::ui;

pub const TEMPLATE_FILENAME: &str = ".agent-template";

pub fn available_templates(cfg: &ConfigState) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(&cfg.templates_dir)
        .with_context(|| format!("Unable to read templates directory {:?}", cfg.templates_dir))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            entries.push(entry.path());
        }
    }

    entries.sort();
    Ok(entries)
}

pub fn choose_template(cfg: &ConfigState) -> Result<Option<PathBuf>> {
    let templates = available_templates(cfg)?;
    if templates.is_empty() {
        println!(
            "{} No template found in {}",
            style("!").yellow(),
            cfg.templates_dir.display()
        );
        return Ok(None);
    }

    if templates.len() == 1 {
        return Ok(Some(templates[0].clone()));
    }

    let items = templates
        .iter()
        .map(|p| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        })
        .collect::<Vec<_>>();

    let selection = ui::skim_select(&items, "Template> ")?;
    Ok(selection.map(|idx| templates[idx].clone()))
}

pub fn copy_template_to_worktree(
    template: &Path,
    worktree: &Path,
    theme: &ColorfulTheme,
) -> Result<PathBuf> {
    let destination = worktree.join(TEMPLATE_FILENAME);
    let raw_template = fs::read_to_string(template)
        .with_context(|| format!("Unable to read template {}", template.display()))?;
    let rendered_template = render_template(&raw_template, theme)?;
    fs::write(&destination, rendered_template).with_context(|| {
        format!(
            "Failed to write rendered template to {}",
            destination.display()
        )
    })?;
    Ok(destination)
}

pub fn edit_template(editor: &str, template_path: &Path) -> Result<()> {
    let status = Command::new(editor)
        .arg(template_path)
        .status()
        .with_context(|| format!("Failed to launch editor {}", editor))?;
    if !status.success() {
        return Err(anyhow!("Editor {} exited with a non zero status", editor));
    }
    Ok(())
}

fn render_template(content: &str, theme: &ColorfulTheme) -> Result<String> {
    let pattern = Regex::new(r"\$\{([^}]+)\}")?;
    let mut prompts = Vec::new();

    for caps in pattern.captures_iter(content) {
        let name = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if !prompts.iter().any(|existing| existing == name) {
            prompts.push(name.to_string());
        }
    }

    if prompts.is_empty() {
        return Ok(content.to_string());
    }

    println!(
        "{} {}",
        style("[info]").blue(),
        style("Template variables detected, please provide their values.").dim()
    );

    let mut values: HashMap<String, String> = HashMap::new();
    for prompt in prompts {
        let value: String = Input::with_theme(theme)
            .with_prompt(format!("Value for {}", prompt))
            .allow_empty(true)
            .interact_text()?;
        values.insert(prompt, value);
    }

    let rendered = pattern.replace_all(content, |caps: &regex::Captures| {
        let key = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
        values.get(key).cloned().unwrap_or_else(|| {
            caps.get(0)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string()
        })
    });

    Ok(rendered.into_owned())
}
