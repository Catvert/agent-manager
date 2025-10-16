use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use console::style;
use dialoguer::{Input, theme::ColorfulTheme};
use regex::Regex;

use crate::config::ConfigState;
use crate::ui;

pub const TEMPLATE_FILENAME: &str = ".agent-template";
pub const PROJECT_TEMPLATES_DIR: &str = ".agent-templates";

pub fn available_templates(cfg: &ConfigState, project_root: &Path) -> Result<Vec<PathBuf>> {
    if let Some(project_templates) = project_templates(project_root)? {
        return Ok(project_templates);
    }
    collect_templates(&cfg.templates_dir)
}

pub fn choose_template(cfg: &ConfigState, project_root: &Path) -> Result<Option<PathBuf>> {
    let project_templates_dir = project_root.join(PROJECT_TEMPLATES_DIR);
    let templates = available_templates(cfg, project_root)?;
    if templates.is_empty() {
        if project_templates_dir.is_dir() {
            println!(
                "{} No template found in {} or {}",
                style("!").yellow(),
                project_templates_dir.display(),
                cfg.templates_dir.display()
            );
        } else {
            println!(
                "{} No template found in {}",
                style("!").yellow(),
                cfg.templates_dir.display()
            );
        }
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
    auto_variables: &HashMap<String, String>,
) -> Result<PathBuf> {
    let destination = worktree.join(TEMPLATE_FILENAME);
    let raw_template = fs::read_to_string(template)
        .with_context(|| format!("Unable to read template {}", template.display()))?;
    let rendered_template = render_template(&raw_template, theme, auto_variables)?;
    fs::write(&destination, rendered_template).with_context(|| {
        format!(
            "Failed to write rendered template to {}",
            destination.display()
        )
    })?;
    ensure_template_ignored(worktree)?;
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

fn render_template(
    content: &str,
    theme: &ColorfulTheme,
    auto_variables: &HashMap<String, String>,
) -> Result<String> {
    let pattern = Regex::new(r"\$\{([^}]+)\}")?;
    let mut prompts = Vec::new();

    for caps in pattern.captures_iter(content) {
        let name = caps.get(1).map(|m| m.as_str().trim()).unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if auto_variables.contains_key(name) {
            continue;
        }
        if !prompts.iter().any(|existing| existing == name) {
            prompts.push(name.to_string());
        }
    }

    if prompts.is_empty() && auto_variables.is_empty() {
        return Ok(content.to_string());
    }

    let mut values: HashMap<String, String> = auto_variables.clone();

    if !prompts.is_empty() {
        println!(
            "{} {}",
            style("[info]").blue(),
            style("Template variables detected, please provide their values.").dim()
        );

        for prompt in prompts {
            let value: String = Input::with_theme(theme)
                .with_prompt(format!("Value for {}", prompt))
                .allow_empty(true)
                .interact_text()?;
            values.insert(prompt, value);
        }
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

fn project_templates(project_root: &Path) -> Result<Option<Vec<PathBuf>>> {
    let project_templates_dir = project_root.join(PROJECT_TEMPLATES_DIR);
    if !project_templates_dir.is_dir() {
        return Ok(None);
    }

    let templates = collect_templates(&project_templates_dir)?;
    if templates.is_empty() {
        Ok(None)
    } else {
        Ok(Some(templates))
    }
}

fn collect_templates(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("Unable to read templates directory {:?}", dir))?
    {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            entries.push(entry.path());
        }
    }
    entries.sort();
    Ok(entries)
}

pub fn ensure_template_ignored(worktree: &Path) -> Result<()> {
    let git_dir = git_dir_for_worktree(worktree)?;
    let info_dir = git_dir.join("info");
    fs::create_dir_all(&info_dir)
        .with_context(|| format!("Unable to create git info directory {}", info_dir.display()))?;

    let exclude_path = info_dir.join("exclude");
    let existing = match fs::read_to_string(&exclude_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err.into()),
    };

    let alt_pattern = format!("./{}", TEMPLATE_FILENAME);
    let already_present = existing
        .lines()
        .map(|line| line.trim())
        .any(|line| line == TEMPLATE_FILENAME || line == alt_pattern);
    if already_present {
        return Ok(());
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude_path)
        .with_context(|| format!("Unable to open git exclude file {}", exclude_path.display()))?;

    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n").with_context(|| {
            format!(
                "Unable to update git exclude file {}",
                exclude_path.display()
            )
        })?;
    }

    file.write_all(TEMPLATE_FILENAME.as_bytes())
        .with_context(|| {
            format!(
                "Unable to update git exclude file {}",
                exclude_path.display()
            )
        })?;
    file.write_all(b"\n").with_context(|| {
        format!(
            "Unable to update git exclude file {}",
            exclude_path.display()
        )
    })?;

    Ok(())
}

fn git_dir_for_worktree(worktree: &Path) -> Result<PathBuf> {
    let git_entry = worktree.join(".git");
    if git_entry.is_dir() {
        return Ok(git_entry);
    }

    let spec = fs::read_to_string(&git_entry).with_context(|| {
        format!(
            "Unable to read git directory pointer {}",
            git_entry.display()
        )
    })?;
    let trimmed = spec.trim();
    let Some(path_spec) = trimmed.strip_prefix("gitdir:") else {
        return Err(anyhow!(
            "Invalid gitdir pointer in {}: {}",
            git_entry.display(),
            trimmed
        ));
    };
    let path_str = path_spec.trim();
    let mut git_dir = PathBuf::from(path_str);
    if git_dir.is_relative() {
        git_dir = worktree.join(git_dir);
    }

    Ok(git_dir)
}
