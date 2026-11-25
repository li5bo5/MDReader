//! 构建脚本：在 MSVC 链接阶段直接嵌入 Windows 11 ComCtl32 v6 视觉样式、PerMonitorV2 DPI 清单及主程序图标资源。

use std::path::{Path, PathBuf};
use std::process::Command;

fn clean_unc_prefix(path: &Path) -> String {
    let raw = path.to_string_lossy();
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
}

fn target_sdk_arch_dir() -> &'static str {
    match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => "x64",
    }
}

fn find_rc_exe() -> PathBuf {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return PathBuf::from("rc.exe");
    }

    let arch_dir = target_sdk_arch_dir();
    let kits_bin = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    if let Ok(entries) = std::fs::read_dir(kits_bin) {
        let mut versions: Vec<(Vec<u32>, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if !p.is_dir() {
                    return None;
                }
                let candidate = p.join(arch_dir).join("rc.exe");
                if !candidate.is_file() {
                    return None;
                }
                let ver_nums: Vec<u32> = e
                    .file_name()
                    .to_string_lossy()
                    .split('.')
                    .filter_map(|part| part.parse::<u32>().ok())
                    .collect();
                Some((ver_nums, candidate))
            })
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        if let Some((_, best)) = versions.into_iter().next() {
            return best;
        }
    }

    panic!("未找到 Windows SDK rc.exe（无法编译嵌入应用程序图标资源）");
}

fn main() {
    println!("cargo:rerun-if-changed=assets/mdreader.manifest");
    println!("cargo:rerun-if-changed=assets/mdreader.ico");

    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest_path = Path::new("assets/mdreader.manifest")
            .canonicalize()
            .expect("缺少 assets/mdreader.manifest");
        let clean_manifest = clean_unc_prefix(&manifest_path);
        println!("cargo:rustc-link-arg-bin=MDReader=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bin=MDReader=/MANIFESTINPUT:{clean_manifest}");

        let ico_path = Path::new("assets/mdreader.ico")
            .canonicalize()
            .expect("缺少 assets/mdreader.ico");
        let clean_ico = clean_unc_prefix(&ico_path).replace('\\', r"\\");

        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("缺少 OUT_DIR"));
        let rc_file = out_dir.join("mdreader_icon.rc");
        let res_file = out_dir.join("mdreader_icon.res");

        std::fs::write(&rc_file, format!("1 ICON \"{clean_ico}\"\r\n"))
            .expect("写入 mdreader_icon.rc 失败");

        let rc_exe = find_rc_exe();
        let status = Command::new(&rc_exe)
            .arg("/nologo")
            .arg("/fo")
            .arg(&res_file)
            .arg(&rc_file)
            .status()
            .unwrap_or_else(|e| panic!("执行 {} 失败: {e}", rc_exe.display()));
        assert!(status.success(), "rc.exe 编译图标资源失败，退出码: {status}");

        let clean_res = clean_unc_prefix(&res_file);
        println!("cargo:rustc-link-arg-bin=MDReader={clean_res}");
    }
}
