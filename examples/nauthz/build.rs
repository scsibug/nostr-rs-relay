fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .build_server(false)
        .protoc_arg("--experimental_allow_proto3_optional")
        .compile(&["../../proto/nauthz.proto"], &["../../proto"])?;
    Ok(())
}
