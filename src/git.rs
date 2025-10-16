use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::config::ConfigState;

#[derive(Debug, Clone)]
pub struct GitRepo {
    pub root: PathBuf,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub locked: bool,
}

impl GitRepo {
    pub fn discover() -> Result<Self> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context(
                "Unable to resolve the current git repository (git rev-parse --show-toplevel)",
            )?;

        if !output.status.success() {
            return Err(anyhow!(
                "git rev-parse --show-toplevel failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let mut path = String::from_utf8(output.stdout)?;
        path.truncate(path.trim_end().len());
        let root = PathBuf::from(path);
        let name = root
            .file_name()
            .ok_or_else(|| anyhow!("Repository name could not be determined"))?
            .to_string_lossy()
            .to_string();

        Ok(Self { root, name })
    }

    pub fn worktree_base(&self, cfg: &ConfigState) -> Result<PathBuf> {
        if let Some(pattern) = &cfg.config.worktree_base_override {
            let rendered = pattern
                .replace("{repo_name}", &self.name)
                .replace("{repo_root}", &self.root.to_string_lossy());
            Ok(PathBuf::from(rendered))
        } else {
            let parent = self
                .root
                .parent()
                .ok_or_else(|| anyhow!("Unable to resolve the repository parent directory"))?;
            Ok(parent.join(format!("{}-worktree-agents", self.name)))
        }
    }

    pub fn list_worktrees(&self) -> Result<Vec<Worktree>> {
        let output = run_git(&self.root, ["worktree", "list", "--porcelain"])?;
        if !output.status.success() {
            return Err(anyhow!(
                "git worktree list --porcelain failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let text = String::from_utf8(output.stdout)?;
        let mut worktrees = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_branch: Option<String> = None;
        let mut locked = false;

        for line in text.lines() {
            if line.is_empty() {
                if let Some(path) = current_path.take() {
                    worktrees.push(Worktree {
                        path,
                        branch: current_branch.take(),
                        locked,
                    });
                    locked = false;
                }
                continue;
            }

            if let Some(rest) = line.strip_prefix("worktree ") {
                current_path = Some(PathBuf::from(rest));
            } else if let Some(rest) = line.strip_prefix("branch ") {
                current_branch = Some(rest.trim().replacen("refs/heads/", "", 1));
            } else if line.starts_with("locked") {
                locked = true;
            }
        }

        if let Some(path) = current_path {
            worktrees.push(Worktree {
                path,
                branch: current_branch,
                locked,
            });
        }

        Ok(worktrees)
    }

    pub fn create_worktree(
        &self,
        branch_name: &str,
        target_dir: &Path,
        base_branch: &str,
    ) -> Result<()> {
        let status = Command::new("git")
            .current_dir(&self.root)
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(branch_name)
            .arg(target_dir)
            .arg(base_branch)
            .status()
            .with_context(|| {
                format!(
                    "Failed to run git worktree add for {} from {}",
                    target_dir.display(),
                    base_branch
                )
            })?;

        if !status.success() {
            return Err(anyhow!(
                "git worktree add returned a non zero status for branch {}",
                branch_name
            ));
        }

        Ok(())
    }

    pub fn remove_worktree(&self, target_dir: &Path, force: bool) -> Result<()> {
        let mut command = Command::new("git");
        command.current_dir(&self.root).args(["worktree", "remove"]);
        if force {
            command.arg("--force");
        }
        command.arg(target_dir);

        let status = command.status().with_context(|| {
            format!("Failed to run git worktree remove {}", target_dir.display())
        })?;
        if !status.success() {
            return Err(anyhow!(
                "git worktree remove failed for {}",
                target_dir.display()
            ));
        }
        Ok(())
    }

    pub fn delete_branch(&self, branch: &str, force: bool) -> Result<()> {
        let flag = if force { "-D" } else { "-d" };
        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["branch", flag, branch])
            .status()
            .context("Failed to run git branch -d")?;
        if !status.success() {
            return Err(anyhow!("Unable to delete branch {}", branch));
        }
        Ok(())
    }

    pub fn merge_branch(&self, source_branch: &str, target_branch: &str) -> Result<()> {
        let current = self.current_branch()?;
        if current.as_deref() != Some(target_branch) {
            self.checkout_branch(target_branch)?;
        }

        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["merge", "--no-ff", source_branch])
            .status()
            .with_context(|| format!("Failed to merge {} into {}", source_branch, target_branch))?;

        if !status.success() {
            return Err(anyhow!(
                "git merge failed while merging {} into {}",
                source_branch,
                target_branch
            ));
        }

        if current.as_deref() != Some(target_branch) {
            if let Some(branch) = current {
                self.checkout_branch(&branch)?;
            }
        }

        Ok(())
    }

    pub fn current_branch(&self) -> Result<Option<String>> {
        let output = run_git(&self.root, ["rev-parse", "--abbrev-ref", "HEAD"])?;
        if !output.status.success() {
            return Ok(None);
        }
        let name = String::from_utf8(output.stdout)?;
        let trimmed = name.trim();
        if trimmed == "HEAD" {
            Ok(None)
        } else {
            Ok(Some(trimmed.to_string()))
        }
    }

    pub fn checkout_branch(&self, branch: &str) -> Result<()> {
        let status = Command::new("git")
            .current_dir(&self.root)
            .args(["checkout", branch])
            .status()
            .with_context(|| format!("Failed to run git checkout {}", branch))?;
        if !status.success() {
            return Err(anyhow!("Unable to checkout branch {}", branch));
        }
        Ok(())
    }
}

fn run_git<S>(root: &Path, args: impl IntoIterator<Item = S>) -> Result<std::process::Output>
where
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .with_context(|| format!("Failed to execute git in {}", root.display()))?;

    Ok(output)
}
