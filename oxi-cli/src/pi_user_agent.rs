//! User agent string for oxi HTTP requests

/// Generate a user agent string identifying oxi
pub fn get_user_agent(version: &str) -> String {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    };
    
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        "unknown"
    };
    
    format!("oxi/{} ({}; {})", version, os, arch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_agent_format() {
        let ua = get_user_agent("0.3.1");
        assert!(ua.starts_with("oxi/0.3.1"));
        assert!(ua.contains("(macos") || ua.contains("(linux") || ua.contains("(windows"));
    }
}
