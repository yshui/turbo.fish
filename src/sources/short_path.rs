use snafu::ResultExt;
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

use serde::{Deserialize, Serialize};

use crate::{
    color::{Ansi, Color, Rgb},
    sources::{GlobalConfig, PathRoot, Segment},
};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Default)]
pub(crate) struct PathInfo {
    /// For all siblings of the current path segment, that shares a common
    /// prefix, this is the length of the longest one.
    abbreviated: Vec<usize>,
}

impl super::PathInfo for PathInfo {
    fn init(len: usize) -> Self {
        Self {
            abbreviated: vec![0; len],
        }
    }
}

#[derive(Debug)]
pub struct Source {
    root_patterns: Vec<RootGlob>,
    cfg: Config,
}
#[derive(Deserialize, Serialize, Default, Debug, PartialEq, Eq, Clone)]
pub struct State;

fn default_path_bg() -> Color {
    Color::Rgb(Rgb {
        r: 0x33,
        g: 0x33,
        b: 0x33,
    })
}

fn default_path_fg() -> Color {
    Color::Ansi(Ansi {
        code: 7,
        named: true,
    })
}

fn default_root_patterns() -> Vec<RootPattern> {
    [
        ".git",
        ".clangd",
        "compile_commands.json",
        "requirements.txt",
        "pyproject.toml",
        "Cargo.toml",
    ]
    .into_iter()
    .map(|s| RootPattern::Simple(s.into()))
    .collect()
}

