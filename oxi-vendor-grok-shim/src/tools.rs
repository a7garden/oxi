//! Tools util shims (xai_grok_tools::util).
pub mod util {
    pub mod image_validate {
        pub fn validate_image_bytes_unrestricted(_bytes: &[u8], _allow_svg: bool) -> Result<(), String> {
            Ok(())
        }
    }
    pub fn detach_std_command(_cmd: &mut std::process::Command) {}
}
