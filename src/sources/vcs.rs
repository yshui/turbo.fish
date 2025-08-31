use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use crate::color::{Ansi, Color, Rgb};
use blocking::unblock;
use git2::{DescribeOptions, Repository};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, whatever};

use crate::sources::{GlobalConfig, UpdateSender, log_result};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dirty {
    /// Repo has unstaged changes
    Dirty { conflicted: bool },
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

fn default_dirty_bg() -> Color {
    Color::Rgb(Rgb {
        r: 0xce,
        g: 0,
        b: 0xf,
    })
}

fn default_dirty_fg() -> Color {
    Color::Ansi(Ansi {
        code: 15,
        named: true,
    })
}

fn default_clean_bg() -> Color {
    Color::Rgb(Rgb {
        r: 0xad,
        g: 0xdc,
        b: 0x10,
    })
}

fn default_clean_fg() -> Color {
    Color::Rgb(Rgb {
        r: 0xc,
        g: 0x48,
        b: 0x1,
    })
}

fn default_staged_bg() -> Color {
    Color::Rgb(Rgb {
        r: 0xf6,
        g: 0xb1,
        b: 0x17,
    })
}

fn default_staged_fg() -> Color {
    Color::Rgb(Rgb {
        r: 0x3a,
        g: 0x2a,
        b: 0x3,
    })
}

fn default_git_short_hash_min() -> u32 {
    5
}

fn default_root_priority() -> u64 {
    5
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Config {
    #[serde(default = "default_dirty_bg")]
    dirty_bg: Color,
    #[serde(default = "default_dirty_fg")]
    dirty_fg: Color,
    #[serde(default = "default_clean_bg")]
    clean_bg: Color,
    #[serde(default = "default_clean_fg")]
    clean_fg: Color,
    #[serde(default = "default_staged_bg")]
    staged_bg: Color,
    #[serde(default = "default_staged_fg")]
    staged_fg: Color,
    /// Minimal length a git commit hash can be shortened to.
    #[serde(default = "default_git_short_hash_min")]
    git_short_hash_min: u32,

    #[serde(default = "default_root_priority")]
    root_priority: u64,
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
            git_short_hash_min: default_git_short_hash_min(),
            root_priority: default_root_priority(),
        }
    }
}

#[derive(Debug)]
pub struct Source {
    cfg: Config,
}

impl Source {
    async fn process_once(
        &self,
        repo: &Arc<Mutex<Repository>>,
        tx: &mut UpdateSender<Source>,
    ) -> Result<(), super::Error> {
        let dirty: Result<_, super::Error> = unblock({
            let repo = repo.clone();
            move || try {
                let repo = repo.lock().unwrap();
                let statuses = repo.statuses(None).whatever_context("repo statuses")?;
                if log::log_enabled!(log::Level::Debug) {
                    for s in statuses.iter() {
                        if s.status() != git2::Status::IGNORED {
                            log::debug!("{:?} {:?}", s.path(), s.status())
                        }
                    }
                }
                if statuses
                    .iter()
                    .any(|s| s.status().contains(git2::Status::CONFLICTED))
                {
                    Dirty::Dirty { conflicted: true }
                } else if statuses
                    .iter()
                    .any(|s| s.status().contains(git2::Status::WT_MODIFIED))
                {
                    Dirty::Dirty { conflicted: false }
                } else if statuses
                    .iter()
                    .any(|s| s.status().contains(git2::Status::INDEX_MODIFIED))
                {
                    Dirty::Staged
                } else {
                    Dirty::Even
                }
            }
        })
        .await;

        let dirty = whatever!(dirty, "dirty");
        tx.send(State {
            dirty: Some(dirty),
            ..Default::default()
        });

        let description: Result<_, super::Error> = unblock({
            let repo = repo.clone();
            let min = self.cfg.git_short_hash_min;
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
                    .unwrap_or_else(|| {
                        let commit_id = head_commit.id();
                        let Ok(odb) = repo.odb() else {
                            return commit_id.to_string();
                        };

                        for i in min as usize..commit_id.as_ref().len() * 2 {
                            match odb.exists_prefix(commit_id, i) {
                                Ok(_) => {
                                    let mut s = commit_id.to_string();
                                    s.truncate(i);
                                    return s;
                                }
                                Err(e) if e.code() == git2::ErrorCode::Ambiguous => {
                                    log::debug!("len {i} is ambiguous");
                                    continue;
                                }
                                Err(e) => {
                                    log::warn!("odb exists_prefix failed: {e}");
                                    break;
                                }
                            }
                        }
                        commit_id.to_string()
                    })
            }
        })
        .await;
        let description = whatever!(description, "description");
        tx.send(State {
            dirty: Some(dirty),
            description: Some(description.clone()),
            ..Default::default()
        });
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
            let Ok(up) = b.upstream() else {
                log::debug!(
                    "branch \"{}\" has no upstream",
                    b.name().ok().flatten().unwrap_or_default()
                );
                return Ok(());
            };
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
}
#[derive(Deserialize, Serialize, Default)]
pub(crate) struct PathInfo {
    #[serde(skip)]
    repository: Option<Arc<Mutex<git2::Repository>>>,
}
impl super::PathInfo for PathInfo {}

