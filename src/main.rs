mod config;
mod git;
mod templates;
mod ui;

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};
use console::style;
use dialoguer::{Confirm, Input, theme::ColorfulTheme};

use config::ConfigState;
use git::{GitRepo, Worktree};

fn main() {
    if let Err(error) = try_main() {
        eprintln!("{} {}", style("Erreur:").red(), error);
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
            println!(
                "{}",
                style("Choisir une action (Ctrl+C pour quitter)").dim()
            );

            let actions = vec![
                "Nouvelle feature -> creer un worktree et lancer l'agent",
                "Fusionner un worktree existant",
                "Supprimer un worktree",
                "Ouvrir lazygit sur un worktree",
                "Quitter",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();

            let selection = ui::skim_select(&actions, "Action> ")?;
            let Some(choice) = selection else {
                println!(
                    "{}",
                    style("Aucune action selectionnee, fin du programme.").yellow()
                );
                return Ok(());
            };

            match choice {
                0 => self.new_feature_flow()?,
                1 => self.merge_existing_worktree()?,
                2 => self.delete_worktree()?,
                3 => self.view_worktree()?,
                _ => {
                    println!("{}", style("A bientot!").green());
                    return Ok(());
                }
            }
        }
    }

    fn new_feature_flow(&mut self) -> Result<()> {
        let name: String = Input::with_theme(&self.theme)
            .with_prompt("Nom de la feature")
            .interact_text()?;
        if name.trim().is_empty() {
            println!("{}", style("Nom vide, annulation.").yellow());
            return Ok(());
        }

        let base_branch: String = Input::with_theme(&self.theme)
            .with_prompt("Branche de base")
            .default(self.cfg.config.merge_target.clone())
            .interact_text()?;

        let slug = sanitize_name(&name);
        let branch_name = format!("agent/{}", slug);
        let worktree_base = self.repo.worktree_base(&self.cfg)?;
        std::fs::create_dir_all(&worktree_base).with_context(|| {
            format!(
                "Impossible de creer le dossier des worktrees {}",
                worktree_base.display()
            )
        })?;
        let worktree_dir = worktree_base.join(&slug);

        if worktree_dir.exists() {
            return Err(anyhow!(
                "Le worktree cible {} existe deja",
                worktree_dir.display()
            ));
        }

        self.repo
            .create_worktree(&branch_name, &worktree_dir, &base_branch)?;

        println!(
            "{} Worktree cree dans {} sur la branche {}",
            style("[ok]").green(),
            worktree_dir.display(),
            branch_name
        );

        let template_path = match templates::choose_template(&self.cfg)? {
            Some(path) => path,
            None => {
                println!(
                    "{} Aucun template selectionne, la creation est annulee.",
                    style("!").yellow()
                );
                let _ = self.repo.remove_worktree(&worktree_dir, true);
                let _ = self.repo.delete_branch(&branch_name, true);
                return Ok(());
            }
        };

        let local_template = templates::copy_template_to_worktree(&template_path, &worktree_dir)?;
        println!(
            "{} Template copie vers {}",
            style("[info]").blue(),
            local_template.display()
        );

        if Confirm::with_theme(&self.theme)
            .with_prompt("Modifier le template avant de lancer l'agent ?")
            .default(true)
            .interact()?
        {
            templates::edit_template(&self.cfg.config.template_editor, &local_template)?;
        }

        self.run_agent(&worktree_dir, &branch_name, &local_template)?;

        if Confirm::with_theme(&self.theme)
            .with_prompt("Ouvrir lazygit pour visualiser/committer ?")
            .default(true)
            .interact()?
        {
            self.open_lazygit(&worktree_dir)?;
        }

        if Confirm::with_theme(&self.theme)
            .with_prompt(format!(
                "Fusionner la branche {} vers {} ?",
                branch_name, self.cfg.config.merge_target
            ))
            .default(false)
            .interact()?
        {
            if let Err(err) = self
                .repo
                .merge_branch(&branch_name, &self.cfg.config.merge_target)
            {
                println!("{} Fusion interrompue: {}", style("!").red(), err);
            } else {
                println!(
                    "{} Fusion reussie vers {}",
                    style("[ok]").green(),
                    self.cfg.config.merge_target
                );
            }
        }

        if Confirm::with_theme(&self.theme)
            .with_prompt("Supprimer le worktree ?")
            .default(false)
            .interact()?
        {
            if let Err(err) = self.repo.remove_worktree(&worktree_dir, false) {
                println!(
                    "{} Suppression impossible sans force: {}",
                    style("!").yellow(),
                    err
                );
                if Confirm::with_theme(&self.theme)
                    .with_prompt("Forcer la suppression ? (perte de changements non commits)")
                    .default(false)
                    .interact()?
                {
                    self.repo.remove_worktree(&worktree_dir, true)?;
                }
            }

            if Confirm::with_theme(&self.theme)
                .with_prompt("Supprimer aussi la branche locale ?")
                .default(false)
                .interact()?
            {
                if let Err(err) = self.repo.delete_branch(&branch_name, false) {
                    println!(
                        "{} Suppression douce impossible: {}",
                        style("!").yellow(),
                        err
                    );
                    if Confirm::with_theme(&self.theme)
                        .with_prompt("Forcer la suppression de la branche ?")
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
            "{} Lancement de l'agent {} ...",
            style("[info]").blue(),
            self.cfg.config.agent_display_name
        );

        let template_str = template.to_string_lossy().to_string();
        let worktree_str = worktree_dir.to_string_lossy().to_string();

        let mut cmd = Command::new(&self.cfg.config.agent_command);
        for arg in &self.cfg.config.agent_args {
            cmd.arg(
                arg.replace("{template}", &template_str)
                    .replace("{worktree}", &worktree_str)
                    .replace("{branch}", branch),
            );
        }

        let status = cmd
            .current_dir(worktree_dir)
            .env("AGENT_TEMPLATE_PATH", &template_str)
            .env("AGENT_WORKTREE_PATH", &worktree_str)
            .env("AGENT_BRANCH_NAME", branch)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| {
                format!(
                    "Echec du lancement de l'agent {}",
                    self.cfg.config.agent_command
                )
            })?;

        if !status.success() {
            return Err(anyhow!(
                "L'agent s'est termine avec un code non nul ({})",
                status
            ));
        }

        Ok(())
    }

    fn merge_existing_worktree(&mut self) -> Result<()> {
        let worktrees = self.filtered_worktrees()?;
        if worktrees.is_empty() {
            println!("{}", style("Aucun worktree agent a fusionner.").yellow());
            return Ok(());
        }

        let (selection, selected) = self.pick_worktree(&worktrees, "Fusion> ")?;
        let Some(idx) = selection else {
            println!("{}", style("Aucune selection, annulation.").yellow());
            return Ok(());
        };
        let worktree = &selected[idx];
        let branch = worktree
            .branch
            .as_deref()
            .ok_or_else(|| anyhow!("Worktree sans branche associee"))?;

        if Confirm::with_theme(&self.theme)
            .with_prompt(format!(
                "Fusionner {} vers {} ?",
                branch, self.cfg.config.merge_target
            ))
            .default(true)
            .interact()?
        {
            self.repo
                .merge_branch(branch, &self.cfg.config.merge_target)?;
            println!(
                "{} Fusion de {} dans {} terminee.",
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
            println!("{}", style("Aucun worktree agent a supprimer.").yellow());
            return Ok(());
        }

        let (selection, selected) = self.pick_worktree(&worktrees, "Supprimer> ")?;
        let Some(idx) = selection else {
            println!("{}", style("Aucune selection, annulation.").yellow());
            return Ok(());
        };
        let worktree = &selected[idx];
        let branch = worktree.branch.clone();

        if Confirm::with_theme(&self.theme)
            .with_prompt(format!(
                "Supprimer le worktree {} ?",
                worktree.path.display()
            ))
            .default(false)
            .interact()?
        {
            if let Err(err) = self.repo.remove_worktree(&worktree.path, false) {
                println!(
                    "{} Suppression douce impossible: {}",
                    style("!").yellow(),
                    err
                );
                if Confirm::with_theme(&self.theme)
                    .with_prompt("Forcer la suppression ?")
                    .default(false)
                    .interact()?
                {
                    self.repo.remove_worktree(&worktree.path, true)?;
                }
            }
        }

        if let Some(branch) = branch {
            if Confirm::with_theme(&self.theme)
                .with_prompt(format!("Supprimer la branche {} ?", branch))
                .default(false)
                .interact()?
            {
                if let Err(err) = self.repo.delete_branch(&branch, false) {
                    println!(
                        "{} Suppression douce impossible: {}",
                        style("!").yellow(),
                        err
                    );
                    if Confirm::with_theme(&self.theme)
                        .with_prompt("Forcer la suppression de la branche ?")
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
            println!("{}", style("Aucun worktree detecte.").yellow());
            return Ok(());
        }

        let (selection, selected) = self.pick_worktree(&worktrees, "lazygit> ")?;
        let Some(idx) = selection else {
            println!("{}", style("Aucune selection, annulation.").yellow());
            return Ok(());
        };
        let worktree = &selected[idx];
        self.open_lazygit(&worktree.path)
    }

    fn open_lazygit(&self, worktree: &Path) -> Result<()> {
        println!(
            "{} Lancement de lazygit dans {}",
            style("[info]").blue(),
            worktree.display()
        );
        let status = Command::new("lazygit")
            .current_dir(worktree)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("Impossible de lancer lazygit")?;
        if !status.success() {
            return Err(anyhow!(
                "lazygit s'est termine avec un code non nul ({})",
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
