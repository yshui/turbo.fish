#![feature(try_blocks)]
use async_signal_with_info::Signal;
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, whatever};
use std::{
    ffi::c_void,
    io::ErrorKind,
    os::{
        fd::{AsFd as _, AsRawFd as _, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

struct AutoDeleteTmp<'a>(Option<&'a std::path::Path>);
impl Drop for AutoDeleteTmp<'_> {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            rustix::fs::unlinkat(&*TMPDIR_FD, p, AtFlags::empty()).ok();
        }
    }
}

type Whatever = turbofish::sources::Error;

use bincode::error::EncodeError;
use futures_util::{FutureExt, StreamExt, pin_mut, select, task::LocalSpawnExt};
use rand::Rng;
use rustix::{
    event::{PollFd, PollFlags},
    fs::{AtFlags, CWD, Mode, OFlags, Timespec},
    process::{Pid, PidfdFlags},
};

static TMPDIR_FD: LazyLock<OwnedFd> = LazyLock::new(|| {
    let p = std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into());
    let p = std::path::Path::new(&p).join("turbo.fish");
    if !p.exists() {
        std::fs::create_dir(&p).unwrap();
    } else if !p.is_dir() {
        panic!("state directory {} is not a directory", p.display());
    }
    rustix::fs::openat(CWD, p, OFlags::DIRECTORY, Mode::empty()).unwrap()
});

mod utils {
    use std::{
        io::{IoSlice, IoSliceMut},
        os::fd::OwnedFd,
    };

    pub struct Fd(OwnedFd);
    impl Fd {
        // SAFETY: must be a writeable file descriptor.
        pub unsafe fn new(fd: OwnedFd) -> Self {
            Self(fd)
        }
        #[allow(dead_code, reason = "might be used later")]
        pub fn into_fd(self) -> OwnedFd {
            self.0
        }
    }

    impl rustix::fd::AsFd for Fd {
        fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
            self.0.as_fd()
        }
    }

    impl std::io::Write for Fd {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.write_vectored(&[IoSlice::new(buf)])
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> std::io::Result<usize> {
            Ok(rustix::io::writev(&self.0, bufs)?)
        }
    }

    impl std::io::Read for Fd {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.read_vectored(&mut [IoSliceMut::new(buf)])
        }
        fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> std::io::Result<usize> {
            Ok(rustix::io::readv(&self.0, bufs)?)
        }
    }
}

