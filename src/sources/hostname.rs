use anstyle::{Effects, Style};
use serde::{Deserialize, Serialize};

use crate::{
    color::{Ansi, Color, Rgb},
    sources::Segment,
    utils::CowEx,
};

#[derive(Debug)]
pub struct Source {
    cfg: Config,
}
#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
pub struct Config {
    #[serde(default = "default_path_bg")]
    bg: Color,
    #[serde(default = "default_path_fg")]
    fg: Color,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bg: default_path_bg(),
            fg: default_path_fg(),
        }
    }
}

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

impl super::Source for Source {
    type State = ();
    type Config = Config;
    type PathInfo = ();

    fn new(
        &cfg: &Self::Config,
        _global_cfg: &super::GlobalConfig,
        _path: &std::path::Path,
    ) -> Self {
        Self { cfg }
    }

    fn render(&self, _path_infos: &super::PathInfos, _state: &Self::State) -> Vec<Segment> {
        let uname = rustix::system::uname();

        vec![Segment {
            text: uname.nodename().to_string_lossy().to_static(),
            style: Style::new()
                .fg_color(Some(self.cfg.fg.into()))
                .bg_color(Some(self.cfg.bg.into()))
                .effects(Effects::new()),
            separator: true,
        }]
    }
}
