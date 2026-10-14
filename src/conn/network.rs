use std::net::SocketAddr;

use bytes::{Bytes, BytesMut};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::error::RusbmuxError;

pub struct NetworkDeviceConn {
    pub stream: TcpStream,

    device_id: u64,

    destination_port: u16,

    device_canceler: CancellationToken,
}

impl NetworkDeviceConn {
    pub async fn new(
        socket: SocketAddr,
        device_id: u64,
        device_canceler: CancellationToken,
    ) -> Result<Self, RusbmuxError> {
        let stream = TcpStream::connect(socket).await?;

        Ok(Self {
            stream,
            device_id,
            destination_port: socket.port(),
            device_canceler,
        })
    }

    pub async fn write(&mut self, value: Bytes) -> Result<(), RusbmuxError> {
        debug!(?value);

        Ok(self.stream.write_all(&value).await?)
    }

    pub async fn read(&mut self) -> Result<Bytes, RusbmuxError> {
        let mut buf = BytesMut::new();

        let n = self.stream.read_buf(&mut buf).await?;

        if n == 0 {
            return Err(RusbmuxError::IO(std::io::Error::new(
                std::io::ErrorKind::NetworkUnreachable,
                "Stream returned 0",
            )));
        }

        let packet = buf.split().freeze();

        debug!(?packet);

        Ok(packet)
    }

    pub async fn wait_shutdown(&self) {
        self.device_canceler.cancelled().await;
    }

    pub fn device_id(&self) -> u64 {
        self.device_id
    }

    pub fn destination_port(&self) -> u16 {
        self.destination_port
    }

    pub fn device_canceler(&self) -> CancellationToken {
        self.device_canceler.clone()
    }
}
