use ::std::ffi::OsString;
use futures_channel::mpsc::UnboundedSender as Sender;
use futures_util::TryStreamExt;
use paste::paste;
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, ensure_whatever};
use std::{
    borrow::Cow,
    ffi::OsStr,
    marker::PhantomData,
    path::{Component, Path, PathBuf},
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
        $(pub mod $name;)*
        #[derive(Default, Debug, ::serde::Serialize, ::serde::Deserialize)]
        struct PathInfosInner {
            $($name: <$t as Source>::PathInfo),*
        }
        impl PathInfosInner {
            fn init(len: usize) -> Self {
                Self {
                    $($name: <<$t as Source>::PathInfo as PathInfo>::init(len)),*
                }
            }
        }
        #[derive(Debug)]
        pub struct Sources {
            $($name: Option<$t>,)*
        }
        impl Sources {
            fn enabled_count(&self) -> usize {
                0 $(+ if self.$name.is_some() { 1 } else { 0 })*
            }
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
            async fn walk_cwd_one(
                &self,
                parent: &Path,
                current_child: &OsStr,
                children: &[OsString],
                info: &mut PathInfosInner,
                index: usize,
            ) -> Result<u64, $crate::sources::Error> {
                use futures_util::FutureExt as _;
                let &mut PathInfosInner { $(ref mut $name),* } = info;
                let mut count = self.enabled_count();
                $(let $name = async {
                    if let Some(s) = &self.$name {
                        let mut priority = 0;
                        for child in children {
                            priority = priority.max(
                                s.walk_path(
                                    parent,
                                    current_child,
                                    &child,
                                    $name,
                                    index
                                ).await?
                            );
                        }
                        Ok(priority)
                    } else {
                        futures_util::future::pending().await
                    }
                }.fuse();
                futures_util::pin_mut!($name);)*

                let mut priority = 0;
                log::debug!("waiting for {count} sources");
                while count > 0 {
                    futures_util::select! {
                        $(p = $name => {
                            log::debug!("{} completed: {p:?}", stringify!($name));
                            priority = priority.max(p?);
                            count -= 1;
                        }),*
                    }
                }

                Ok(priority)
            }
        }

        $(paste! {
            impl UpdateSender<$name::Source> {
                /// Sending None signals one round of updates has completed.
                #[allow(dead_code, reason = "generic code, might not be used")]
                fn send(&mut self, update: <$name::Source as $crate::sources::Source>::State) {
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

            impl Sources {
                pub async fn run(
                    &self,
                    path_infos: &PathInfos,
                    tx: Sender<Update>,
                ) {
                    use futures_util::FutureExt;
                    let mut count = self.enabled_count();
                    $(
                        let $name = async {
                            if let Some(src) = &self.$name {
                                let update = src
                                    .run(UpdateSender(tx.clone(), PhantomData))
                                    .map(|s| s.map(Update::[<$name:camel>])).await;
                                if let Some(update) = update {
                                    tx.unbounded_send(update).unwrap();
                                }
                                let update = src
                                    .run_with_path_info(path_infos, UpdateSender(tx.clone(), PhantomData))
                                    .map(|s| s.map(Update::[<$name:camel>])).await;
                                if let Some(update) = update {
                                    tx.unbounded_send(update).unwrap();
                                }
                            } else {
                                futures_util::future::pending().await
                            }
                        }.fuse();
                        futures_util::pin_mut!($name);
                    )*
                    while count > 0 {
                        futures_util::select! {
                            $(_ = $name => count -= 1),*
                        }
                    }
                }

                fn render_by_name(
                    &self,
                    path: &PathInfos,
                    state: &State,
                    name: &str
                ) -> Vec<Segment> {
                    match name {
                    $(
                        stringify!($name) => self
                            .$name
                            .as_ref()
                            .unwrap()
                            .render(path, &state.$name),
                    )*
                        _ => unreachable!(),
                    }
                }
            }

            pub fn render(
                cfg: &Config,
                path: &PathInfos,
                state: &State,
            ) -> Vec<Segment> {
                let segments = cfg.segments.clone();
                let sources = Sources::new(cfg, &path.full_path);
                let mut ret = vec![];
                for seg in segments {
                    ret.extend(sources.render_by_name(path, &state, &seg))
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
        hostname: hostname::Source,
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
enum PathRoot {
    Home(PathBuf),
    #[default]
    Root,
}

#[derive(Default, Debug, Serialize, Deserialize)]
struct PathSegment {
    /// Name of the current segment, empty for the root segment.
    name: OsString,
    /// Highlight priority
    priority: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PathInfos {
    segments: Vec<PathSegment>,
    root: PathRoot,
    pub full_path: PathBuf,
    inner: PathInfosInner,
}

impl PathInfos {
    /// Create a state from a path quickly without doing I/O.
    fn new(path: &Path) -> Self {
        let (ret, rest) = Self::empty_impl(path);
        let mut segments = vec![Default::default()];
        for c in rest.components() {
            let c = match c {
                Component::Prefix(_)
                | Component::RootDir
                | Component::CurDir
                | Component::ParentDir => unreachable!("invalid cwd"),
                Component::Normal(c) => c,
            };
            segments.push(PathSegment {
                name: c.to_os_string(),
                ..Default::default()
            });
        }
        let inner = PathInfosInner::init(segments.len());

        Self {
            segments,
            inner,
            ..ret
        }
    }

    fn empty_impl(path: &Path) -> (Self, &Path) {
        let home = std::env::var_os("HOME");
        let (root, rest) = if let Some(home) = home
            && let Ok(path) = path.strip_prefix(&home)
        {
            (PathRoot::Home(home.into()), path)
        } else {
            (PathRoot::Root, path.strip_prefix("/").unwrap())
        };

        (
            Self {
                full_path: path.to_path_buf(),
                root,
                ..Default::default()
            },
            rest,
        )
    }
    pub fn empty(path: &Path) -> Self {
        Self::empty_impl(path).0
    }
}

impl Sources {
    pub async fn walk_cwd(&self, path: &Path) -> Result<PathInfos, Error> {
        let mut infos = PathInfos::new(path);
        let mut curr = path;
        let mut current_child = OsStr::new("");
        log::debug!("starting info {infos:?}");
        for i in (0..infos.segments.len()).rev() {
            let dir = async_fs::read_dir(&curr)
                .await
                .whatever_context("read_dir")?;
            let entries: Vec<_> = dir
                .map_ok(|de| de.file_name())
                .try_collect()
                .await
                .whatever_context("failed to read dir entries")?;
            log::debug!("[{i}]: {} {}", curr.display(), current_child.display());
            if !current_child.is_empty() {
                let found_self = entries.iter().any(|e| e == current_child);
                ensure_whatever!(found_self, "current working directory deleted or renamed");
            }
            infos.segments[i].priority = self
                .walk_cwd_one(curr, current_child, &entries, &mut infos.inner, i)
                .await?;
            if i != 0 {
                current_child = curr.file_name().unwrap();
                curr = curr.parent().unwrap();
            }
        }
        Ok(infos)
    }
}

#[derive(Debug)]
pub struct Segment {
    /// Content of this segment
    pub text: Cow<'static, str>,
    /// Style of this segment
    pub style: anstyle::Style,
    /// Whether to print a separator to the right of this segment
    pub separator: bool,
}

trait PathInfo: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + Default {
    fn init(_len: usize) -> Self {
        Default::default()
    }
}

impl PathInfo for () {}

trait Source: Sized {
    type State: serde::Serialize + serde::de::DeserializeOwned + Default + Eq + std::fmt::Debug;
    type Config: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug;
    type PathInfo: PathInfo;
    /// Returns the source and a root pattern detector.
    fn new(cfg: &Self::Config, global_cfg: &GlobalConfig, path: &Path) -> Self;

    /// Walk down the current working directory path.
    ///
    /// This is called for each level of the current working directory path, and for each child
    /// under each level. This can be used to attach extra per-source information to each level of
    /// the current path. This function is called from bottom up.
    ///
    /// This path walking process is only done once per cwd change.
    ///
    /// # Arguments
    ///
    /// - parent: the current directory being walked, the first call to `walk_path` will be with
    ///   the full current working directory, and the last call will be with either "/" for paths
    ///   outside HOME, or the HOME directory otherwise.
    /// - current_child: the child of `parent` that contains the current working directory. the
    ///   first call to this function is called with `parent == cwd`, in which case `current_child`
    ///   will be empty.
    /// - child: the child currently being walked.
    /// - info: the per-source attached path info.
    ///
    /// # Return
    ///
    /// The priority of `parent`. The final priority of `parent` will be the maximum value across
    /// all children of `parent`, and across all sources.
    #[inline]
    fn walk_path(
        &self,
        _parent: &Path,
        _current_child: &OsStr,
        _child: &OsStr,
        _info: &mut Self::PathInfo,
        _index: usize,
    ) -> impl Future<Output = Result<u64, Error>> {
        futures_util::future::ok(0)
    }

    /// Start a source on a given CWD.
    ///
    /// # Arguments
    ///
    /// - root: the receiver for the result of root search. this will be a closed receiver if
    ///   Self::new returned a `None` as the root pattern detector.
    /// - tx: channel for sending progressive updatas to the main application. It's fine to not
    ///   use this and only return a final state.
    ///
    /// # Return
    ///
    /// The final state. If `None`, the last state update sent to `tx` will be used. If none was
    /// ever sent, then `State::default()` will be used.
    #[inline]
    fn run(
        &self,
        _tx: UpdateSender<Self>,
    ) -> impl std::future::Future<Output = Option<Self::State>> {
        futures_util::future::ready(None)
    }

    /// Start a source on a given CWD, with path info. This is called after path walking has
    /// finished.
    ///
    /// # Arguments
    ///
    /// - root: the receiver for the result of root search. this will be a closed receiver if
    ///   Self::new returned a `None` as the root pattern detector.
    /// - path_infos: CWD broken down into path segments, each segment has `Self::PathInfo`
    ///   attached to it.
    /// - tx: channel for sending progressive updatas to the main application. It's fine to not
    ///   use this and only return a final state.
    ///
    /// # Return
    ///
    /// The final state. If `None`, the last state update sent to `tx` will be used. If none was
    /// ever sent, then `State::default()` will be used.
    #[inline]
    fn run_with_path_info(
        &self,
        _path_infos: &PathInfos,
        _tx: UpdateSender<Self>,
    ) -> impl std::future::Future<Output = Option<Self::State>> {
        futures_util::future::ready(None)
    }

    fn render(&self, path_infos: &PathInfos, state: &Self::State) -> Vec<Segment>;
}
