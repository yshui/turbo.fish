use serde::{Deserialize, Serialize};

use crate::color::{Color, Rgb};

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub(crate) struct State;

fn default_nix_bg() -> Color {
    Color::Rgb(Rgb {
        r: 0,
        g: 0x5f,
        b: 0xaf,
    })
}
fn default_nix_fg() -> Color {
    Color::Rgb(Rgb {
        r: 0xcc,
        g: 0xcc,
        b: 0xcc,
    })
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Config {
    #[serde(default = "default_nix_fg")]
    fg: Color,
    #[serde(default = "default_nix_bg")]
    bg: Color,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            fg: default_nix_fg(),
            bg: default_nix_bg(),
        }
    }
}

#[derive(Debug)]
pub struct Source {
    cfg: Config,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub(crate) struct PathInfo;

impl super::PathInfo for PathInfo {}

impl super::Source for Source {
    type State = State;
    type Config = Config;
    type PathInfo = PathInfo;
    fn new(
        &cfg: &Self::Config,
        _global_cfg: &super::GlobalConfig,
        _path: &std::path::Path,
    ) -> Self {
        Self { cfg }
    }

    fn render(&self, _path: &super::PathInfos, _state: &Self::State) -> Vec<super::Segment> {
        let Ok(nix) = std::env::var("IN_NIX_SHELL") else {
            return vec![];
        };
        vec![super::Segment {
            text: nix.into(),
            style: anstyle::Style::new()
                .fg_color(Some(self.cfg.fg.into()))
                .bg_color(Some(self.cfg.bg.into()))
                .bold(),
            separator: true,
        }]
    }
}
