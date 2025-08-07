use futures_channel::mpsc::UnboundedSender as Sender;
use futures_util::stream::FuturesUnordered;
use paste::paste;
use serde::{Deserialize, Serialize};
use std::{
    marker::PhantomData,
    path::Path,
};

#[derive(Debug, snafu::Snafu)]
#[snafu(whatever)]
#[snafu(display("{message}\n{backtrace}\n{}", if let Some(source) = source { format!("Caused by:\n{source}") } else { String::new() }))]
#[snafu(provide(opt, ref, chain, dyn std::error::Error + Send + Sync => source.as_deref()))]
pub struct Error {
    #[snafu(source(from(Box<dyn std::error::Error + Send + Sync>, Some)))]
    #[snafu(provide(false))]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
    message: String,
    backtrace: std::backtrace::Backtrace,
}

fn log_result<E: std::fmt::Display>(name: &str, result: Result<(), E>) {
    match result {
        Ok(()) => (),
        Err(e) => log::info!("source '{name}' failed with: {e}"),
    }
}

pub mod nix;
pub mod short_path;
pub mod spinner;
pub mod status;
pub mod vcs;

fn default_separator_chars() -> String {
    "".into()
}

/// Global configs
#[derive(Serialize, Deserialize, Debug)]
pub struct GlobalConfig {
    /// Character used as separators. One or two characters. If two
    /// characters are provided, the second one will be used when the
    /// two adjacent segments have the same background color.
    #[serde(default = "default_separator_chars")]
    pub separator_chars: String,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            separator_chars: default_separator_chars(),
        }
    }
}

struct UpdateSender<Source>(Sender<Update>, PhantomData<Source>);

