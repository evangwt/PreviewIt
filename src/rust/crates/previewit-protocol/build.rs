use std::{error::Error, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..");
    let protocol_root = repository_root.join("protocol");
    let schema = protocol_root.join("preview/v0/preview.proto");

    println!("cargo:rerun-if-changed={}", schema.display());

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    config.compile_protos(&[schema], &[protocol_root])?;

    Ok(())
}