/// Get the state file as a file name under "$TMPDIR/turbo.fish/"
fn state_file_path() -> Result<PathBuf, Whatever> {
    let tmpdir = std::path::Path::new(&std::env::var_os("TMPDIR").unwrap_or_else(|| "/tmp".into()))
        .join("turbo.fish");
    let path: PathBuf = std::env::args().nth(2).unwrap().into();
    Ok(if let Ok(tmpfile) = path.strip_prefix(&tmpdir) {
        tmpfile.to_path_buf()
    } else if path.components().count() == 1 {
        path
    } else {
        whatever!(
            "invalid tmpfile path {}, tmpdir {}",
            path.display(),
            tmpdir.display()
        )
    })
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct State {
    path: PathBuf,
    inner: turbofish::sources::State,
    /// A version tag, `None` if update is still in progress. Only makes sense
    /// if matches the cwd of the renderer, because otherwise the renderer will disregard
    /// this `State`.
    version: Option<u32>,
}

macro_rules! bail_whatever {
    ($source:expr, $fmt:literal$(, $($arg:expr),* $(,)?)*) => {
        return Err(snafu::FromString::with_source($source.into(), format!($fmt$(, $($arg),*)*)))
    };
}

fn write_state_file(path: &Path, data: &State) -> Result<(), Whatever> {
    let f = rustix::fs::openat(
        &*TMPDIR_FD,
        ".",
        OFlags::TMPFILE | OFlags::RDWR,
        Mode::RUSR | Mode::WUSR,
    )
    .whatever_context("open state file")?;
    let mut f = unsafe { utils::Fd::new(f) };
    match bincode::serde::encode_into_std_write(data, &mut f, bincode::config::standard()) {
        Ok(_) => Ok(()),
        Err(EncodeError::Io { inner, .. }) => whatever!(Err(inner), "read state file"),
        Err(EncodeError::UnexpectedEnd) => whatever!("storage full"),
        Err(e) => panic!("{e}"),
    }?;

    let original_file = rustix::fs::fstat(&f).whatever_context("fstat")?;
    let source_path = format!("/proc/self/fd/{}", f.as_fd().as_raw_fd());
    loop {
        let name: String = rand::rng()
            .sample_iter(&rand::distr::Alphanumeric)
            .take(16)
            .map(char::from)
            .collect();
        match rustix::fs::linkat(
            CWD,
            &source_path,
            &*TMPDIR_FD,
            &name,
            AtFlags::SYMLINK_FOLLOW,
        ) {
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => bail_whatever!(e, "linkat"),
            Ok(_) => (),
        }

        let mut auto_del = AutoDeleteTmp(Some(std::path::Path::new(&name)));

        rustix::fs::renameat(&*TMPDIR_FD, &name, &*TMPDIR_FD, path).whatever_context("rename")?;
        let new_file =
            rustix::fs::statat(&*TMPDIR_FD, path, AtFlags::empty()).whatever_context("statat")?;
        if new_file.st_dev != original_file.st_dev || new_file.st_ino != original_file.st_ino {
            continue;
        }

        auto_del.0.take();
        break;
    }
    Ok(())
}
fn read_state_file(path: &Path, cwd: &Path) -> Result<State, Whatever> {
    try {
        log::debug!("{}", path.display());
        let f = rustix::fs::openat(
            &*TMPDIR_FD,
            path.as_os_str().as_bytes(),
            OFlags::RDONLY,
            Mode::RUSR | Mode::WUSR,
        )
        .whatever_context("open state file")?;
        let mut f = unsafe { utils::Fd::new(f) };
        let state: State =
            bincode::serde::decode_from_std_read(&mut f, bincode::config::standard())
                .whatever_context("decode state file")?;
        if state.path == cwd {
            state
        } else {
            State {
                path: cwd.to_path_buf(),
                ..Default::default()
            }
        }
    }
}
fn shell_is_alive(pidfd: &OwnedFd) -> Result<bool, Whatever> {
    match rustix::event::poll(
        &mut [PollFd::new(&pidfd, PollFlags::IN)],
        Some(&Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        }),
    ) {
        Ok(0) => Ok(true),
        Ok(_) => Ok(false),
        Err(e) => bail_whatever!(e, "poll"),
    }
}
async fn serve_once(
    cfg: &turbofish::sources::Config,
    state_file: &Path,
    pid: Pid,
    pg: &OwnedFd,
) -> Result<bool, Whatever> {
    let mut version = 0u32;
    let mut status = State {
        path: std::fs::read_link(format!("/proc/{}/cwd", pid.as_raw_nonzero()))
            .whatever_context("read_link cwd")?,
        inner: turbofish::sources::State::default(),
        version: None,
    };
    if !shell_is_alive(pg)? {
        log::info!("shell exited, quiting");
        return Ok(true);
    }
    let sources = turbofish::sources::Sources::new(cfg, &status.path);

    let mut signal =
        async_signal_with_info::Signals::new([Signal::Usr1, Signal::Usr2, Signal::Hup])
            .whatever_context("create notifying signal")?
            .fuse();
    'outer: loop {
        let mut timer = <_ as StreamExt>::fuse(async_io::Timer::interval(Duration::from_millis(
            cfg.spinner_interval_ms(),
        )));
        let (tx, rx) = async_channel::unbounded();
        let mut jobs = turbofish::sources::start(&sources, tx).boxed().fuse();
        pin_mut!(rx);
        loop {
            let updated = select! {
                s = signal.next() => {
                    let (sig, info) = s
                        .whatever_context("signal stream ended")?;
                    let sig = sig
                        .whatever_context("error reading signal")?;
                    let new_path = std::fs::read_link(format!("/proc/{}/cwd", pid.as_raw_nonzero()))
                        .whatever_context("read_link cwd")?;
                    if !shell_is_alive(pg)? || new_path != status.path {
                        break 'outer;
                    }
                    match sig {
                        Signal::Usr1 => {
                            status.version = None;
                            write_state_file(state_file, &status)?;
                            version = version.wrapping_add(1);
                            rustix::process::pidfd_send_signal(pg, rustix::process::Signal::USR1)
                                .whatever_context("signal shell redraw")?;
                            // restart source updates. if `jobs` haven't completed yet, they will
                            // be dropped and cancelled. update channels will be recreated too, to
                            // avoid stale updates confusing us.
                            break;
                        },
                        Signal::Usr2 => {
                            if unsafe { info.si_value().sival_ptr } as usize as u32 == version {
                                // We only stop the timer if the renderer has rendered the latest
                                // version of the state file.
                                timer.get_mut().set_interval(Duration::MAX);
                            }
                            false
                        },
                        Signal::Hup => {
                            log::info!("got SIGHUP, quiting");
                            return Ok(true)
                        },
                        _ => unreachable!()
                    }
                },
                _ = timer.next() => true,
                msg = rx.next() => {
                    log::debug!("{msg:?}");
                    if let Some(msg) = msg {
                        status.inner.update(msg);
                        write_state_file(state_file, &status)?;
                        true
                    } else {
                        false
                    }
                }
                // `jobs` completion means all update jobs are finished, we can set the version
                // tag in state.
                _ = jobs => {
                    status.version = Some(version);
                    write_state_file(state_file, &status)?;
                    true
                }
            };
            if updated {
                rustix::process::pidfd_send_signal(pg, rustix::process::Signal::USR1)
                    .whatever_context("signal shell redraw")?;
            }
        }
    }
    Ok(false)
}

