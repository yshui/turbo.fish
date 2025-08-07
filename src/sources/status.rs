use serde::{Deserialize, Serialize};

use crate::{
    color::{Ansi, Color, Rgb},
    sources::UpdateSender,
};

#[derive(Debug)]
pub struct Source {
    cfg: Config,
}

fn default_jobs_fg() -> Color {
    Color::Rgb(Rgb {
        r: 0x25,
        g: 0x5e,
        b: 0x87,
    })
}

fn default_nonzero_fg() -> Color {
    Color::Rgb(Rgb {
        r: 0xce,
        g: 0,
        b: 0xf,
    })
}

fn default_jobs_symbol() -> String {
    "%".into()
}

fn default_status_bg() -> Color {
    Color::Ansi(Ansi {
        code: 15,
        named: true,
    })
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    #[serde(default = "default_status_bg")]
    bg: Color,
    #[serde(default = "default_jobs_fg")]
    jobs_fg: Color,
    #[serde(default = "default_nonzero_fg")]
    nonzero_fg: Color,
    #[serde(default)]
    show_count: bool,
    #[serde(default = "default_jobs_symbol")]
    jobs_symbol: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bg: default_status_bg(),
            jobs_fg: default_jobs_fg(),
            nonzero_fg: default_nonzero_fg(),
            show_count: false,
            jobs_symbol: default_jobs_symbol(),
        }
    }
}

#[derive(Deserialize, Serialize, Debug, PartialEq, Eq, Default)]
pub struct State;

impl Source {
    fn render_bg_jobs(&self) -> Option<super::Segment> {
        let bg_jobs = std::env::var("__TURBO_FISH_BG_JOBS").ok()?;

        if bg_jobs == "0" || bg_jobs.is_empty() {
            return None;
        }

        Some(super::Segment {
            text: if self.cfg.show_count {
                bg_jobs
            } else {
                self.cfg.jobs_symbol.clone()
            },
            style: anstyle::Style::new()
                .bg_color(Some(self.cfg.bg.into()))
                .fg_color(Some(self.cfg.jobs_fg.into()))
                .bold(),
            separator: false,
        })
    }
    fn render_nonzero(&self) -> Option<super::Segment> {
        let last_status = std::env::var("__TURBO_FISH_LAST_STATUS").ok()?;

        if last_status == "0" || last_status.is_empty() {
            return None;
        }

        Some(super::Segment {
            text: "!".into(),
            style: anstyle::Style::new()
                .bg_color(Some(self.cfg.bg.into()))
                .fg_color(Some(self.cfg.nonzero_fg.into()))
                .bold(),
            separator: false,
        })
    }
}

impl super::Source for Source {
    type Config = Config;
    type State = State;
    fn new(cfg: &Config, _global_cfg: &super::GlobalConfig, _path: &std::path::Path) -> Self {
        Source { cfg: cfg.clone() }
    }
    async fn start(&self, _: UpdateSender<Self>) -> Option<State> {
        None
    }
    fn render(&self, _path: &std::path::Path, State: &Self::State) -> Vec<super::Segment> {
        let it = self.render_nonzero().into_iter();
        let mut it = it.chain(self.render_bg_jobs());
        let mut joined_segments = Vec::new();
        if let Some(seg) = it.next() {
            joined_segments.push(seg);
        }
        for seg in it {
            joined_segments.push(super::Segment {
                text: " ".into(),
                style: anstyle::Style::new().bg_color(Some(self.cfg.bg.into())),
                separator: false,
            });
            joined_segments.push(seg);
        }
        if let Some(last_seg) = joined_segments.last_mut() {
            last_seg.separator = true;
        }
        joined_segments
    }
}
