// SPDX-License-Identifier: GPL-3.0-or-later

//! The broker wire protocol (ADR-0011).
//!
//! A private Unix socket, not D-Bus, because this carries Binder-like traffic
//! on the hottest path. Frames are a four byte big-endian length followed by a
//! JSON body. JSON is enough for the control plane; the data plane (Binder
//! transactions) gets its own compact codec in Phase 3. Descriptor passing
//! arrives with it, via `SCM_RIGHTS`.

use crate::broker::registry::Package;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Refuse absurd frames rather than allocating on a hostile length prefix.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Reserve a UID and data directory for an APK. The broker owns the
    /// allocation; nothing privileged happens here.
    Install {
        apk: String,
    },
    /// Make a reserved package permanent, after the caller has done the
    /// privileged setup. Idempotent: committing twice is the same as once.
    Commit {
        package: Package,
    },
    Uninstall {
        package: String,
    },
    Launch {
        package: String,
        args: Vec<String>,
    },
    Query {
        package: Option<String>,
    },
    Ping,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Pong,
    /// A reserved package: the caller must now create its user and data
    /// directory, then send `Commit`.
    Planned(Package),
    Installed(Package),
    /// The removed package, so the caller can delete its data directory.
    Uninstalled(Package),
    Apps(Vec<Package>),
    Launched {
        package: String,
    },
    Error {
        message: String,
    },
}

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    let body = serde_json::to_vec(value)?;
    anyhow::ensure!(
        body.len() <= MAX_FRAME,
        "frame too large: {} bytes",
        body.len()
    );
    writer.write_all(&(body.len() as u32).to_be_bytes()).await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R, T>(reader: &mut R) -> anyhow::Result<T>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len = [0u8; 4];
    reader.read_exact(&mut len).await?;
    read_frame_after(len, reader).await
}

/// The same frame, when the four byte length prefix has already been read.
///
/// The broker reads those four bytes first to tell a control connection from a
/// Binder one, so by the time it knows this is a control frame the length is
/// already in hand.
pub async fn read_frame_after<R, T>(len: [u8; 4], reader: &mut R) -> anyhow::Result<T>
where
    R: AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let len = u32::from_be_bytes(len) as usize;
    anyhow::ensure!(len <= MAX_FRAME, "frame too large: {} bytes", len);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_roundtrips_over_a_pipe() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let request = Request::Install {
            apk: "/tmp/app.apk".to_string(),
        };
        write_frame(&mut a, &request).await.unwrap();
        let got: Request = read_frame(&mut b).await.unwrap();
        assert_eq!(got, request);
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_before_allocating() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let bogus = ((MAX_FRAME + 1) as u32).to_be_bytes();
        a.write_all(&bogus).await.unwrap();
        let got: Result<Request, _> = read_frame(&mut b).await;
        assert!(got.is_err());
    }
}