async fn serve(cfg: turbofish::sources::Config, state_file: PathBuf) {
    let _cleanup = AutoDeleteTmp(Some(&state_file));
    let ppid = rustix::process::getppid().unwrap();
    let pg = rustix::process::pidfd_open(ppid, PidfdFlags::empty()).unwrap();
    loop {
        match serve_once(&cfg, &state_file, ppid, &pg).await {
            Err(e) => log::error!("serve failed with {e}"),
            Ok(false) => (),
            Ok(true) => break,
        }

        if shell_is_alive(&pg).unwrap() {
            log::info!("resettng...");
        } else {
            log::info!("parent shell terminated");
            break;
        }
    }
}

async fn debug_state(
    cfg: turbofish::sources::Config,
    path: Option<PathBuf>,
) -> Result<(), Whatever> {
    let (tx, rx) = async_channel::unbounded();
    let pid = rustix::process::getppid().unwrap();

    log::debug!("{path:?}");
    let mut status = State {
        path: if let Some(path) = path {
            path
        } else {
            std::fs::read_link(format!("/proc/{}/cwd", pid.as_raw_nonzero()))
                .whatever_context("read_link cwd")?
        },
        inner: turbofish::sources::State::default(),
        version: None,
    };
    let sources = turbofish::sources::Sources::new(&cfg, &status.path);
    turbofish::sources::start(&sources, tx).await;

    while let Ok(msg) = rx.recv().await {
        log::debug!("{msg:?}");
        status.inner.update(msg);
    }
    status.version = Some(0);
    log::debug!("final state: {status:?}");
    write_state_file(
        std::env::args()
            .nth(2)
            .whatever_context("not enough arguments")?
            .as_ref(),
        &status,
    )?;
    Ok(())
}

#[allow(non_camel_case_types, dead_code, reason = "ffi type")]
#[repr(C)]
union sigval {
    sival_int: i32,
    sival_ptr: *const c_void,
}

unsafe extern "C" {
    fn sigqueue(pid: i32, sig: i32, value: sigval) -> i32;
}

