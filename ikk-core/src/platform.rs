use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub os:   Os,
    pub arch: Arch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    MacOs,
    Linux,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    Aarch64,
    X86_64,
}

impl Platform {
    pub fn current() -> Self {
        Self {
            os:   Os::current(),
            arch: Arch::current(),
        }
    }
}

impl Os {
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]   { Os::MacOs }
        #[cfg(target_os = "linux")]   { Os::Linux }
        #[cfg(target_os = "windows")] { Os::Windows }
    }

    /// All known asset name variants for this OS — checked in order
    pub fn variants(&self) -> &[&str] {
        match self {
            Os::MacOs   => &["darwin", "macos", "apple-darwin", "osx"],
            Os::Linux   => &["linux", "linux-gnu", "linux-musl", "unknown-linux"],
            Os::Windows => &["windows", "win32", "win64", "pc-windows"],
        }
    }
}

impl Arch {
    pub fn current() -> Self {
        #[cfg(target_arch = "aarch64")] { Arch::Aarch64 }
        #[cfg(target_arch = "x86_64")]  { Arch::X86_64 }
    }

    /// All known asset name variants — checked in order
    pub fn variants(&self) -> &[&str] {
        match self {
            Arch::Aarch64 => &["aarch64", "arm64", "armv8"],
            Arch::X86_64  => &["x86_64", "amd64", "x64"],
        }
    }
}

/// Score an asset name against the current platform.
/// Higher score = better match. None = not a match.
pub fn score_asset(name: &str, platform: &Platform) -> Option<u32> {
    let name = name.to_lowercase();

    // must match both arch and os
    let arch_match = platform.arch.variants().iter()
        .enumerate()
        .find(|(_, v)| name.contains(*v))
        .map(|(i, _)| (platform.arch.variants().len() - i) as u32)?;

    let os_match = platform.os.variants().iter()
        .enumerate()
        .find(|(_, v)| name.contains(*v))
        .map(|(i, _)| (platform.os.variants().len() - i) as u32)?;

    // prefer native archives over generic ones
    let archive_bonus: u32 = if name.ends_with(".tar.gz") || name.ends_with(".tar.xz") {
        2
    } else if name.ends_with(".zip") {
        1
    } else if name.ends_with(".exe") || name.ends_with(".dmg") || name.ends_with(".pkg") {
        0
    } else {
        1  // raw binary
    };

    Some(arch_match + os_match + archive_bonus)
}
