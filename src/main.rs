mod config;
mod git;
mod templates;
mod ui;

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use console::style;
use dialoguer::{Confirm, Input, theme::ColorfulTheme};

use config::ConfigState;
use git::{GitRepo, Worktree};

fn main() {
    if let Err(error) = try_main() {
        eprintln!("{} {}", style("Error:").red(), error);
        std::process::exit(1);
    }
}

fn try_main() -> Result<()> {
    let cfg = ConfigState::load()?;
    let repo = GitRepo::discover()?;
    let mut app = App::new(repo, cfg);
    app.run()
}

struct App {
    repo: GitRepo,
    cfg: ConfigState,
    theme: ColorfulTheme,
}

impl App {
    fn new(repo: GitRepo, cfg: ConfigState) -> Self {
        Self {
            repo,
            cfg,
            theme: ColorfulTheme::default(),
        }
    }

    fn run(&mut self) -> Result<()> {
        loop {
            println!(
                "{} {} ({})",
                style("AgentManager").green().bold(),
                style(&self.cfg.config.agent_display_name).cyan(),
                self.repo.root.display()
            );
            println!("{}", style("Select an action (Ctrl+C to quit)").dim());

            let actions = vec![
                "New feature -> create worktree and launch the agent",
                "Start an existing workflow",
                "Merge an existing worktree",
                "Delete a worktree",
                "Open lazygit on a worktree",
                "Quit",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();

            let selection = ui::skim_select(&actions, "Action> ")?;
            let Some(choice) = selection else {
                println!("{}", style("No action selected, exiting program.").yellow());
                return Ok(());
            };

            match choice {
                0 => self.new_feature_flow()?,
                1 => self.start_existing_workflow()?,
                2 => self.merge_existing_worktree()?,
                3 => self.delete_worktree()?,
                4 => self.view_worktree()?,
                _ => {
                    println!("{}", style("See you!").green());
                    return Ok(());
                }
            }
        }
    }

    fn new_feature_flow(&mut self) -> Result<()> {
        let branch_name_input: String = Input::with_theme(&self.theme)
            .with_prompt("Branch name")
            .default("agent/".to_string())
            .interact_text()?;
        let branch_name = branch_name_input.trim().to_string();
        if branch_name.is_empty() {
            println!("{}", style("Empty branch name, aborting.").yellow());
            return Ok(());
        }

        let feature_description: String = Input::with_theme(&self.theme)
            .with_prompt("Feature name")
            .interact_text()?;
        if feature_description.trim().is_empty() {
            println!("{}", style("Empty feature name, aborting.").yellow());
            return Ok(());
        }

        let base_branch: String = Input::with_theme(&self.theme)
            .with_prompt("Base branch")
            .default(self.cfg.config.merge_target.clone())
            .interact_text()?;

        let slug = sanitize_name(&branch_name);
        let worktree_base = self.repo.worktree_base(&self.cfg)?;
        std::fs::create_dir_all(&worktree_base).with_context(|| {
            format!(
                "Unable to create worktree directory {}",
                worktree_base.display()
            )
        })?;
        let worktree_dir = worktree_base.join(&slug);

        if worktree_dir.exists() {
            return Err(anyhow!(
                "Target worktree {} already exists",
                worktree_dir.display()
            ));
        }

        self.repo
            .create_worktree(&branch_name, &worktree_dir, &base_branch)?;

        println!(
            "{} Worktree created in {} on branch {}",
            style("[ok]").green(),
            worktree_dir.display(),
            branch_name
        );

        let template_path = match templates::choose_template(&self.cfg, &self.repo.root)? {
            Some(path) => path,
            None => {
                println!(
                    "{} No template selected, aborting feature creation.",
                    style("!").yellow()
                );
                let _ = self.repo.remove_worktree(&worktree_dir, true);
                let _ = self.repo.delete_branch(&branch_name, true);
                return Ok(());
            }
        };

        let mut automatic_variables = HashMap::new();
        automatic_variables.insert(
            "feature".to_string(),
            feature_description.trim().to_string(),
        );
        automatic_variables.insert("branch".to_string(), branch_name.clone());

        let local_template = templates::copy_template_to_worktree(
            &template_path,
            &worktree_dir,
            &self.theme,
            &automatic_variables,
        )?;
        println!(
            "{} Template copied to {}",
            style("[info]").blue(),
            local_template.display()
        );

        if Confirm::with_theme(&self.theme)
            .with_prompt("Edit the template before launching the agent?")
            .default(true)
            .interact()?
        {
            templates::edit_template(&self.cfg.config.template_editor, &local_template)?;
        }

        self.run_agent(&worktree_dir, &branch_name, &local_template)?;

        if Confirm::with_theme(&self.theme)
            .with_prompt("Open lazygit to review or commit?")
            .default(true)
            .interact()?
        {
            self.open_lazygit(&worktree_dir)?;
        }

        if Confirm::with_theme(&self.theme)
            .with_prompt(format!(
                "Merge branch {} into {}?",
                branch_name, self.cfg.config.merge_target
            ))
            .default(false)
            .interact()?
        {
            if let Err(err) = self
                .repo
                .merge_branch(&branch_name, &self.cfg.config.merge_target)
            {
                println!("{} Merge aborted: {}", style("!").red(), err);
            } else {
                println!(
                    "{} Merge completed into {}",
                    style("[ok]").green(),
                    self.cfg.config.merge_target
                );
            }
        }

        if Confirm::with_theme(&self.theme)
            .with_prompt("Remove the worktree?")
            .default(false)
            .interact()?
        {
            if let Err(err) = self.repo.remove_worktree(&worktree_dir, false) {
                println!(
                    "{} Unable to remove without force: {}",
                    style("!").yellow(),
                    err
                );
                if Confirm::with_theme(&self.theme)
                    .with_prompt("Force removal? (will discard uncommitted changes)")
                    .default(false)
                    .interact()?
                {
                    self.repo.remove_worktree(&worktree_dir, true)?;
                }
            }

            if Confirm::with_theme(&self.theme)
                .with_prompt("Delete the local branch as well?")
                .default(false)
                .interact()?
            {
                if let Err(err) = self.repo.delete_branch(&branch_name, false) {
                    println!(
                        "{} Unable to delete branch softly: {}",
                        style("!").yellow(),
                        err
                    );
                    if Confirm::with_theme(&self.theme)
                        .with_prompt("Force branch deletion?")
                        .default(false)
                        .interact()?
                    {
                        self.repo.delete_branch(&branch_name, true)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn run_agent(&self, worktree_dir: &Path, branch: &str, template: &Path) -> Result<()> {
        println!(
            "{} Launching agent {} ...",
            style("[info]").blue(),
            self.cfg.config.agent_display_name
        );

        let template_str = template.to_string_lossy().to_string();
        let worktree_str = worktree_dir.to_string_lossy().to_string();
        let template_content = std::fs::read_to_string(template)
            .with_context(|| format!("Unable to read template {}", template.display()))?;

        let mut cmd = Command::new(&self.cfg.config.agent_command);
        let mut uses_template_placeholder = false;

        for arg in &self.cfg.config.agent_args {
            if arg.contains("{template}") || arg.contains("{template_content}") {
                uses_template_placeholder = true;
            }

            cmd.arg(
                arg.replace("{template}", &template_str)
                    .replace("{worktree}", &worktree_str)
                    .replace("{branch}", branch)
                    .replace("{template_content}", &template_content),
            );
        }

        if !uses_template_placeholder {
            cmd.arg(&template_content);
        }

        let status = cmd
            .current_dir(worktree_dir)
            .env("AGENT_TEMPLATE_PATH", &template_str)
            .env("AGENT_WORKTREE_PATH", &worktree_str)
            .env("AGENT_BRANCH_NAME", branch)
            .env("AGENT_TEMPLATE_CONTENT", &template_content)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("Failed to launch agent {}", self.cfg.config.agent_command))?;

        if !status.success() {
            return Err(anyhow!("Agent exited with a non zero status ({})", status));
        }

        Ok(())
    }

    fn start_existing_workflow(&mut self) -> Result<()> {
        let worktrees = self.filtered_worktrees()?;
        if worktrees.is_empty() {
            println!(
                "{}",
                style("No agent worktree available to start.").yellow()
            );
            return Ok(());
        }

        let (selection, selected) = self.pick_worktree(&worktrees, "Start> ")?;
        let Some(idx) = selection else {
            println!("{}", style("No selection, aborting.").yellow());
            return Ok(());
        };
        let worktree = &selected[idx];

        let cached_template = worktree.path.join(templates::TEMPLATE_FILENAME);
        if !cached_template.exists() {
            println!(
                "{} Cached template not found at {}, aborting.",
                style("!").yellow(),
                cached_template.display()
            );
            return Ok(());
        }

        templates::ensure_template_ignored(&worktree.path)?;

        if Confirm::with_theme(&self.theme)
            .with_prompt("Edit the cached template before launching the agent?")
            .default(false)
            .interact()?
        {
            templates::edit_template(&self.cfg.config.template_editor, &cached_template)?;
        }

        let branch = worktree.branch.as_deref().unwrap_or("<detached>");
        self.run_agent(&worktree.path, branch, &cached_template)
    }

    fn merge_existing_worktree(&mut self) -> Result<()> {
        let worktrees = self.filtered_worktrees()?;
        if worktrees.is_empty() {
            println!(
                "{}",
                style("No agent worktree available to merge.").yellow()
            );
            return Ok(());
        }

        let (selection, selected) = self.pick_worktree(&worktrees, "Merge> ")?;
        let Some(idx) = selection else {
            println!("{}", style("No selection, aborting.").yellow());
            return Ok(());
        };
        let worktree = &selected[idx];
        let branch = worktree
            .branch
            .as_deref()
            .ok_or_else(|| anyhow!("Worktree has no associated branch"))?;

        if Confirm::with_theme(&self.theme)
            .with_prompt(format!(
                "Merge {} into {}?",
                branch, self.cfg.config.merge_target
            ))
            .default(true)
            .interact()?
        {
            self.repo
                .merge_branch(branch, &self.cfg.config.merge_target)?;
            println!(
                "{} Merge of {} into {} completed.",
                style("[ok]").green(),
                branch,
                self.cfg.config.merge_target
            );
        }

        Ok(())
    }

    fn delete_worktree(&mut self) -> Result<()> {
        let worktrees = self.filtered_worktrees()?;
        if worktrees.is_empty() {
            println!(
                "{}",
                style("No agent worktree available to delete.").yellow()
            );
            return Ok(());
        }

        let (selection, selected) = self.pick_worktree(&worktrees, "Delete> ")?;
        let Some(idx) = selection else {
            println!("{}", style("No selection, aborting.").yellow());
            return Ok(());
        };
        let worktree = &selected[idx];
        let branch = worktree.branch.clone();

        if Confirm::with_theme(&self.theme)
            .with_prompt(format!("Delete worktree {}?", worktree.path.display()))
            .default(false)
            .interact()?
        {
            if let Err(err) = self.repo.remove_worktree(&worktree.path, false) {
                println!(
                    "{} Unable to delete without force: {}",
                    style("!").yellow(),
                    err
                );
                if Confirm::with_theme(&self.theme)
                    .with_prompt("Force deletion?")
                    .default(false)
                    .interact()?
                {
                    self.repo.remove_worktree(&worktree.path, true)?;
                }
            }
        }

        if let Some(branch) = branch {
            if Confirm::with_theme(&self.theme)
                .with_prompt(format!("Delete branch {}?", branch))
                .default(false)
                .interact()?
            {
                if let Err(err) = self.repo.delete_branch(&branch, false) {
                    println!(
                        "{} Unable to delete branch without force: {}",
                        style("!").yellow(),
                        err
                    );
                    if Confirm::with_theme(&self.theme)
                        .with_prompt("Force branch deletion?")
                        .default(false)
                        .interact()?
                    {
                        self.repo.delete_branch(&branch, true)?;
                    }
                }
            }
        }

        Ok(())
    }

    fn view_worktree(&mut self) -> Result<()> {
        let worktrees = self.repo.list_worktrees()?;
        if worktrees.is_empty() {
            println!("{}", style("No worktree detected.").yellow());
            return Ok(());
        }

        let (selection, selected) = self.pick_worktree(&worktrees, "lazygit> ")?;
        let Some(idx) = selection else {
            println!("{}", style("No selection, aborting.").yellow());
            return Ok(());
        };
        let worktree = &selected[idx];
        self.open_lazygit(&worktree.path)
    }

    fn open_lazygit(&self, worktree: &Path) -> Result<()> {
        println!(
            "{} Launching lazygit in {}",
            style("[info]").blue(),
            worktree.display()
        );
        let status = Command::new("lazygit")
            .current_dir(worktree)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Failed to launch lazygit")?;
        if !status.success() {
            return Err(anyhow!(
                "lazygit exited with a non zero status ({})",
                status
            ));
        }
        Ok(())
    }

    fn filtered_worktrees(&self) -> Result<Vec<Worktree>> {
        let worktrees = self.repo.list_worktrees()?;
        Ok(worktrees
            .into_iter()
            .filter(|wt| wt.path != self.repo.root)
            .collect())
    }

    fn pick_worktree(
        &self,
        worktrees: &[Worktree],
        prompt: &str,
    ) -> Result<(Option<usize>, Vec<Worktree>)> {
        let items = worktrees
            .iter()
            .map(|wt| worktree_label(wt))
            .collect::<Vec<_>>();
        let selection = ui::skim_select(&items, prompt)?;
        Ok((selection, worktrees.to_vec()))
    }
}

fn sanitize_name(input: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in input.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            last_dash = false;
        } else if "-_".contains(lower) {
            slug.push(lower);
            last_dash = lower == '-';
        } else {
            if !last_dash {
                slug.push('-');
                last_dash = true;
            }
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "feature".to_string()
    } else {
        trimmed.to_string()
    }
}

fn worktree_label(worktree: &Worktree) -> String {
    let branch = worktree.branch.as_deref().unwrap_or("<detached>");
    let mut label = format!("{} - {}", branch, worktree.path.display());
    if worktree.locked {
        label.push_str(" [locked]");
    }
    label
}