fn render(
    mut cfg: turbofish::sources::Config,
    state_file: PathBuf,
    path: Option<PathBuf>,
) -> Result<(), Whatever> {
    let ppid = rustix::process::getppid().unwrap();
    let separator_chars = cfg
        .global_config()
        .separator_chars
        .chars()
        .collect::<Vec<_>>();
    let turbofish_pid = std::env::var("__TURBO_FISH_PID")
        .ok()
        .and_then(|pid| pid.parse().ok())
        .and_then(rustix::process::Pid::from_raw);
    let path = if let Some(path) = path {
        path
    } else {
        std::fs::read_link(format!("/proc/{}/cwd", ppid.as_raw_nonzero()))
            .whatever_context("read cwd")?
    };
    let state = read_state_file(&state_file, &path).unwrap_or_default();
    log::debug!("{state:?}");
    if state.version.is_some() {
        // remove the spinner if completed
        cfg.segments.retain(|s| s != "spinner");
    }
    let mut segments = turbofish::sources::render(&cfg, &path, &state.inner);
    log::debug!("{segments:?}");
    segments.push(turbofish::sources::Segment {
        text: String::new(),
        style: anstyle::Style::new(),
        separator: false,
    });
    if let Some(first) = segments.first() {
        print!("{} ", first.style);
    }
    for segment_pair in segments.windows(2) {
        let &[segment, next_segment] = &segment_pair else {
            unreachable!()
        };
        print!("{}", segment.text);

        if !segment.separator {
            print!("{:#}{}", segment.style, next_segment.style);
            continue;
        }
        if segment.style.get_bg_color() == next_segment.style.get_bg_color()
            && separator_chars.len() > 1
        {
            let fg = segment
                .style
                .get_bg_color()
                .map(turbofish::sources::Color::from)
                .map(|c| c.invert().into())
                .unwrap_or(anstyle::AnsiColor::BrightWhite.into());
            let separator_style = anstyle::Style::new()
                .bg_color(segment.style.get_bg_color())
                .fg_color(Some(fg));
            print!(
                " {:#}{separator_style}{}{separator_style:#}{} ",
                segment.style, separator_chars[1], next_segment.style
            );
        } else if !separator_chars.is_empty() {
            let separator_style = anstyle::Style::new()
                .fg_color(segment.style.get_bg_color())
                .bg_color(next_segment.style.get_bg_color());
            print!(
                " {:#}{separator_style}{}{} ",
                segment.style, separator_chars[0], next_segment.style
            );
        } else {
            print!(" {:#}{} ", segment.style, next_segment.style);
        }
    }
    println!();

    if let Some(version) = state.version
        && let Some(turbofish_pid) = turbofish_pid
    {
        unsafe {
            sigqueue(
                turbofish_pid.as_raw_nonzero().get(),
                Signal::Usr2 as i32,
                sigval {
                    sival_int: version as i32,
                },
            );
        };
    }

    Ok(())
}

const INIT_FISH: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/init.fish"));

fn print_init() -> Result<(), Whatever> {
    let self_exe =
        std::fs::read_link("/proc/self/exe").whatever_context("failed to get self exe path")?;
    let self_exe = self_exe
        .to_str()
        .whatever_context("self exe path not valid string")?;
    let init_fish = INIT_FISH.replace("::TURBOFISH::", self_exe);
    print!("{init_fish}");
    Ok(())
}

fn main() {
    env_logger::init();
    let xdg = xdg::BaseDirectories::new();
    let err: Result<(), Whatever> = try {
        let cfg: turbofish::sources::Config = turbofish::sources::Config::load(
            &xdg.find_config_file("turbo.fish/config.toml")
                .whatever_context("no config file found")?,
        )
        .whatever_context("load config")?;
        log::debug!("{cfg:?}");
        let verb = std::env::args()
            .nth(1)
            .whatever_context("No enough arguments")?;

        if verb == "init" {
            return print_init()?;
        }

        let mut ex = futures_executor::LocalPool::new();
        let state_file = state_file_path()?;
        let spawn = ex.spawner();
        let opt_path = std::env::args().nth(3).map(Into::into);
        match verb.as_str() {
            "serve" => spawn
                .spawn_local(serve(cfg, state_file))
                .whatever_context("spawn")?,
            "render" => render(cfg, state_file, opt_path)?,
            "debug_state" => futures_executor::block_on(debug_state(cfg, opt_path))?,
            _ => Err(snafu::FromString::without_source(format!(
                "Invalid verb {verb}"
            )))?,
        }

        ex.run();
    };
    if let Err(e) = err {
        log::error!("{e}")
    }
}
