fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("../../proto/vmm.proto")?;
    tonic_prost_build::compile_protos("../../proto/agent.proto")?;
    Ok(())
}
