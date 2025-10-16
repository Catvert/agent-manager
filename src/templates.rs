use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use console::style;

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

pub fn copy_template_to_worktree(template: &Path, worktree: &Path) -> Result<PathBuf> {
    let destination = worktree.join(TEMPLATE_FILENAME);
    fs::copy(template, &destination).with_context(|| {
        format!(
            "Failed to copy template {} to {}",
            template.display(),
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
