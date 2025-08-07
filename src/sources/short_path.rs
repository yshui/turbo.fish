use futures_util::StreamExt;
use snafu::ensure;
use std::{
    ffi::{OsStr, OsString},
    future::Future,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::sources::{GlobalConfig, UpdateSender};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
struct PathSegment {
    full: OsString,
    abbreviated: OsString,
}

#[derive(Debug)]
pub struct Source {
    path: PathBuf,
    highlight_patterns: Vec<glob::Pattern>,
    cfg: Config,
}
#[derive(Deserialize, Serialize, Default, Debug, PartialEq, Eq, Clone)]
pub struct State {
    absolute: bool,
    short_path: Vec<PathSegment>,
    highlight: Option<usize>,
}

impl State {
    fn new_home() -> Self {
        Self {
            absolute: false,
            short_path: vec![PathSegment {
                full: "~".into(),
                abbreviated: "~".into(),
            }],
            highlight: None,
        }
    }
    fn new_absolute() -> Self {
        Self {
            absolute: true,
            short_path: vec![],
            highlight: None,
        }
    }
    /// Create a state from a path quickly without doing I/O.
    fn new_path(path: &Path) -> Self {
        let home = std::env::var_os("HOME");
        let (mut ret, rest) = if let Some(home) = home
            && let Ok(path) = path.strip_prefix(&home)
        {
            (Self::new_home(), path)
        } else {
            (Self::new_absolute(), path.strip_prefix("/").unwrap())
        };

        for c in rest.components() {
            let c = match c {
                Component::Prefix(_)
                | Component::RootDir
                | Component::CurDir
                | Component::ParentDir => unreachable!("invalid cwd"),
                Component::Normal(c) => c,
            };
            ret.short_path.push(PathSegment {
                full: c.to_os_string(),
                abbreviated: c.to_os_string(),
            });
        }

        ret
    }
}

fn default_path_bg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0x33,
        g: 0x33,
        b: 0x33,
    })
}

fn default_path_fg() -> super::Color {
    super::Color::Ansi(super::Ansi {
        code: 7,
        named: true,
    })
}

fn default_highlight_patterns() -> Vec<String> {
    [
        ".git",
        ".clangd",
        "compile_commands.json",
        "requirements.txt",
        "pyproject.toml",
        "Cargo.toml",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect()
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_path_bg")]
    bg: super::Color,
    #[serde(default = "default_path_fg")]
    fg: super::Color,
    /// Whether the last component of the path should be abbreviated as well.
    #[serde(default)]
    abbreviate_basename: bool,
    /// The nearest ancestor containing a file matching one of these patterns
    /// will be highlighted. For example, this can be a root of a git repository.
    /// Highlighted segment is displayed in bold and not abbreviated.
    /// Supports globbing.
    #[serde(default = "default_highlight_patterns")]
    highlight_patterns: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bg: default_path_bg(),
            fg: default_path_fg(),
            abbreviate_basename: false,
            highlight_patterns: default_highlight_patterns(),
        }
    }
}

#[derive(Debug, snafu::Snafu)]
enum Error {
    #[snafu(context(false))]
    Io {
        source: std::io::Error,
        backtrace: std::backtrace::Backtrace,
    },
    #[snafu(display("{} has no child named {}", parent.display(), child.display()))]
    NonExistentChild { parent: PathBuf, child: PathBuf },
}

impl Source {
    async fn shorten_path(
        &self,
        base: &Path,
        rest: &Path,
        result: &mut State,
    ) -> Result<(), Error> {
        let mut curr = base.to_path_buf();
        for c in rest.components() {
            let c = match c {
                Component::Prefix(_)
                | Component::RootDir
                | Component::CurDir
                | Component::ParentDir => unreachable!("invalid cwd"),
                Component::Normal(c) => c,
            };
            let mut dir = async_fs::read_dir(&curr).await?;
            let mut path_prefix = 1;
            let mut found_self = false;
            while let Some(e) = dir.next().await {
                let e = e?;
                let fname = e.file_name();
                if self
                    .highlight_patterns
                    .iter()
                    .any(|p| p.matches_path(Path::new(&fname)))
                {
                    log::debug!(
                        "highlight is {}, because {}",
                        curr.display(),
                        fname.display()
                    );
                    result.highlight = Some(result.short_path.len() - 1);
                }
                if fname == c {
                    found_self = true;
                    continue;
                }
                log::trace!("{} {}", c.display(), fname.display());
                let fname = fname.as_bytes();
                let c = c.as_bytes();
                if path_prefix <= fname.len()
                    && path_prefix < c.len()
                    && c[..path_prefix] == fname[..path_prefix]
                {
                    // use path_prefix - 1, so the first byte always matches,
                    // path_prefix will always be incremented, avoiding a special case for
                    // fname.len() == path_prefix
                    for (cch, fnamech) in c[path_prefix - 1..]
                        .iter()
                        .zip(fname[path_prefix - 1..].iter())
                    {
                        if cch != fnamech {
                            break;
                        }
                        path_prefix += 1;
                    }
                }
            }
            ensure!(
                found_self,
                NonExistentChildSnafu {
                    parent: curr,
                    child: c
                }
            );
            // if component is valid utf8, we truncate it to closest char boundary, otherwise
            // it's shortened to path_prefix bytes.
            let shortened = if let Ok(c) = str::from_utf8(c.as_bytes()) {
                c.ceil_char_boundary(path_prefix)
            } else {
                path_prefix
            };
            result.short_path.push(PathSegment {
                abbreviated: OsStr::from_bytes(&c.as_bytes()[..shortened]).to_os_string(),
                full: c.to_os_string(),
            });
            curr.push(c);
        }
        let mut dir = async_fs::read_dir(&curr).await?;
        while let Some(e) = dir.next().await {
            let e = e?;
            let fname = e.file_name();
            if self
                .highlight_patterns
                .iter()
                .any(|p| p.matches_path(Path::new(&fname)))
            {
                log::debug!(
                    "highlight is {}, because {}",
                    curr.display(),
                    fname.display()
                );
                result.highlight = Some(result.short_path.len() - 1);
                break;
            }
        }
        Ok(())
    }
}

