use serde::{Deserialize, Serialize};

use crate::{
    color::{Ansi, Color, Rgb},
    sources::UpdateSender,
};
#[derive(Debug)]
pub(crate) struct Source {
    ticker: Vec<char>,
    cfg: Config,
}

impl super::Config {
    pub fn spinner_interval_ms(&self) -> u64 {
        self.spinner.interval_ms
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Default)]
pub(crate) struct State {
    start: i64,
}

fn default_spinner_fg() -> Color {
    Color::Ansi(Ansi {
        code: 7,
        named: true,
    })
}

fn default_spinner_bg() -> Color {
    Color::Rgb(Rgb {
        r: 130,
        g: 20,
        b: 180,
    })
}

fn default_interval_ms() -> u64 {
    100
}

fn default_delay_ms() -> u64 {
    150
}

fn default_ticker() -> String {
    "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ ".into()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct Config {
    #[serde(default = "default_spinner_fg")]
    fg: Color,
    #[serde(default = "default_spinner_bg")]
    bg: Color,
    #[serde(default = "default_interval_ms")]
    interval_ms: u64,
    #[serde(default = "default_delay_ms")]
    delay_ms: u64,
    #[serde(default = "default_ticker")]
    ticker: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fg: default_spinner_fg(),
            bg: default_spinner_bg(),
            delay_ms: default_delay_ms(),
            interval_ms: default_interval_ms(),
            ticker: default_ticker(),
        }
    }
}

impl super::Source for Source {
    type State = State;
    type Config = Config;
    fn new(cfg: &Self::Config, _global_cfg: &super::GlobalConfig, _path: &std::path::Path) -> Self {
        Self {
            cfg: cfg.clone(),
            ticker: cfg.ticker.chars().collect(),
        }
    }
    async fn start(&self, _: UpdateSender<Self>) -> Option<State> {
        let ts = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        Some(State {
            start: ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000,
        })
    }
    fn render(&self, _path: &std::path::Path, state: &Self::State) -> Vec<super::Segment> {
        let ts = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        let now = ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000;
        let elapsed = (now - state.start).unsigned_abs();
        if elapsed < self.cfg.delay_ms {
            return vec![];
        }
        vec![super::Segment {
            text: self.ticker[(elapsed / self.cfg.interval_ms) as usize % self.ticker.len()]
                .to_string(),
            style: anstyle::Style::new()
                .fg_color(Some(self.cfg.fg.into()))
                .bg_color(Some(self.cfg.bg.into())),
            separator: true,
        }]
    }
}
