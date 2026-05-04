// src/client.rs
// Shared socket client used by CLI commands and the TUI.
// Cross-platform: Unix domain sockets on unix, named pipes on Windows.

use anyhow::{Context, Result};
#[cfg(windows)]
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::ipc::{ClientMsg, DaemonMsg};

pub struct ShepherdClient {
    reader: BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
    writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
}

#[cfg(windows)]
const CONNECT_RETRY_TIMEOUT: Duration = Duration::from_millis(1500);

#[cfg(windows)]
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(50);

impl ShepherdClient {
    /// Connect to the running daemon. Returns a descriptive error if not found.
    pub async fn connect() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::net::UnixStream;
            let path = crate::ipc::socket_path();
            let stream = UnixStream::connect(&path).await.with_context(|| {
                format!(
                    "Cannot connect to shepherd daemon at {}\n\
                         Is it running? Start with: shepherd daemon",
                    path.display()
                )
            })?;
            let (reader, writer) = stream.into_split();
            Ok(Self {
                reader: BufReader::new(Box::new(reader)),
                writer: Box::new(writer),
            })
        }
        #[cfg(windows)]
        {
            use std::io;
            use tokio::net::windows::named_pipe::ClientOptions;

            let pipe_name = crate::ipc::pipe_name();
            let started = Instant::now();

            // Retry briefly: the server might still be creating the next pipe
            // instance, but a missing daemon must not make CLI commands hang.
            let client = loop {
                match ClientOptions::new().open(&pipe_name) {
                    Ok(c) => break c,
                    Err(e) if e.kind() == io::ErrorKind::NotFound
                        || e.raw_os_error() == Some(231) /* ERROR_PIPE_BUSY */ =>
                    {
                        if started.elapsed() >= CONNECT_RETRY_TIMEOUT {
                            return Err(anyhow::anyhow!(
                                "Cannot connect to shepherd daemon at {}\n\
                                 Is it running? Start with: shepherd daemon\n\
                                 Error: {}",
                                pipe_name,
                                e
                            ));
                        }
                        tokio::time::sleep(CONNECT_RETRY_DELAY).await;
                        continue;
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!(
                            "Cannot connect to shepherd daemon at {}\n\
                             Is it running? Start with: shepherd daemon\n\
                             Error: {}",
                            pipe_name, e
                        ));
                    }
                }
            };

            let (reader, writer) = tokio::io::split(client);
            Ok(Self {
                reader: BufReader::new(Box::new(reader)),
                writer: Box::new(writer),
            })
        }
    }

    /// Send one message and receive one response.
    pub async fn send_recv(&mut self, msg: &ClientMsg) -> Result<DaemonMsg> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;

        let mut resp_line = String::new();
        self.reader.read_line(&mut resp_line).await?;

        serde_json::from_str(resp_line.trim()).context("Failed to parse daemon response as JSON")
    }
}
