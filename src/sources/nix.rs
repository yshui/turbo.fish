use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Default, PartialEq, Eq)]
pub(crate) struct State;

fn default_nix_bg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0,
        g: 0x5f,
        b: 0xaf,
    })
}
fn default_nix_fg() -> super::Color {
    super::Color::Rgb(super::Rgb {
        r: 0xcc,
        g: 0xcc,
        b: 0xcc,
    })
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy)]
pub struct Config {
    #[serde(default = "default_nix_fg")]
    fg: super::Color,
    #[serde(default = "default_nix_bg")]
    bg: super::Color,
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

impl super::Source for Source {
    type State = State;
    type Config = Config;
    fn new(
        &cfg: &Self::Config,
        _global_cfg: &super::GlobalConfig,
        _path: &std::path::Path,
    ) -> Self {
        Self { cfg }
    }
    async fn start(
        self,
        mut tx: super::UpdateSender<Self>,
        notify: &std::sync::Arc<super::Notify>,
    ) -> ! {
        let mut w = notify.waiter();
        loop {
            tx.send(None).await;
            w.wait().await;
        }
    }

    fn render(&self, _path: &std::path::Path, _state: &Self::State) -> Vec<super::Segment> {
        let Ok(nix) = std::env::var("IN_NIX_SHELL") else {
            return vec![];
        };
        vec![super::Segment {
            text: nix,
            style: anstyle::Style::new()
                .fg_color(Some(self.cfg.fg.into()))
                .bg_color(Some(self.cfg.bg.into()))
                .bold(),
            separator: true,
        }]
    }
}
