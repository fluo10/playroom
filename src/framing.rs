use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

use crate::error::NetworkError;

/// 長さプレフィックス付き postcard フレームを書き込む
pub struct FramedSender<T> {
    send: iroh::endpoint::SendStream,
    _marker: PhantomData<T>,
}

/// 長さプレフィックス付き postcard フレームを読み込む
pub struct FramedReceiver<T> {
    recv: iroh::endpoint::RecvStream,
    _marker: PhantomData<T>,
}

impl<T: Serialize> FramedSender<T> {
    pub fn new(send: iroh::endpoint::SendStream) -> Self {
        Self {
            send,
            _marker: PhantomData,
        }
    }

    pub async fn send(&mut self, msg: &T) -> Result<(), NetworkError> {
        let bytes = postcard::to_allocvec(msg)
            .map_err(|e| NetworkError::Framing(e.to_string()))?;
        let len = bytes.len() as u32;
        self.send.write_all(&len.to_le_bytes()).await?;
        self.send.write_all(&bytes).await?;
        Ok(())
    }
}

impl<T: for<'de> Deserialize<'de>> FramedReceiver<T> {
    pub fn new(recv: iroh::endpoint::RecvStream) -> Self {
        Self {
            recv,
            _marker: PhantomData,
        }
    }

    pub async fn recv(&mut self) -> Result<T, NetworkError> {
        let mut len_buf = [0u8; 4];
        self.recv.read_exact(&mut len_buf).await
            .map_err(|_| NetworkError::Closed)?;
        let len = u32::from_le_bytes(len_buf) as usize;

        let mut buf = vec![0u8; len];
        self.recv.read_exact(&mut buf).await
            .map_err(|_| NetworkError::Closed)?;

        postcard::from_bytes(&buf)
            .map_err(|e| NetworkError::Framing(e.to_string()))
    }
}
