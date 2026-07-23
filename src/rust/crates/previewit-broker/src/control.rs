use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use previewit_protocol::v0::{BrokerControlRequest, BrokerControlResponse, broker_control_request};
use previewit_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR};
use thiserror::Error;

const MAX_COMMAND_ID_BYTES: usize = 128;
const MAX_PATH_UTF16_UNITS: usize = 32_767;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CommandId(String);

impl CommandId {
    pub fn parse(value: impl Into<String>) -> Result<Self, CommandRejectionCode> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= MAX_COMMAND_ID_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
        if valid {
            Ok(Self(value))
        } else {
            Err(CommandRejectionCode::InvalidCommandId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BrokerCommand {
    Open(PathBuf),
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedCommand {
    command_id: CommandId,
    command: BrokerCommand,
}

impl ValidatedCommand {
    pub fn command_id(&self) -> &CommandId {
        &self.command_id
    }

    pub fn command(&self) -> &BrokerCommand {
        &self.command
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandRejectionCode {
    ProtocolMajorMismatch,
    ProtocolMinorMismatch,
    InvalidCommandId,
    InvalidCommand,
    InvalidUtf16,
    EmbeddedNul,
    PathTooLong,
    PathNotAbsolute,
    PathNotFound,
    InvalidProtobuf,
    FrameTooLarge,
    PrimaryBusy,
    QueueFull,
}

impl CommandRejectionCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolMajorMismatch => "protocol-major-mismatch",
            Self::ProtocolMinorMismatch => "protocol-minor-mismatch",
            Self::InvalidCommandId => "invalid-command-id",
            Self::InvalidCommand => "invalid-command",
            Self::InvalidUtf16 => "invalid-utf16",
            Self::EmbeddedNul => "embedded-nul",
            Self::PathTooLong => "path-too-long",
            Self::PathNotAbsolute => "path-not-absolute",
            Self::PathNotFound => "path-not-found",
            Self::InvalidProtobuf => "invalid-protobuf",
            Self::FrameTooLarge => "frame-too-large",
            Self::PrimaryBusy => "primary-busy",
            Self::QueueFull => "queue-full",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "protocol-major-mismatch" => Self::ProtocolMajorMismatch,
            "protocol-minor-mismatch" => Self::ProtocolMinorMismatch,
            "invalid-command-id" => Self::InvalidCommandId,
            "invalid-command" => Self::InvalidCommand,
            "invalid-utf16" => Self::InvalidUtf16,
            "embedded-nul" => Self::EmbeddedNul,
            "path-too-long" => Self::PathTooLong,
            "path-not-absolute" => Self::PathNotAbsolute,
            "path-not-found" => Self::PathNotFound,
            "invalid-protobuf" => Self::InvalidProtobuf,
            "frame-too-large" => Self::FrameTooLarge,
            "primary-busy" => Self::PrimaryBusy,
            "queue-full" => Self::QueueFull,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRejection {
    command_id: Option<CommandId>,
    code: CommandRejectionCode,
}

impl CommandRejection {
    pub fn code(&self) -> CommandRejectionCode {
        self.code
    }

    pub fn safe_command_id(&self) -> Option<&str> {
        self.command_id.as_ref().map(CommandId::as_str)
    }

    pub fn into_ack(self) -> CommandAck {
        CommandAck::Rejected {
            command_id: self.command_id,
            reason: self.code,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandAck {
    OpenAccepted {
        command_id: CommandId,
        request_id: String,
    },
    CloseAccepted {
        command_id: CommandId,
    },
    Rejected {
        command_id: Option<CommandId>,
        reason: CommandRejectionCode,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedCommandKind {
    Open,
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedAck {
    command_id: CommandId,
    kind: ExpectedCommandKind,
}

impl ExpectedAck {
    pub fn open(command_id: CommandId) -> Self {
        Self {
            command_id,
            kind: ExpectedCommandKind::Open,
        }
    }

    pub fn close(command_id: CommandId) -> Self {
        Self {
            command_id,
            kind: ExpectedCommandKind::Close,
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum InvalidCommandResponse {
    #[error("Broker response protocol major does not match")]
    ProtocolMajorMismatch,
    #[error("Broker response protocol minor does not match")]
    ProtocolMinorMismatch,
    #[error("Broker response command id is invalid")]
    InvalidCommandId,
    #[error("Broker response command id does not match the request")]
    CommandMismatch,
    #[error("Broker response fields do not form a valid acknowledgement")]
    InvalidShape,
    #[error("Broker response contains an unknown rejection code")]
    UnknownRejection,
}

impl InvalidCommandResponse {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ProtocolMajorMismatch => "response-protocol-major-mismatch",
            Self::ProtocolMinorMismatch => "response-protocol-minor-mismatch",
            Self::InvalidCommandId => "response-command-id-invalid",
            Self::CommandMismatch => "response-command-mismatch",
            Self::InvalidShape | Self::UnknownRejection => "invalid-response-shape",
        }
    }
}

pub struct BrokerControlContract;

impl BrokerControlContract {
    pub fn decode_request(
        request: BrokerControlRequest,
    ) -> Result<ValidatedCommand, CommandRejection> {
        let command_id = CommandId::parse(request.command_id).map_err(|code| CommandRejection {
            command_id: None,
            code,
        })?;

        if request.protocol_major != PROTOCOL_MAJOR {
            return Err(rejection(
                command_id,
                CommandRejectionCode::ProtocolMajorMismatch,
            ));
        }
        if request.protocol_minor != PROTOCOL_MINOR {
            return Err(rejection(
                command_id,
                CommandRejectionCode::ProtocolMinorMismatch,
            ));
        }

        let command = match request.command {
            Some(broker_control_request::Command::OpenPath(open)) => BrokerCommand::Open(
                decode_path(&open.path_utf16le)
                    .map_err(|code| rejection(command_id.clone(), code))?,
            ),
            Some(broker_control_request::Command::ClosePreview(_)) => BrokerCommand::Close,
            None => {
                return Err(rejection(command_id, CommandRejectionCode::InvalidCommand));
            }
        };

        Ok(ValidatedCommand {
            command_id,
            command,
        })
    }

    pub fn encode_response(ack: &CommandAck) -> BrokerControlResponse {
        let (command_id, accepted, request_id, error_code) = match ack {
            CommandAck::OpenAccepted {
                command_id,
                request_id,
            } => (command_id.as_str(), true, request_id.as_str(), ""),
            CommandAck::CloseAccepted { command_id } => (command_id.as_str(), true, "", ""),
            CommandAck::Rejected { command_id, reason } => (
                command_id.as_ref().map(CommandId::as_str).unwrap_or(""),
                false,
                "",
                reason.as_str(),
            ),
        };

        BrokerControlResponse {
            protocol_major: PROTOCOL_MAJOR,
            protocol_minor: PROTOCOL_MINOR,
            command_id: command_id.to_owned(),
            accepted,
            request_id: request_id.to_owned(),
            error_code: error_code.to_owned(),
        }
    }

    pub fn decode_response(
        expected: &ExpectedAck,
        response: BrokerControlResponse,
    ) -> Result<CommandAck, InvalidCommandResponse> {
        if response.protocol_major != PROTOCOL_MAJOR {
            return Err(InvalidCommandResponse::ProtocolMajorMismatch);
        }
        if response.protocol_minor != PROTOCOL_MINOR {
            return Err(InvalidCommandResponse::ProtocolMinorMismatch);
        }

        if response.command_id.is_empty() {
            if !response.accepted
                && response.request_id.is_empty()
                && response.error_code == CommandRejectionCode::PrimaryBusy.as_str()
            {
                return Ok(CommandAck::Rejected {
                    command_id: None,
                    reason: CommandRejectionCode::PrimaryBusy,
                });
            }
            return Err(InvalidCommandResponse::InvalidCommandId);
        }

        let command_id = CommandId::parse(response.command_id)
            .map_err(|_| InvalidCommandResponse::InvalidCommandId)?;
        if command_id != expected.command_id {
            return Err(InvalidCommandResponse::CommandMismatch);
        }

        if response.accepted {
            if !response.error_code.is_empty() {
                return Err(InvalidCommandResponse::InvalidShape);
            }
            return match expected.kind {
                ExpectedCommandKind::Open if !response.request_id.is_empty() => {
                    Ok(CommandAck::OpenAccepted {
                        command_id,
                        request_id: response.request_id,
                    })
                }
                ExpectedCommandKind::Close if response.request_id.is_empty() => {
                    Ok(CommandAck::CloseAccepted { command_id })
                }
                ExpectedCommandKind::Open | ExpectedCommandKind::Close => {
                    Err(InvalidCommandResponse::InvalidShape)
                }
            };
        }

        if !response.request_id.is_empty() || response.error_code.is_empty() {
            return Err(InvalidCommandResponse::InvalidShape);
        }
        let reason = CommandRejectionCode::parse(&response.error_code)
            .ok_or(InvalidCommandResponse::UnknownRejection)?;
        Ok(CommandAck::Rejected {
            command_id: Some(command_id),
            reason,
        })
    }
}

fn rejection(command_id: CommandId, code: CommandRejectionCode) -> CommandRejection {
    CommandRejection {
        command_id: Some(command_id),
        code,
    }
}

fn decode_path(bytes: &[u8]) -> Result<PathBuf, CommandRejectionCode> {
    if !bytes.len().is_multiple_of(2) {
        return Err(CommandRejectionCode::InvalidUtf16);
    }

    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    if units.len() > MAX_PATH_UTF16_UNITS {
        return Err(CommandRejectionCode::PathTooLong);
    }
    if units.contains(&0) {
        return Err(CommandRejectionCode::EmbeddedNul);
    }
    if char::decode_utf16(units.iter().copied()).any(|unit| unit.is_err()) {
        return Err(CommandRejectionCode::InvalidUtf16);
    }

    let path = PathBuf::from(OsString::from_wide(&units));
    if !path.is_absolute() {
        return Err(CommandRejectionCode::PathNotAbsolute);
    }
    Ok(path)
}
