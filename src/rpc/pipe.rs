//! The listener (docs/RPC_DESIGN.md §5): the lowest free `<base>-N`, N = 0..9, and never one
//! someone else holds. Windows: a named pipe created with `first_pipe_instance`, so an existing
//! pipe of that name (Discord, arRPC, another Rustcord) is refused without connecting to it.
//! Unix: a socket in the first set of `XDG_RUNTIME_DIR`, `TMPDIR`, `TMP`, `TEMP`, else `/tmp`.
//! Production uses `BASE`; tests use their own base and never `discord-ipc-N`.

use std::io;

/// What games look for.
pub const BASE: &str = "discord-ipc";
/// Games try N = 0..9.
const SLOTS: u8 = 10;

#[cfg(windows)]
pub use windows::{Listener, Stream};

#[cfg(unix)]
pub use unix::{Listener, Stream};

#[cfg(windows)]
mod windows {
    use super::*;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    pub type Stream = NamedPipeServer;

    pub struct Listener {
        name: String,
        index: u8,
        /// The instance the next game connects to.
        next: NamedPipeServer,
    }

    impl Listener {
        /// The lowest free index, None when all ten are taken. Any create error counts as taken:
        /// the usual one is access denied for an existing name.
        pub async fn bind(base: &str) -> Option<Listener> {
            (0..SLOTS).find_map(|index| {
                let name = format!(r"\\.\pipe\{base}-{index}");
                let next = ServerOptions::new()
                    .first_pipe_instance(true)
                    .create(&name)
                    .ok()?;
                Some(Listener { name, index, next })
            })
        }

        pub fn index(&self) -> u8 {
            self.index
        }

        /// The next game; a fresh instance is ready before the caller gets this one, so a second
        /// game never meets a busy pipe.
        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.next.connect().await?;
            Ok(std::mem::replace(
                &mut self.next,
                ServerOptions::new().create(&self.name)?,
            ))
        }
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    pub type Stream = UnixStream;

    pub struct Listener {
        path: PathBuf,
        index: u8,
        inner: UnixListener,
    }

    fn dir() -> PathBuf {
        ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
            .iter()
            .find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()))
            .map_or_else(|| "/tmp".into(), PathBuf::from)
    }

    impl Listener {
        /// The lowest free index, None when all ten are taken. A socket that accepts a
        /// connection is in use; one that refuses is left over from a crash and replaced.
        pub async fn bind(base: &str) -> Option<Listener> {
            for index in 0..SLOTS {
                let path = dir().join(format!("{base}-{index}"));
                match UnixStream::connect(&path).await {
                    Ok(_) => continue,
                    Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                        let _ = std::fs::remove_file(&path);
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => continue,
                }
                let Ok(inner) = UnixListener::bind(&path) else {
                    continue;
                };
                // shortcut: the socket exists with the umask's mode until this call; the per-user runtime dir hides it, /tmp does not, upgrade with umask(0o177) around bind.
                if std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).is_err()
                {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
                return Some(Listener { path, index, inner });
            }
            None
        }

        pub fn index(&self) -> u8 {
            self.index
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.inner.accept().await.map(|(s, _)| s)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Where the socket `<base>-<index>` is (tests connect to it).
    #[cfg(test)]
    pub fn path(base: &str, index: u8) -> PathBuf {
        dir().join(format!("{base}-{index}"))
    }
}

#[cfg(test)]
pub use tests::connect;

#[cfg(test)]
mod tests {
    use super::*;

    /// A game's side: connect to `<base>-<index>`.
    #[cfg(windows)]
    pub async fn connect(
        base: &str,
        index: u8,
    ) -> io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
        tokio::net::windows::named_pipe::ClientOptions::new()
            .open(format!(r"\\.\pipe\{base}-{index}"))
    }

    #[cfg(unix)]
    pub async fn connect(base: &str, index: u8) -> io::Result<tokio::net::UnixStream> {
        tokio::net::UnixStream::connect(super::unix::path(base, index)).await
    }

    #[tokio::test]
    async fn binds_the_lowest_free_index_and_never_a_taken_one() {
        let base = format!("rustcord-test-ipc-{}-bind", std::process::id());
        assert!(!base.starts_with("discord-ipc"));
        let mut held = Vec::new();
        for want in 0..SLOTS {
            let l = Listener::bind(&base).await.unwrap();
            assert_eq!(l.index(), want);
            held.push(l);
        }
        assert!(Listener::bind(&base).await.is_none(), "all ten taken");
        held.clear();
        assert_eq!(
            Listener::bind(&base).await.map(|l| l.index()),
            Some(0),
            "freed"
        );
    }
}
