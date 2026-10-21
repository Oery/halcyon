//! Packet writes share one deadline, flush and accounting path.

use std::time::Instant;

use futures_lite::AsyncWriteExt;
use glommio::net::TcpStream;
use mc_protocol::packets::Payload;

use super::{Client, WRITE_TIMEOUT, with_deadline};
use crate::error::Error;
use crate::transport::ClientStream;

impl Client {
    pub async fn send<'p, P: Payload<'p>>(&mut self, packet: &P) -> Result<(), Error> {
        self.packet_buffer.clear();
        packet.write_packet(&mut self.packet_buffer).await?;
        self.send_encoded_buffer().await
    }

    pub async fn send_encoded_buffer(&mut self) -> Result<(), Error> {
        let bytes = write_parts(&mut self.stream, [&self.packet_buffer]).await?;
        self.lobby.record_sent(bytes);
        Ok(())
    }

    pub async fn send_encoded(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let bytes = write_parts(&mut self.stream, [bytes]).await?;
        self.lobby.record_sent(bytes);
        Ok(())
    }

    pub async fn send_chunk_parts(
        &mut self,
        header: &[u8],
        data: &[u8],
        suffix: &[u8],
    ) -> Result<(), Error> {
        let bytes = write_parts(&mut self.stream, [header, data, suffix]).await?;
        self.lobby.record_sent(bytes);
        Ok(())
    }

    pub async fn send_buffered_chunk(&mut self, data: &[u8], suffix: &[u8]) -> Result<(), Error> {
        let bytes = write_parts(&mut self.stream, [&self.packet_buffer, data, suffix]).await?;
        self.lobby.record_sent(bytes);
        Ok(())
    }
}

async fn write_parts<const N: usize>(
    stream: &mut ClientStream<TcpStream>,
    parts: [&[u8]; N],
) -> Result<usize, Error> {
    with_deadline(Instant::now() + WRITE_TIMEOUT, async {
        let mut bytes = 0;
        for part in parts {
            stream.write_all(part).await?;
            bytes += part.len();
        }
        stream.flush().await?;
        Ok(bytes)
    })
    .await
}
