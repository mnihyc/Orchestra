//! Local RFB sign-in answers server passwords without exposing them to a viewer.

use super::DisplayError;
use des::cipher::{BlockEncrypt, KeyInit};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// Adoption has fifteen seconds on the wire; ten leaves room to return its result.
pub const HANDSHAKE_WAIT: Duration = Duration::from_secs(10);
/// Server names are metadata, bounded by the same budget as browser control messages.
const MAX_NAME: usize = 256 * 1024;
/// VNC password authentication encrypts two eight-byte DES blocks.
const CHALLENGE_BYTES: usize = 16;

/// The local connection's asynchronous byte transport.
pub trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// A server connection after its security handshake, before ClientInit.
pub struct Authenticated {
    /// The connection; only protocol bytes travel past this boundary.
    pub stream: Box<dyn Stream>,
    /// The supported server version, to be spoken independently to the viewer.
    pub version: [u8; 12],
}

/// A verified local target; DNS and nonloopback IP addresses are never used.
#[derive(Clone)]
pub enum Endpoint {
    /// An IPv4 loopback port.
    Port(u16),
    /// A local Unix socket path.
    Socket(PathBuf),
}

impl Endpoint {
    /// Accepts only the documented loopback authorities or an absolute socket path.
    pub fn parse(value: &str, root: &Path) -> Result<Self, DisplayError> {
        if value.starts_with('/') {
            let path = PathBuf::from(value);
            if path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(DisplayError::NotLocal);
            }
            return Ok(Self::Socket(path));
        }
        if let Some((host, port)) = value.split_once(':') {
            if matches!(host, "127.0.0.1" | "localhost")
                && let Ok(port) = port.parse::<u16>()
                && port != 0
            {
                return Ok(Self::Port(port));
            }
            return Err(DisplayError::NotLocal);
        }
        // A relative socket is anchored to this environment's workspace, never the connector's cwd.
        let path = Path::new(value);
        if value.is_empty()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(DisplayError::NotLocal);
        }
        Ok(Self::Socket(root.join(path)))
    }

    /// Opens only the validated local transport.
    pub async fn connect(&self) -> Result<Box<dyn Stream>, DisplayError> {
        match self {
            Self::Port(port) => Ok(Box::new(TcpStream::connect(("127.0.0.1", *port)).await?)),
            #[cfg(unix)]
            Self::Socket(path) => Ok(Box::new(tokio::net::UnixStream::connect(path).await?)),
            #[cfg(not(unix))]
            Self::Socket(_) => Err(DisplayError::Unsupported),
        }
    }
}

/// Connects and answers None or classic VNC authentication; arbitrary server errors are never echoed.
pub async fn authenticate(
    endpoint: &Endpoint,
    password: Option<&[u8]>,
) -> Result<Authenticated, DisplayError> {
    tokio::time::timeout(HANDSHAKE_WAIT, async {
        let mut stream = endpoint.connect().await?;
        let mut version = [0; 12];
        stream.read_exact(&mut version).await?;
        let minor = match &version {
            b"RFB 003.003\n" => 3,
            b"RFB 003.007\n" => 7,
            b"RFB 003.008\n" => 8,
            _ => return Err(DisplayError::NotADisplay),
        };
        stream.write_all(&version).await?;
        let security = if minor == 3 {
            let security = stream.read_u32().await?;
            if !matches!(security, 1 | 2) {
                return Err(DisplayError::NotADisplay);
            }
            security as u8
        } else {
            let count = stream.read_u8().await?;
            if count == 0 {
                return Err(DisplayError::NotADisplay);
            }
            let mut offered = vec![0; usize::from(count)];
            stream.read_exact(&mut offered).await?;
            let selected = if password.is_some() && offered.contains(&2) {
                2
            } else if offered.contains(&1) {
                1
            } else if offered.contains(&2) {
                return Err(DisplayError::BadPassword);
            } else {
                return Err(DisplayError::NotADisplay);
            };
            stream.write_u8(selected).await?;
            selected
        };
        if security == 2 {
            let password = password.ok_or(DisplayError::BadPassword)?;
            let mut challenge = [0; CHALLENGE_BYTES];
            stream.read_exact(&mut challenge).await?;
            stream.write_all(&response(password, challenge)).await?;
        }
        if (security == 2 || minor == 8) && stream.read_u32().await? != 0 {
            return Err(DisplayError::BadPassword);
        }
        Ok(Authenticated { stream, version })
    })
    .await
    .map_err(|_| DisplayError::Unreachable)?
}

fn response(password: &[u8], mut challenge: [u8; CHALLENGE_BYTES]) -> [u8; CHALLENGE_BYTES] {
    let mut key = [0; 8];
    for (slot, byte) in key.iter_mut().zip(password.iter()) {
        *slot = byte.reverse_bits();
    }
    let cipher = des::Des::new((&key).into());
    for block in challenge.chunks_exact_mut(8) {
        cipher.encrypt_block(block.into());
    }
    key.fill(0);
    challenge
}

/// Verifies a server's initial framebuffer and extracts bounded metadata after joining shared.
pub async fn describe(
    mut connection: Authenticated,
) -> Result<(Option<String>, String), DisplayError> {
    tokio::time::timeout(HANDSHAKE_WAIT, async {
        connection.stream.write_u8(1).await?;
        let width = connection.stream.read_u16().await?;
        let height = connection.stream.read_u16().await?;
        if width == 0 || height == 0 {
            return Err(DisplayError::NotADisplay);
        }
        let mut format = [0; 16];
        connection.stream.read_exact(&mut format).await?;
        if !matches!(format[0], 8 | 16 | 32) {
            return Err(DisplayError::NotADisplay);
        }
        let length = usize::try_from(connection.stream.read_u32().await?)
            .map_err(|_| DisplayError::NotADisplay)?;
        if length > MAX_NAME {
            return Err(DisplayError::NotADisplay);
        }
        let mut name = vec![0; length];
        connection.stream.read_exact(&mut name).await?;
        let name = String::from_utf8(name).ok().filter(|s| !s.is_empty());
        Ok((name, format!("{width}x{height}")))
    })
    .await
    .map_err(|_| DisplayError::Unreachable)?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_named_server_is_local_and_never_resolves_an_arbitrary_host() {
        for endpoint in [
            "example.test:5900",
            "127.0.0.2:5900",
            "http://localhost:5900",
            "localhost:0",
            "../socket",
        ] {
            assert!(Endpoint::parse(endpoint, Path::new("/workspace")).is_err());
        }
        assert!(matches!(
            Endpoint::parse("localhost:5900", Path::new("/workspace")).unwrap(),
            Endpoint::Port(5900)
        ));
    }
    #[tokio::test]
    async fn a_wrong_password_returns_a_safe_refusal_and_never_the_server_message() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.write_all(b"RFB 003.008\n").await.unwrap();
            let mut version = [0; 12];
            socket.read_exact(&mut version).await.unwrap();
            socket.write_all(&[1, 2]).await.unwrap();
            assert_eq!(socket.read_u8().await.unwrap(), 2);
            socket.write_all(&[42; 16]).await.unwrap();
            let mut answer = [0; 16];
            socket.read_exact(&mut answer).await.unwrap();
            assert_ne!(answer, response(b"right-password", [42; 16]));
            socket.write_u32(1).await.unwrap();
        });
        let error = authenticate(&Endpoint::Port(port), Some(b"wrong-password"))
            .await
            .err()
            .unwrap();
        assert_eq!(error.code(), "bad_password");
        assert!(!error.to_string().contains("password-private"));
        server.await.unwrap();
    }
}
