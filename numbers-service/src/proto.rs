pub mod numbers {
    tonic::include_proto!("numbers.v1");
}
pub use numbers::*;
pub(crate) const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("numbers.v1");
