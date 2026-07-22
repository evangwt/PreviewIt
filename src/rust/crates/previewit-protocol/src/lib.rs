use thiserror::Error;

pub mod v0 {
    include!(concat!(env!("OUT_DIR"), "/previewit.preview.v0.rs"));
}

pub const MAX_CONTROL_FRAME: usize = 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("control frame payload is too large: {0} bytes")]
    TooLarge(usize),
    #[error("control frame is missing its four-byte length prefix")]
    MissingLength,
    #[error("control frame length mismatch: declared {declared} bytes, received {actual}")]
    LengthMismatch { declared: usize, actual: usize },
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_CONTROL_FRAME {
        return Err(FrameError::TooLarge(payload.len()));
    }

    let len = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge(payload.len()))?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8]) -> Result<&[u8], FrameError> {
    let length_bytes: [u8; 4] = frame
        .get(..4)
        .ok_or(FrameError::MissingLength)?
        .try_into()
        .expect("length slice contains exactly four bytes");
    let declared = u32::from_le_bytes(length_bytes) as usize;

    if declared > MAX_CONTROL_FRAME {
        return Err(FrameError::TooLarge(declared));
    }

    let payload = &frame[4..];
    if payload.len() != declared {
        return Err(FrameError::LengthMismatch {
            declared,
            actual: payload.len(),
        });
    }

    Ok(payload)
}