impl std::fmt::Debug for PathInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        struct Ellipsis;
        impl std::fmt::Debug for Ellipsis {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("…")
            }
        }
        f.debug_struct("PathInfo")
            .field("repository", &self.repository.as_ref().map(|_| Ellipsis))
            .finish()
    }
}
impl super::Source for Source {
    type State = State;
    type Config = Config;
    type PathInfo = PathInfo;
    fn new(&cfg: &Config, _global_cfg: &GlobalConfig, _path: &Path) -> Self {
        Self { cfg }
    }
    async fn walk_path(
        &self,
        parent: &Path,
        _current_child: &std::ffi::OsStr,
        child: &std::ffi::OsStr,
        info: &mut PathInfo,
        _index: usize,
    ) -> Result<u64, super::Error> {
        Ok(if info.repository.is_some() {
            0
        } else if child == ".git" {
            info.repository = match git2::Repository::open(parent) {
                Ok(repo) => Some(Arc::new(Mutex::new(repo))),
                Err(e) => {
                    log::warn!("Failed to open repo at {}: {e}", parent.display());
                    None
                }
            };
            if info.repository.is_some() {
                self.cfg.root_priority
            } else {
                0
            }
        } else {
            0
        })
    }
    async fn run_with_path_info(
        &self,
        path_infos: &super::PathInfos,
        mut tx: UpdateSender<Self>,
    ) -> Option<Self::State> {
        if let Some(r) = &path_infos.inner.vcs.repository {
            log_result("git", self.process_once(r, &mut tx).await);
        }
        None
    }
    fn render(&self, _path: &super::PathInfos, state: &State) -> Vec<super::Segment> {
        let Some(desc) = &state.description else {
            return vec![];
        };
        let (fg, bg, status_bit) = match state.dirty {
            Some(Dirty::Dirty { conflicted }) => (
                self.cfg.dirty_fg,
                self.cfg.dirty_bg,
                if conflicted { " ✘" } else { "" },
            ),
            Some(Dirty::Staged) => (self.cfg.staged_fg, self.cfg.staged_bg, ""),
            Some(Dirty::Even) => (self.cfg.clean_fg, self.cfg.clean_bg, ""),
            None => return vec![],
        };
        vec![super::Segment {
            text: format!(" {desc}{status_bit}").into(),
            separator: true,
            style: anstyle::Style::new()
                .fg_color(Some(fg.into()))
                .bg_color(Some(bg.into()))
                .effects(anstyle::Effects::new()),
        }]
    }
}