impl super::Source for Source {
    type State = State;
    type Config = Config;
    fn new(cfg: &Config, _global_config: &GlobalConfig, path: &Path) -> Self {
        log::debug!("{:?}", cfg.highlight_patterns);
        let highlight_patterns = cfg
            .highlight_patterns
            .iter()
            .filter_map(|p| match p.parse() {
                Ok(p) => Some(p),
                Err(e) => {
                    log::warn!("Cannot parse pattern {p}: {e}, ignoring");
                    None
                }
            })
            .collect();
        Self {
            cfg: cfg.clone(),
            path: path.to_path_buf(),
            highlight_patterns,
        }
    }
    fn start(
        self,
        mut tx: UpdateSender<Source>,
        notify: &std::sync::Arc<super::Notify>,
    ) -> impl Future<Output = !> {
        let (result_base, base, rest) = if let Some(home) = std::env::var_os("HOME")
            && let Ok(rest) = self.path.strip_prefix(Path::new(&home))
        {
            (State::new_home(), PathBuf::from(home), rest)
        } else {
            (
                State::new_absolute(),
                "/".into(),
                self.path.strip_prefix("/").unwrap(),
            )
        };
        let rest = rest.to_path_buf();
        let mut wait = notify.waiter();
        async move {
            loop {
                let mut result = result_base.clone();
                match self.shorten_path(&base, &rest, &mut result).await {
                    Ok(()) => {
                        log::debug!("shortened path: {:?}", result);
                        tx.send(Some(result)).await;
                        tx.send(None).await;
                    }
                    Err(e) => {
                        log::warn!("failed to shorten path: {e}");
                    }
                }
                wait.wait().await;
            }
        }
    }
    fn render(&self, path: &Path, state: &State) -> Vec<super::Segment> {
        let state = if !state.absolute && state.short_path.is_empty() {
            // state is not ready yet, use unabbreviated path
            State::new_path(path)
        } else {
            state.clone()
        };
        let mut ret = Vec::new();
        let highlight = if let Some(h) = state.highlight {
            h
        } else {
            state.short_path.len()
        };

        let mut before_highlight = String::new();
        for i in 0..highlight {
            if state.absolute || !before_highlight.is_empty() {
                before_highlight.push('/')
            }
            before_highlight.push_str(
                &(if i == state.short_path.len() - 1 && !self.cfg.abbreviate_basename {
                    &state.short_path[i].full
                } else {
                    &state.short_path[i].abbreviated
                })
                .to_string_lossy(),
            );
        }
        if highlight < state.short_path.len() && (state.absolute || !before_highlight.is_empty()) {
            before_highlight.push('/')
        }
        if !before_highlight.is_empty() {
            ret.push(super::Segment {
                text: before_highlight,
                style: anstyle::Style::new()
                    .fg_color(Some(self.cfg.fg.dim(0.9).into()))
                    .bg_color(Some(self.cfg.bg.into()))
                    .effects(anstyle::Effects::new()),
                separator: false,
            });
        }
        if highlight < state.short_path.len() {
            ret.push(super::Segment {
                text: state.short_path[highlight]
                    .full
                    .to_string_lossy()
                    .into_owned(),
                style: anstyle::Style::new()
                    .fg_color(Some(self.cfg.fg.into()))
                    .bg_color(Some(self.cfg.bg.into()))
                    .bold(),
                separator: false,
            })
        }
        let mut after_highlight = String::new();
        for i in highlight + 1..state.short_path.len() {
            after_highlight.push('/');
            after_highlight.push_str(
                &(if i == state.short_path.len() - 1 && !self.cfg.abbreviate_basename {
                    &state.short_path[i].full
                } else {
                    &state.short_path[i].abbreviated
                })
                .to_string_lossy(),
            );
        }
        if !after_highlight.is_empty() {
            ret.push(super::Segment {
                text: after_highlight,
                style: anstyle::Style::new()
                    .fg_color(Some(self.cfg.fg.dim(0.9).into()))
                    .bg_color(Some(self.cfg.bg.into()))
                    .effects(anstyle::Effects::new()),
                separator: false,
            })
        }
        ret.last_mut().unwrap().separator = true;
        ret
    }
}