fn default_root_priority() -> u64 {
    10
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct RootPatternWithPriority {
    glob: String,
    priority: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
enum RootPattern {
    Simple(String),
    WithPriority(RootPatternWithPriority),
}

#[derive(Debug)]
struct RootGlob {
    glob: glob::Pattern,
    priority: u64,
}

impl RootPattern {
    fn parse(&self, priority: u64) -> Result<RootGlob, super::Error> {
        let (s, priority) = match self {
            Self::Simple(s) => (s, priority),
            Self::WithPriority(p) => (&p.glob, p.priority),
        };
        Ok(RootGlob {
            glob: s.parse().whatever_context("invalid root pattern")?,
            priority,
        })
    }
    fn pattern(&self) -> &str {
        match self {
            Self::Simple(s) => s,
            Self::WithPriority(p) => &p.glob,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_path_bg")]
    bg: Color,
    #[serde(default = "default_path_fg")]
    fg: Color,
    /// Whether the last component of the path should be abbreviated as well.
    #[serde(default)]
    abbreviate_basename: bool,
    /// The nearest ancestor containing a file matching one of these patterns
    /// will be highlighted. For example, this can be a root of a git repository.
    /// Highlighted segment is displayed in bold and not abbreviated.
    /// Supports globbing.
    #[serde(default = "default_root_patterns")]
    root_patterns: Vec<RootPattern>,
    #[serde(default = "default_root_priority")]
    root_priority: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bg: default_path_bg(),
            fg: default_path_fg(),
            abbreviate_basename: false,
            root_patterns: default_root_patterns(),
            root_priority: default_root_priority(),
        }
    }
}

impl super::Source for Source {
    type State = State;
    type Config = Config;
    type PathInfo = PathInfo;
    fn new(cfg: &Config, _global_config: &GlobalConfig, _path: &Path) -> Self {
        log::debug!("{:?}", cfg.root_patterns);
        let root_patterns = cfg
            .root_patterns
            .iter()
            .filter_map(|p| match p.parse(cfg.root_priority) {
                Ok(p) => Some(p),
                Err(e) => {
                    log::warn!("Cannot parse pattern {}: {e}, ignoring", p.pattern());
                    None
                }
            })
            .collect();
        Self {
            cfg: cfg.clone(),
            root_patterns,
        }
    }
    fn walk_path(
        &self,
        _parent: &Path,
        current_child: &OsStr,
        child: &OsStr,
        info: &mut PathInfo,
        index: usize,
    ) -> impl Future<Output = Result<u64, super::Error>> {
        let priority = self
            .root_patterns
            .iter()
            .filter_map(|p| p.glob.matches_path(Path::new(child)).then_some(p.priority))
            .max()
            .unwrap_or_default();

        if current_child == child {
            return futures_util::future::ok(priority);
        }

        let current_child = current_child.as_bytes();
        let child = child.as_bytes();
        let prefix_len = info.abbreviated.get_mut(index).unwrap();

        if *prefix_len < current_child.len().min(child.len())
            && current_child[..*prefix_len] == child[..*prefix_len]
        {
            for (ch1, ch2) in child[*prefix_len..]
                .iter()
                .zip(current_child[*prefix_len..].iter())
            {
                if ch1 != ch2 {
                    break;
                }
                *prefix_len += 1
            }
        }
        futures_util::future::ok(priority)
    }
    fn render(&self, path: &super::PathInfos, _state: &State) -> Vec<super::Segment> {
        let normal_style = anstyle::Style::new()
            .fg_color(Some(self.cfg.fg.dim(0.9).into()))
            .bg_color(Some(self.cfg.bg.into()))
            .effects(anstyle::Effects::new());

        if path.segments.is_empty() {
            return vec![super::Segment {
                text: if let PathRoot::Home(home) = &path.root {
                    format!("~/{}", path.full_path.strip_prefix(home).unwrap().display())
                } else {
                    path.full_path.display().to_string()
                }
                .into(),
                style: normal_style,
                separator: true,
            }];
        }
        let mut ret = Vec::new();
        let max_priority = path.segments.iter().map(|s| s.priority).max().unwrap();
        let highlight_pos = (max_priority > 0).then(|| {
            path.segments
                .iter()
                .rposition(|s| s.priority == max_priority)
                .unwrap()
        });

        let highlight_style = anstyle::Style::new()
            .fg_color(Some(self.cfg.fg.into()))
            .bg_color(Some(self.cfg.bg.into()))
            .bold();

        if let PathRoot::Home(_) = path.root {
            ret.push(Segment {
                text: "~".into(),
                style: if highlight_pos == Some(0) {
                    highlight_style
                } else {
                    normal_style
                },
                separator: false,
            });
        }
        for i in 1..path.segments.len() {
            let seg = &path.segments[i];
            ret.push(Segment {
                text: "/".into(),
                style: normal_style,
                separator: false,
            });
            // Any segments that were considered any kind of root by any of the sources will
            // be displayed in full, highlighted segment will be bold, and the last segment
            // will be shown in full unconditionally.
            if highlight_pos == Some(i) || seg.priority > 0 || i == path.segments.len() - 1 {
                ret.push(Segment {
                    text: seg.name.to_string_lossy().into_owned().into(),
                    style: if highlight_pos == Some(i) {
                        highlight_style
                    } else {
                        normal_style
                    },
                    separator: false,
                })
            } else {
                let mut shortend = path.inner.short_path.abbreviated[i - 1];
                // abbreviated[i] is the length of the longest common prefix, so to disambiguate,
                // we must include one more byte if there is one.
                if shortend < seg.name.as_bytes().len() {
                    shortend += 1;
                }
                // if segment is valid utf8, we truncate it to closest char boundary, otherwise
                // it's shortened to path_prefix bytes.
                let shortened = if let Ok(c) = str::from_utf8(seg.name.as_bytes()) {
                    c[..c.ceil_char_boundary(shortend)].to_string()
                } else {
                    let bytes = &seg.name.as_bytes()[..shortend];
                    OsStr::from_bytes(bytes).to_string_lossy().into_owned()
                };

                ret.push(Segment {
                    text: shortened.into(),
                    style: normal_style,
                    separator: false,
                })
            }
        }
        ret.last_mut().unwrap().separator = true;
        ret
    }
}
