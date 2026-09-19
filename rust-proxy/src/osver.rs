//! 版本闸（方案 §5.7 / §7.5 第 1 条、Phase 2 范围项 ⑧）：`RtlGetVersion` 判 OS。
//!
//! ⚠ **绝不用 `GetVersionEx`** —— 它会被应用兼容性垫片（AppCompat shim）欺骗，报出假版本
//! （指南 §5.3 与方案 §7.5 写死）。`RtlGetVersion` 是唯一可靠的运行期判据。
//!
//! **Win7 静态导入面纪律**：`RtlGetVersion` 经 `GetProcAddress(ntdll.dll)` **动态取** ——
//! 本模块不产生任何新的静态导入（Analyzer 只看静态导入表；动态查找不在其判据内，指南 §4.1 边界）。
//!
//! 版本闸的唯一实际分支 = **深色标题栏**（`DWMWA_USE_IMMERSIVE_DARK_MODE`，Win10 1809/build 17763 起）：
//! Win7/8/8.1 与更早的 Win10 一律跳过（§5.2「可选增强：失败即忽略、不留半截效果」）。
//! GDI+ 面只用了 XP 起就稳定的 flat API（`GdipSetSmoothingMode` / 路径 / 填充）⇒ 无需按 OS 分支，
//! 报告里已如实登记这条判断。

/// 运行期 OS 版本（只取判定需要的三个分量）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsVersion {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
}

impl OsVersion {
    /// 纯分类入口（可单测；`current()` 的真实读数走同一函数）。
    pub fn from_raw(major: u32, minor: u32, build: u32) -> OsVersion {
        OsVersion {
            major,
            minor,
            build,
        }
    }

    /// 展示名（环境面板 / 关于对话框用）。
    pub fn name(&self) -> &'static str {
        match (self.major, self.minor) {
            (6, 0) => "Windows Vista",
            (6, 1) => "Windows 7",
            (6, 2) => "Windows 8",
            (6, 3) => "Windows 8.1",
            (10, _) if self.build >= 22000 => "Windows 11",
            (10, _) => "Windows 10",
            _ => "Windows",
        }
    }

    /// `MAJOR.MINOR.BUILD` 展示串。
    pub fn display(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.build)
    }

    /// Win7 SP1 判定（本项目的 OS 底线）。
    pub fn is_windows7(&self) -> bool {
        self.major == 6 && self.minor == 1
    }

    /// 深色标题栏（`DWMWA_USE_IMMERSIVE_DARK_MODE = 20`）：Win10 1809（build 17763）起才有效。
    /// 更早系统上一律**不调用**（Win7/8/8.1 与 17763 之前的 Win10 ⇒ `false`）。
    pub fn supports_dark_title_bar(&self) -> bool {
        self.major >= 10 && self.build >= 17763
    }

    /// 是否支持 Per-Monitor V2 清单项（Win10 1703+）。**只用于日志/关于展示** ——
    /// 感知模式由清单声明决定（Win7 忽略未知项、退到 `dpiAware=true/pm`），不靠运行期 API 设置。
    pub fn supports_per_monitor_v2(&self) -> bool {
        self.major >= 10 && self.build >= 15063
    }
}

/// 取当前 OS 版本；`RtlGetVersion` 不可用（非 Windows / ntdll 异常）⇒ `None`（调用方按"未知"降级）。
#[cfg(windows)]
pub fn current() -> Option<OsVersion> {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;

    unsafe {
        let module = GetModuleHandleW(w!("ntdll.dll")).ok()?;
        let proc = GetProcAddress(module, windows::core::s!("RtlGetVersion"))?;
        // 签名由 ntdll 的文档化导出确定：NTSTATUS RtlGetVersion(PRTL_OSVERSIONINFOW)
        let func: unsafe extern "system" fn(*mut OSVERSIONINFOW) -> i32 = std::mem::transmute(proc);
        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
            ..Default::default()
        };
        // NTSTATUS：0 = STATUS_SUCCESS；非 0 一律按"取不到"处理（不猜）
        if func(&mut info) != 0 {
            return None;
        }
        Some(OsVersion::from_raw(
            info.dwMajorVersion,
            info.dwMinorVersion,
            info.dwBuildNumber,
        ))
    }
}

/// 非 Windows：本子项目只为 Windows 构建（方案 §5.4），这里不写"假版本"占位。
#[cfg(not(windows))]
pub fn current() -> Option<OsVersion> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_gates_are_classified_by_build() {
        let win7 = OsVersion::from_raw(6, 1, 7601);
        assert_eq!(win7.name(), "Windows 7");
        assert!(win7.is_windows7());
        assert!(!win7.supports_dark_title_bar());
        assert!(!win7.supports_per_monitor_v2());

        let win81 = OsVersion::from_raw(6, 3, 9600);
        assert_eq!(win81.name(), "Windows 8.1");
        assert!(!win81.supports_dark_title_bar());

        // 深色标题栏的界线 = Win10 1809（17763）：17762 不行、17763 可以
        let win10_1803 = OsVersion::from_raw(10, 0, 17134);
        assert_eq!(win10_1803.name(), "Windows 10");
        assert!(!win10_1803.supports_dark_title_bar());
        assert!(win10_1803.supports_per_monitor_v2());

        let win10_1809 = OsVersion::from_raw(10, 0, 17763);
        assert!(win10_1809.supports_dark_title_bar());

        let win11 = OsVersion::from_raw(10, 0, 22631);
        assert_eq!(win11.name(), "Windows 11");
        assert!(win11.supports_dark_title_bar());
    }

    #[test]
    fn current_reports_a_real_version_on_windows() {
        let version = current().expect("Windows 上 RtlGetVersion 必须可用");
        // 本项目只支持 Win7+：读到的版本必须 ≥ 6.1（防"垫片假版本"式退化）
        assert!(
            version.major > 6 || (version.major == 6 && version.minor >= 1),
            "读到 {}.{}.{}，低于 Win7 底线",
            version.major,
            version.minor,
            version.build
        );
        assert!(!version.display().is_empty());
    }
}