macro_rules! define_sources {
    (struct Sources { $($name:ident: $t:ty,)* }) => {
        #[derive(Debug)]
        pub struct Sources {
            $($name: Option<$t>,)*
        }
        impl Sources {
            pub fn new(cfg: &Config, path: &Path) -> Self {
                let all_source_names = cfg
                    .segments
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>();
                Self {
                    $($name: all_source_names
                        .contains(stringify!($name))
                        .then(|| <$t as Source>::new(&cfg.$name, &cfg.global_config, path))),*
                }
            }
        }

        $(paste! {
            impl UpdateSender<$name::Source> {
                /// Sending None signals one round of updates has completed.
                #[allow(dead_code, reason = "generic code, might not be used")]
                fn send(&mut self, update: $name::State) {
                    self.0.unbounded_send(Update::[<$name:camel>](update)).unwrap();
                }
            }
        })*

        #[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
        pub struct State {
            $($name: <$t as Source>::State,)*
        }
        #[derive(Debug, Serialize, Deserialize, Default)]
        pub struct Config {
            #[serde(flatten)]
            global_config: GlobalConfig,
            #[serde(skip)]
            pub segments: Vec<String>,
            $(#[serde(default)] $name: <$t as Source>::Config,)*
        }

        #[derive(snafu::Snafu, Debug)]
        pub enum LoadConfigError {
            #[snafu(display("cannot read config file: {source}"), context(false))]
            Io { source: std::io::Error },
            #[snafu(display("cannot parse config file as toml: {source}"), context(false))]
            Parse { source: toml_edit::TomlError },
            #[snafu(display("invalid options in config file: {source}"), context(false))]
            Config { source: toml_edit::de::Error },
        }

        impl Config {
            pub fn load(p: &Path) -> Result<Self, LoadConfigError> {
                let f = std::fs::File::open(p)?;
                let f = std::io::read_to_string(f)?;
                let parsed = toml_edit::Document::parse(f)?;

                let mut cfg: Self = toml_edit::de::from_document(parsed.clone())?;
                cfg.segments = parsed
                    .iter()
                    .filter_map(|(k, v)| v.is_table_like().then(|| k.to_string()))
                    .collect();

                Ok(cfg)
            }
            pub fn global_config(&self) -> &GlobalConfig {
                &self.global_config
            }
        }

        paste! {
            #[derive(Debug, PartialEq, Eq)]
            #[allow(private_interfaces)]
            pub enum Update {
                $([<$name:camel>](<$t as Source>::State)),*
            }

            pub async fn start(
                sources: &Sources,
                tx: Sender<Update>,
            ) {
                use futures_util::StreamExt;
                let mut runner = FuturesUnordered::new();
                $(
                    if let Some(src) = &sources.$name {
                        use futures_util::FutureExt;
                        runner.push(
                            src
                                .start(UpdateSender(tx.clone(), PhantomData))
                                .map(|s| s.map(Update::[<$name:camel>]))
                                .boxed()
                        );
                    }
                )*
                while let Some(update) = runner.next().await {
                    if let Some(update) = update {
                        tx.unbounded_send(update).unwrap();
                    }
                    log::debug!("~~ {}", runner.len());
                }
            }

            fn render_by_name(
                sources: &Sources,
                path: &Path,
                state: &State,
                name: &str
            ) -> Vec<Segment> {
                match name {
                $(
                    stringify!($name) => sources
                        .$name
                        .as_ref()
                        .unwrap()
                        .render(path, &state.$name),
                )*
                    _ => unreachable!(),
                }
            }

            pub fn render(
                cfg: &Config,
                path: &Path,
                state: &State,
            ) -> Vec<Segment> {
                let segments = cfg.segments.clone();
                let sources = Sources::new(cfg, path);
                let mut ret = vec![];
                for seg in segments {
                    ret.extend(render_by_name(&sources, path, &state, &seg))
                }
                ret
            }

            impl State {
                pub fn update(&mut self, update: Update) -> bool {
                    match update {
                        $(Update::[<$name:camel>](inner) => if &self.$name != &inner {
                            self.$name = inner;
                            true
                        } else {
                            false
                        }),*
                    }
                }
            }
        }
    };
}

define_sources! {
    struct Sources {
        vcs: vcs::Source,
        short_path: short_path::Source,
        status: status::Source,
        nix: nix::Source,
        spinner: spinner::Source,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct Ansi {
    code: u8,
    named: bool,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, Copy)]
#[serde(untagged)]
pub enum Color {
    Rgb(Rgb),
    Ansi(Ansi),
}

impl Color {
    /// Convert Self::Ansi into Self::Rgb
    pub fn into_rgb(self) -> Rgb {
        match self {
            Self::Rgb(rgb) => rgb,
            Self::Ansi(ansi) => ansi.into_rgb(),
        }
    }
    pub fn dim(self, frac: f32) -> Self {
        let rgb = self.into_rgb();
        Self::Rgb(Rgb {
            r: (rgb.r as f32 * frac) as u8,
            g: (rgb.g as f32 * frac) as u8,
            b: (rgb.b as f32 * frac) as u8,
        })
    }
    pub fn invert(self) -> Self {
        let rgb = self.into_rgb();
        Self::Rgb(Rgb {
            r: 255 - rgb.r,
            g: 255 - rgb.g,
            b: 255 - rgb.b,
        })
    }
}

impl From<Color> for anstyle::Color {
    fn from(value: Color) -> Self {
        match value {
            Color::Rgb(rgb) => anstyle::Color::Rgb(anstyle::RgbColor(rgb.r, rgb.g, rgb.b)),
            Color::Ansi(ansi) => anstyle::Color::Ansi256(anstyle::Ansi256Color(ansi.code)),
        }
    }
}

impl From<anstyle::Color> for Color {
    fn from(value: anstyle::Color) -> Self {
        match value {
            anstyle::Color::Ansi(ansi) => Self::Ansi(Ansi {
                code: ansi as u8,
                named: true,
            }),
            anstyle::Color::Ansi256(ansi256) => Self::Ansi(Ansi {
                code: ansi256.0,
                named: false,
            }),
            anstyle::Color::Rgb(rgb) => Self::Rgb(Rgb {
                r: rgb.r(),
                g: rgb.g(),
                b: rgb.b(),
            }),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Rgb {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        try {
            let s = <String as serde::Deserialize<'_>>::deserialize(deserializer);

            let s = s?;
            let s = s
                .strip_prefix('#')
                .ok_or_else(|| serde::de::Error::custom(format!("invalid rgb {s}")))?;
            if s.len() != 6 {
                return Err(serde::de::Error::custom(format!("invalid rgb {s}")));
            }

            let r = u8::from_str_radix(&s[..2], 16).map_err(serde::de::Error::custom)?;
            let g = u8::from_str_radix(&s[2..4], 16).map_err(serde::de::Error::custom)?;
            let b = u8::from_str_radix(&s[4..], 16).map_err(serde::de::Error::custom)?;

            Rgb { r, g, b }
        }
    }
}

impl serde::Serialize for Rgb {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b))
    }
}

const ANSI_NAMES: &[&str] = &[
    "black", "red", "green", "yellow", "blue", "purple", "cyan", "white",
];

const BASE_6_COLOR_RAMP: &[u8] = &[0, 95, 135, 175, 215, 255];

impl Ansi {
    pub fn into_rgb(self) -> Rgb {
        if self.code < 16 {
            let base = if self.code > 8 {
                self.code - 8
            } else {
                self.code
            };
            let intensity = if self.code > 8 { 255 } else { 205 };
            Rgb {
                r: (base & 1) * intensity,
                g: (base & 2) / 2 * intensity,
                b: (base & 4) / 4 * intensity,
            }
        } else if self.code < 232 {
            // base-6 encoded rgb
            let base = self.code - 16;
            Rgb {
                b: BASE_6_COLOR_RAMP[(base % 6) as usize],
                g: BASE_6_COLOR_RAMP[(base / 6 % 6) as usize],
                r: BASE_6_COLOR_RAMP[(base / 36) as usize],
            }
        } else {
            // grey scale
            let base = self.code - 232;
            Rgb {
                r: 8 + base * 10,
                g: 8 + base * 10,
                b: 8 + base * 10,
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for Ansi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = <String as serde::Deserialize<'_>>::deserialize(deserializer)?;

        Ok(if let Ok(code) = s.parse::<u8>() {
            Ansi { code, named: false }
        } else {
            let (intense, s) = s
                .strip_prefix("bright")
                .map(|s| (true, s))
                .unwrap_or((false, &s));
            let intense = if intense { 8 } else { 0 };
            Ansi {
                code: ANSI_NAMES
                    .iter()
                    .position(|&e| e == s)
                    .ok_or_else(|| serde::de::Error::custom(format!("invalid color {s}")))?
                    as u8
                    + intense,
                named: true,
            }
        })
    }
}

impl serde::Serialize for Ansi {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if self.named {
            serializer.serialize_str(&format!(
                "{}{}",
                if self.code > 7 { "bright" } else { "" },
                ANSI_NAMES[(self.code & 7) as usize]
            ))
        } else {
            serializer.serialize_u8(self.code)
        }
    }
}

#[derive(Debug)]
pub struct Segment {
    /// Content of this segment
    pub text: String,
    /// Style of this segment
    pub style: anstyle::Style,
    /// Whether to print a separator to the right of this segment
    pub separator: bool,
}

trait Source: Sized {
    type State: serde::Serialize + serde::de::DeserializeOwned + Default + Eq + std::fmt::Debug;
    type Config: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug;
    fn new(cfg: &Self::Config, global_cfg: &GlobalConfig, path: &Path) -> Self;
    /// Start a source on a given CWD.
    ///
    /// # Arguments
    ///
    /// - name: name of the source.
    /// - path: working directory.
    /// - tx: channel for sending progressive updatas to the main application. It's fine to not
    ///       use this and only return a final state.
    /// - notify: used to notify the source to start refreshing.
    ///
    /// # Return
    ///
    /// The final state. If `None`, the last state update sent to `tx` will be used. If none was
    /// ever sent, then `State::default()` will be used.
    fn start(
        &self,
        tx: UpdateSender<Self>,
    ) -> impl std::future::Future<Output = Option<Self::State>>;

    fn render(&self, path: &Path, state: &Self::State) -> Vec<Segment>;
}
