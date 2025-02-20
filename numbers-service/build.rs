fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("../proto/numbers/v1/numbers_service.proto")?;
    Ok(())
}
