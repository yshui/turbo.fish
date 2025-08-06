use std::{
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
};

use blocking::unblock;
use git2::{DescribeOptions, Repository};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, whatever};

use crate::sources::{GlobalConfig, UpdateSender, log_result};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    /// Repo has unstaged changes
    Dirty,
    /// Repo has staged changed
    Staged,
    /// Repo is even with a commit
    Even,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub struct State {
    dirty: Option<Dirty>,
    description: Option<String>,
    ahead: Option<Option<u64>>,
    behind: Option<Option<u64>>,
}

fn default_dirty_bg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0xce,
        g: 0,
        b: 0xf,
    })
}

fn default_dirty_fg() -> super::Color {
    super::Color::Ansi(super::Ansi {
        code: 15,
        named: true,
    })
}

fn default_clean_bg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0xad,
        g: 0xdc,
        b: 0x10,
    })
}

fn default_clean_fg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0xc,
        g: 0x48,
        b: 0x1,
    })
}

fn default_staged_bg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0xf6,
        g: 0xb1,
        b: 0x17,
    })
}

fn default_staged_fg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0x3a,
        g: 0x2a,
        b: 0x3,
    })
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Config {
    #[serde(default = "default_dirty_bg")]
    dirty_bg: super::Color,
    #[serde(default = "default_dirty_fg")]
    dirty_fg: super::Color,
    #[serde(default = "default_clean_bg")]
    clean_bg: super::Color,
    #[serde(default = "default_clean_fg")]
    clean_fg: super::Color,
    #[serde(default = "default_staged_bg")]
    staged_bg: super::Color,
    #[serde(default = "default_staged_fg")]
    staged_fg: super::Color,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            dirty_bg: default_dirty_bg(),
            clean_bg: default_clean_bg(),
            dirty_fg: default_dirty_fg(),
            clean_fg: default_clean_fg(),
            staged_fg: default_staged_fg(),
            staged_bg: default_staged_bg(),
        }
    }
}

pub struct Source {
    repository: Option<Arc<Mutex<git2::Repository>>>,
    cfg: Config,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field(
                "repository",
                &(self
                    .repository
                    .as_ref()
                    .map(|r| r as *const _)
                    .unwrap_or_default()),
            )
            .finish()
    }
}

async fn process_once(
    repo: &Arc<Mutex<Repository>>,
    tx: &mut UpdateSender<Source>,
) -> Result<(), super::Error> {
    let dirty: Result<_, super::Error> = unblock({
        let repo = repo.clone();
        move || try {
            let repo = repo.lock().unwrap();
            let statuses = repo.statuses(None).whatever_context("repo statuses")?;
            if statuses
                .iter()
                .any(|s| s.status() == git2::Status::WT_MODIFIED)
            {
                Dirty::Dirty
            } else if statuses
                .iter()
                .any(|s| s.status() == git2::Status::INDEX_MODIFIED)
            {
                Dirty::Staged
            } else {
                Dirty::Even
            }
        }
    })
    .await;

    let dirty = whatever!(dirty, "dirty");
    tx.send(Some(State {
        dirty: Some(dirty),
        ..Default::default()
    }))
    .await;

    let description: Result<_, super::Error> = unblock({
        let repo = repo.clone();
        move || try {
            let repo = repo.lock().unwrap();
            let head = repo.head().whatever_context("repo head")?;
            let head_commit = head.peel_to_commit().whatever_context("peel to commit")?;
            (!repo.head_detached().whatever_context("head detached")?)
                .then(|| head.shorthand())
                .flatten()
                .map(ToOwned::to_owned)
                .or_else(|| {
                    head_commit
                        .as_object()
                        .describe(DescribeOptions::new().describe_tags())
                        .and_then(|d| d.format(None))
                        .ok()
                })
                .unwrap_or_else(|| head_commit.id().to_string())
        }
    })
    .await;
    let description = whatever!(description, "description");
    tx.send(Some(State {
        dirty: Some(dirty),
        description: Some(description),
        ..Default::default()
    }))
    .await;
    let repo = repo.try_lock().unwrap();
    let head = repo.head().whatever_context("repo head")?;
    log::debug!(
        "{:?} {}",
        head.name(),
        repo.head_detached()
            .whatever_context("repo head_detached")?
    );
    if head.is_branch() {
        let b = git2::Branch::wrap(head);
        let up = b.upstream().whatever_context("upstream")?;
        log::debug!("{:?}", up.name());
        let mut walk = repo.revwalk().whatever_context("revwalk")?;

        let to = b
            .get()
            .peel_to_commit()
            .whatever_context("peel to commit")?
            .id();
        let from = up
            .get()
            .peel_to_commit()
            .whatever_context("peel to commit")?
            .id();
        log::debug!("{from:?} {to:?}");
        walk.push(to).whatever_context("revwalk push")?;
        walk.hide(from).whatever_context("revwalk hide")?;
        for r in walk {
            let r = r.whatever_context("revwalk reference")?;
            log::debug!("{r:?}");
        }
        let mut walk = repo.revwalk().whatever_context("revwalk")?;
        walk.hide(to).whatever_context("revwalk hide")?;
        walk.push(from).whatever_context("revwalk push")?;
        for r in walk {
            let r = r.whatever_context("revwalk reference")?;
            log::debug!("{r:?}");
        }
    }
    Ok(())
}
impl super::Source for Source {
    type State = State;
    type Config = Config;
    fn new(&cfg: &Config, _global_cfg: &GlobalConfig, path: &Path) -> Self {
        Self {
            cfg,
            repository: git2::Repository::open(path)
                .ok()
                .map(|r| Arc::new(Mutex::new(r))),
        }
    }
    fn start(
        self,
        mut tx: UpdateSender<Source>,
        notify: &Arc<super::Notify>,
    ) -> impl Future<Output = !> {
        let mut waiter = notify.waiter();
        async move {
            loop {
                if let Some(r) = &self.repository {
                    log_result("git", process_once(r, &mut tx).await);
                }
                tx.send(None).await;
                waiter.wait().await;
            }
        }
    }
    fn render(&self, _path: &Path, state: &State) -> Vec<super::Segment> {
        let Some(desc) = &state.description else {
            return vec![];
        };
        let (fg, bg) = match state.dirty {
            Some(Dirty::Dirty) => (self.cfg.dirty_fg, self.cfg.dirty_bg),
            Some(Dirty::Staged) => (self.cfg.staged_fg, self.cfg.staged_bg),
            Some(Dirty::Even) => (self.cfg.clean_fg, self.cfg.clean_bg),
            None => return vec![],
        };
        vec![super::Segment {
            text: desc.to_string(),
            separator: true,
            style: anstyle::Style::new()
                .fg_color(Some(fg.into()))
                .bg_color(Some(bg.into()))
                .effects(anstyle::Effects::new()),
        }]
    }
}
