//! A local X desktop is shared through a private loopback connection for each admitted viewer.

use super::{DisplayError, rfb};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Desktop {
    display: String,
    program: PathBuf,
    socket: PathBuf,
    original_socket: std::sync::Arc<std::fs::File>,
    launcher: Vec<String>,
}

#[cfg(target_os = "linux")]
pub(super) fn socket_name(display: &str) -> Option<String> {
    let mut parts = display.strip_prefix(':')?.split('.');
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let number = parts.next().filter(|part| digits(part))?;
    if parts.next().is_some_and(|screen| !digits(screen)) || parts.next().is_some() {
        return None;
    }
    Some(format!("X{number}"))
}

impl Desktop {
    #[cfg(target_os = "linux")]
    pub fn discover(launcher: Vec<String>) -> Option<Self> {
        use rustix::fs::{Mode, OFlags, open};
        use std::os::unix::fs::FileTypeExt;
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind != "x11") {
            return None;
        }
        let display = std::env::var("DISPLAY").ok()?;
        let socket = PathBuf::from("/tmp/.X11-unix").join(socket_name(&display)?);
        let socket_handle = std::fs::File::from(
            open(
                &socket,
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .ok()?,
        );
        let owner = socket_handle.metadata().ok()?;
        if !owner.file_type().is_socket() {
            return None;
        }
        Some(Self {
            program: crate::browser_discovery::find_program("x11vnc")?
                .canonicalize()
                .ok()?,
            display,
            socket,
            original_socket: std::sync::Arc::new(socket_handle),
            launcher,
        })
    }

    pub fn name(&self) -> String {
        format!("X11 {}", self.display)
    }

    #[cfg(target_os = "linux")]
    pub async fn connection(&self) -> Result<Box<dyn rfb::Stream>, DisplayError> {
        use std::os::{fd::OwnedFd, unix::fs::MetadataExt};
        use std::process::Stdio;
        use tokio::net::{TcpListener, TcpStream};

        let socket = std::fs::symlink_metadata(&self.socket)?;
        let original = self.original_socket.metadata()?;
        if (socket.dev(), socket.ino()) != (original.dev(), original.ino()) {
            return Err(DisplayError::NotFound);
        }
        // Inetd mode receives one already connected socket. Only our matching peer gets handed to X11.
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let client = TcpStream::connect(listener.local_addr()?).await?;
        let ours = client.local_addr()?;
        let server = loop {
            let (stream, peer) = listener.accept().await?;
            if peer == ours {
                break stream.into_std()?;
            }
        };
        drop(listener);
        server.set_nonblocking(false)?;
        let mut command = crate::browser_discovery::browser_command_in(
            &self.program,
            &Default::default(),
            &self.launcher,
        );
        if let Some(authority) = std::env::var_os("XAUTHORITY") {
            command.env("XAUTHORITY", authority);
        }
        let child = command
            .args([
                "-norc",
                "-inetd",
                "-q",
                "-once",
                "-rfbport",
                "0",
                "-no6",
                "-safer",
                "-nocmds",
                "-nopw",
                "-display",
                &self.display,
            ])
            .stdin(Stdio::from(OwnedFd::from(server.try_clone()?)))
            .stdout(Stdio::from(OwnedFd::from(server)))
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Box::new(DesktopConnection {
            socket: client,
            _server: child,
        }))
    }
}

/// The sharing process belongs to this single connection and is killed when its transport closes.
struct DesktopConnection {
    socket: tokio::net::TcpStream,
    _server: tokio::process::Child,
}

impl tokio::io::AsyncRead for DesktopConnection {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.socket).poll_read(cx, buf)
    }
}
impl tokio::io::AsyncWrite for DesktopConnection {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.socket).poll_write(cx, bytes)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.socket).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.socket).poll_shutdown(cx)
    }
}
