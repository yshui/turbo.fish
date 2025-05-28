use futures_util::StreamExt;
use snafu::ensure;
use std::{
    ffi::OsStr,
    future::Future,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::sources::{GlobalConfig, UpdateSender};
#[derive(Debug)]
pub struct Source {
    path: PathBuf,
    cfg: Config,
}
#[derive(Deserialize, Serialize, Default, Debug, PartialEq, Eq)]
pub struct State {
    short_path: Option<PathBuf>,
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

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
pub struct Config {
    #[serde(default = "default_path_bg")]
    bg: super::Color,
    #[serde(default = "default_path_fg")]
    fg: super::Color,
    /// Whether the last segment should be abbreviated as well.
    #[serde(default)]
    abbrev_last_seg: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bg: default_path_bg(),
            fg: default_path_fg(),
            abbrev_last_seg: false,
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

async fn shorten_path(base: &Path, rest: &Path, result: &mut PathBuf) -> Result<(), Error> {
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
        result.push(OsStr::from_bytes(&c.as_bytes()[..shortened]));
        curr.push(c);
    }
    Ok(())
}

impl super::Source for Source {
    type State = State;
    type Config = Config;
    fn new(&cfg: &Config, _global_config: &GlobalConfig, path: &Path) -> Self {
        Self {
            cfg,
            path: path.to_path_buf(),
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
            (Path::new("~"), PathBuf::from(home), rest)
        } else {
            (
                Path::new("/"),
                "/".into(),
                self.path.strip_prefix("/").unwrap(),
            )
        };
        let rest = rest.to_path_buf();
        let mut wait = notify.waiter();
        async move {
            loop {
                let mut result = result_base.to_path_buf();
                match shorten_path(&base, &rest, &mut result).await {
                    Ok(()) => {
                        log::info!("shortened path: {}", result.display());
                        tx.send(Some(State {
                            short_path: Some(result),
                        }))
                        .await;
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
        let short_path = state.short_path.clone().unwrap_or_else(|| {
            let home = std::env::var_os("HOME");
            if let Some(home) = home
                && let Ok(path) = path.strip_prefix(&home)
            {
                std::path::Path::new("~").join(path)
            } else {
                path.to_path_buf()
            }
        });
        let style = anstyle::Style::new()
            .fg_color(Some(self.cfg.fg.dim(0.9).into()))
            .bg_color(Some(self.cfg.bg.into()))
            .effects(anstyle::Effects::new());
        // render the last segment in bold
        if let Some(short_parent) = short_path.parent()
            && !short_parent.as_os_str().is_empty()
        {
            let last_seg = if self.cfg.abbrev_last_seg {
                short_path.strip_prefix(short_parent).unwrap()
            } else {
                path.strip_prefix(path.parent().unwrap()).unwrap()
            };
            vec![
                super::Segment {
                    text: format!("{}/", short_parent.display()),
                    style,
                    separator: false,
                },
                super::Segment {
                    text: last_seg.to_string_lossy().into(),
                    style: style.fg_color(Some(self.cfg.fg.into())).bold(),
                    separator: true,
                },
            ]
        } else {
            vec![super::Segment {
                text: short_path.to_string_lossy().into(),
                style,
                separator: true,
            }]
        }
    }
}
