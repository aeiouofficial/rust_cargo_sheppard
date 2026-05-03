// src/client.rs
// Shared socket client used by CLI commands and the TUI.
// Cross-platform: Unix domain sockets on unix, named pipes on Windows.

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::ipc::{ClientMsg, DaemonMsg};

pub struct ShepherdClient {
    reader: BufReader<Box<dyn tokio::io::AsyncRead + Unpin + Send>>,
    writer: Box<dyn tokio::io::AsyncWrite + Unpin + Send>,
}

impl ShepherdClient {
    /// Connect to the running daemon. Returns a descriptive error if not found.
    pub async fn connect() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::net::UnixStream;
            let path = crate::ipc::socket_path();
            let stream = UnixStream::connect(&path)
                .await
                .with_context(|| {
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
            use tokio::net::windows::named_pipe::ClientOptions;
            use std::io;

            let pipe_name = crate::ipc::pipe_name();

            // Retry loop: the server might be busy or not yet ready
            let client = loop {
                match ClientOptions::new().open(&pipe_name) {
                    Ok(c) => break c,
                    Err(e) if e.kind() == io::ErrorKind::NotFound
                        || e.raw_os_error() == Some(231) /* ERROR_PIPE_BUSY */ =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
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

        serde_json::from_str(resp_line.trim())
            .context("Failed to parse daemon response as JSON")
    }

    /// Convenience: fire-and-forget (ignores response).
    pub async fn send(&mut self, msg: &ClientMsg) -> Result<()> {
        let mut line = serde_json::to_string(msg)?;
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }
}
