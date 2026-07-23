use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use previewit_broker::{
    BrokerCommandClient, BrokerCommandError, BrokerCommandServer, BrokerControlContract,
    BrokerEvent, BrokerRuntime, CommandAck, EventSink, InstanceLease, InstanceRole,
    InstanceRoleName, StderrEventSink,
};
use previewit_protocol::v0::{
    BrokerControlRequest, BrokerControlResponse, ClosePreview, OpenPath, broker_control_request,
};
use previewit_protocol::{PROTOCOL_MAJOR, PROTOCOL_MINOR};
use uuid::Uuid;

const PRODUCT_ID: &str = "PreviewIt.Broker";
const PRODUCT_SUFFIX_ENV: &str = "PREVIEWIT_TEST_PRODUCT_SUFFIX";
const STARTUP_TIMEOUT: Duration = Duration::from_millis(500);
const IO_TIMEOUT: Duration = Duration::from_secs(2);

enum StartupCommand {
    Open(OsString),
    Close,
}

fn main() -> ExitCode {
    let command = match parse_arguments(std::env::args_os().skip(1).collect()) {
        Ok(command) => command.map(control_request),
        Err(()) => {
            eprintln!("usage: previewit-broker [--open <path> | --close]");
            return ExitCode::from(2);
        }
    };

    let events: Arc<dyn EventSink> = Arc::new(StderrEventSink);
    match InstanceLease::elect(&product_id()) {
        Ok(InstanceRole::Primary(lease)) => run_primary(lease, command, events),
        Ok(InstanceRole::Secondary(contender)) => {
            events.emit(&BrokerEvent::InstanceElected {
                role: InstanceRoleName::Secondary,
            });
            run_secondary(contender, command, events)
        }
        Err(_) => {
            eprintln!("role=unknown accepted=false error_code=instance-error");
            ExitCode::FAILURE
        }
    }
}

fn parse_arguments(arguments: Vec<OsString>) -> Result<Option<StartupCommand>, ()> {
    match arguments.as_slice() {
        [] => Ok(None),
        [flag] if flag == OsStr::new("--close") => Ok(Some(StartupCommand::Close)),
        [flag, path] if flag == OsStr::new("--open") && !path.is_empty() => {
            Ok(Some(StartupCommand::Open(path.clone())))
        }
        _ => Err(()),
    }
}

fn control_request(command: StartupCommand) -> BrokerControlRequest {
    let command = match command {
        StartupCommand::Open(path) => broker_control_request::Command::OpenPath(OpenPath {
            path_utf16le: path
                .as_os_str()
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect(),
        }),
        StartupCommand::Close => broker_control_request::Command::ClosePreview(ClosePreview {}),
    };
    BrokerControlRequest {
        protocol_major: PROTOCOL_MAJOR,
        protocol_minor: PROTOCOL_MINOR,
        command_id: Uuid::new_v4().to_string(),
        command: Some(command),
    }
}

fn product_id() -> String {
    if cfg!(debug_assertions)
        && let Some(suffix) = std::env::var_os(PRODUCT_SUFFIX_ENV)
        && !suffix.is_empty()
    {
        return format!("{PRODUCT_ID}.Test.{}", suffix.to_string_lossy());
    }
    PRODUCT_ID.to_owned()
}

fn run_primary(
    lease: InstanceLease,
    startup_command: Option<BrokerControlRequest>,
    events: Arc<dyn EventSink>,
) -> ExitCode {
    let server = match BrokerCommandServer::create_with_event_sink(
        lease.pipe_name(),
        STARTUP_TIMEOUT,
        IO_TIMEOUT,
        events.clone(),
    ) {
        Ok(server) => server,
        Err(error) => {
            print_error("primary", error.code());
            return ExitCode::FAILURE;
        }
    };
    let mut runtime = BrokerRuntime::new(lease, server, events.clone());

    if let Some(request) = startup_command {
        let ack = match BrokerControlContract::decode_request(request) {
            Ok(command) => runtime.handle(command),
            Err(rejection) => {
                events.emit(&BrokerEvent::CommandRejected {
                    command_id: rejection.safe_command_id().map(ToOwned::to_owned),
                    reason: rejection.code().as_str(),
                });
                rejection.into_ack()
            }
        };
        print_ack("primary", &ack);
    } else {
        print_ready_primary();
    }

    if let Err(error) = runtime.run() {
        print_error("primary", error.code());
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_secondary(
    mut contender: previewit_broker::InstanceContender,
    command: Option<BrokerControlRequest>,
    events: Arc<dyn EventSink>,
) -> ExitCode {
    let Some(command) = command else {
        print_simple_ack("secondary", true, "");
        return ExitCode::SUCCESS;
    };

    match BrokerCommandClient::send(contender.pipe_name(), &command, STARTUP_TIMEOUT, IO_TIMEOUT) {
        Ok(ack) => {
            print_ack("secondary", &ack);
            accepted_exit(&ack)
        }
        Err(BrokerCommandError::PrimaryNotReady) => match contender.try_take_over() {
            Ok(Some(lease)) => run_primary(lease, Some(command), events),
            Ok(None) => {
                print_error("secondary", "primary-not-ready");
                ExitCode::FAILURE
            }
            Err(_) => {
                print_error("secondary", "instance-error");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            print_error("secondary", error.code());
            ExitCode::FAILURE
        }
    }
}

fn accepted_exit(ack: &CommandAck) -> ExitCode {
    if matches!(
        ack,
        CommandAck::OpenAccepted { .. } | CommandAck::CloseAccepted { .. }
    ) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_ack(role: &str, ack: &CommandAck) {
    let response = BrokerControlContract::encode_response(ack);
    print_response(role, &response);
}

fn print_response(role: &str, response: &BrokerControlResponse) {
    println!(
        "role={role} accepted={} command_id={} request_id={} error_code={}",
        response.accepted, response.command_id, response.request_id, response.error_code
    );
    flush_stdout();
}

fn print_ready_primary() {
    print_simple_ack("primary", true, "");
}

fn print_simple_ack(role: &str, accepted: bool, error_code: &str) {
    println!("role={role} accepted={accepted} command_id= request_id= error_code={error_code}");
    flush_stdout();
}

fn print_error(role: &str, error_code: &str) {
    print_simple_ack(role, false, error_code);
}

fn flush_stdout() {
    let _ = io::stdout().flush();
}
