//! GitHub 自动更新与在线版本检测模块。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub const GITHUB_REPO: &str = "https://github.com/olwater/DeltaFishing";
#[allow(dead_code)]
pub const GITHUB_RELEASES_URL: &str = "https://github.com/olwater/DeltaFishing/releases";
pub const GITHUB_API_LATEST: &str = "https://api.github.com/repos/olwater/DeltaFishing/releases/latest";

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub size: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GithubRelease {
    pub tag_name: String,
    pub html_url: String,
    pub body: Option<String>,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub has_update: bool,
    pub release_url: String,
    pub download_url: Option<String>,
    pub changelog: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available(UpdateInfo),
    Downloading { progress: f32 },
    ReadyToRestart,
    Failed(String),
}

/// 语义化版本号比较：latest 是否比 current 新。
pub fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u32> {
        v.trim()
            .trim_start_matches(|c: char| c == 'v' || c == 'V')
            .split('.')
            .filter_map(|s| s.parse::<u32>().ok())
            .collect()
    };
    let l_parts = parse(latest);
    let c_parts = parse(current);
    for (l, c) in l_parts.iter().zip(c_parts.iter()) {
        if l > c {
            return true;
        }
        if l < c {
            return false;
        }
    }
    l_parts.len() > c_parts.len()
}

/// 同步向 GitHub 请求最新 Release 信息。
pub fn check_update_sync() -> Result<UpdateInfo, String> {
    let current_version = env!("CARGO_PKG_VERSION").to_string();
    let config = ureq::config::Config::builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build();
    let agent = ureq::Agent::new_with_config(config);
    let resp = agent
        .get(GITHUB_API_LATEST)
        .header("User-Agent", "DeltaFishing-Client")
        .header("Accept", "application/vnd.github.v3+json")
        .call()
        .map_err(|e| format!("连接 GitHub 失败: {e}"))?;

    let release: GithubRelease = resp
        .into_body()
        .read_json()
        .map_err(|e| format!("解析版本信息失败: {e}"))?;

    let latest_ver = release
        .tag_name
        .trim_start_matches(|c| c == 'v' || c == 'V')
        .to_string();
    let has_update = is_newer_version(&latest_ver, &current_version);

    // 寻找 assets 中的可执行文件（优先带 deltafishing 或以 .exe 结尾）
    let download_url = release
        .assets
        .iter()
        .find(|a| a.name.to_lowercase().ends_with(".exe"))
        .map(|a| a.browser_download_url.clone());

    Ok(UpdateInfo {
        current_version,
        latest_version: latest_ver,
        has_update,
        release_url: release.html_url,
        download_url,
        changelog: release.body.unwrap_or_default(),
    })
}

/// 下载最新版本并执行 Windows 原地热替换。
pub fn download_and_replace_sync<F>(download_url: &str, on_progress: F) -> Result<PathBuf, String>
where
    F: Fn(f32),
{
    let current_exe = std::env::current_exe().map_err(|e| format!("获取自身路径失败: {e}"))?;
    let target_dir = current_exe
        .parent()
        .ok_or_else(|| "无法获取可执行文件目录".to_string())?;

    let new_exe = target_dir.join("deltafishing_new.tmp");
    let old_backup = current_exe.with_extension("exe.old");

    let config = ureq::config::Config::builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    let resp = agent
        .get(download_url)
        .header("User-Agent", "DeltaFishing-Client")
        .call()
        .map_err(|e| format!("下载更新失败: {e}"))?;

    let total_len = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = resp.into_body().into_reader();
    let mut file = std::fs::File::create(&new_exe).map_err(|e| format!("创建临时更新文件失败: {e}"))?;

    let mut downloaded: u64 = 0;
    let mut buf = [0u8; 16384];

    loop {
        use std::io::{Read, Write};
        let bytes_read = reader.read(&mut buf).map_err(|e| format!("读取下载流失败: {e}"))?;
        if bytes_read == 0 {
            break;
        }
        file.write_all(&buf[..bytes_read])
            .map_err(|e| format!("写入文件失败: {e}"))?;
        downloaded += bytes_read as u64;
        if total_len > 0 {
            let progress = (downloaded as f32 / total_len as f32).clamp(0.0, 1.0);
            on_progress(progress);
        }
    }

    // 清理旧的备份
    if old_backup.exists() {
        let _ = std::fs::remove_file(&old_backup);
    }

    // Windows 热替换原子操作：当前 exe 重命名为 .old，new 重命名为当前 exe
    std::fs::rename(&current_exe, &old_backup)
        .map_err(|e| format!("重命名原文件失败: {e}"))?;

    if let Err(e) = std::fs::rename(&new_exe, &current_exe) {
        // 回滚
        let _ = std::fs::rename(&old_backup, &current_exe);
        return Err(format!("替换新文件失败: {e}"));
    }

    Ok(current_exe)
}

/// 重启自身并退出当前进程。
pub fn restart_app(current_exe: &PathBuf) -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new(current_exe)
            .spawn()
            .map_err(|e| format!("重启应用失败: {e}"))?;
        std::process::exit(0);
    }
    #[cfg(not(windows))]
    {
        let _ = current_exe;
        Ok(())
    }
}

/// 自动清理之前更新产生的 .old 历史残留文件。
pub fn clean_old_files() {
    if let Ok(current_exe) = std::env::current_exe() {
        let old_backup = current_exe.with_extension("exe.old");
        if old_backup.exists() {
            let _ = std::fs::remove_file(old_backup);
        }
        let tmp_file = current_exe.parent().map(|p| p.join("deltafishing_new.tmp"));
        if let Some(tmp) = tmp_file {
            if tmp.exists() {
                let _ = std::fs::remove_file(tmp);
            }
        }
    }
}

/// 异步更新状态管理器（供 UI 绑定）。
#[derive(Clone)]
pub struct UpdaterHandle {
    pub status: Arc<Mutex<UpdateStatus>>,
}

impl Default for UpdaterHandle {
    fn default() -> Self {
        Self {
            status: Arc::new(Mutex::new(UpdateStatus::Idle)),
        }
    }
}

impl UpdaterHandle {
    pub fn get_status(&self) -> UpdateStatus {
        self.status.lock().unwrap().clone()
    }

    /// 后台异步检查更新。
    pub fn check_async(&self) {
        let status = self.status.clone();
        {
            let mut s = status.lock().unwrap();
            *s = UpdateStatus::Checking;
        }
        thread::Builder::new()
            .name("update-checker".into())
            .spawn(move || {
                match check_update_sync() {
                    Ok(info) => {
                        let mut s = status.lock().unwrap();
                        if info.has_update {
                            *s = UpdateStatus::Available(info);
                        } else {
                            *s = UpdateStatus::UpToDate;
                        }
                    }
                    Err(e) => {
                        let mut s = status.lock().unwrap();
                        *s = UpdateStatus::Failed(e);
                    }
                }
            })
            .expect("无法创建更新检测线程");
    }

    /// 后台异步下载并执行自动替换更新。
    pub fn download_and_install_async(&self, download_url: String) {
        let status = self.status.clone();
        {
            let mut s = status.lock().unwrap();
            *s = UpdateStatus::Downloading { progress: 0.0 };
        }
        thread::Builder::new()
            .name("update-installer".into())
            .spawn(move || {
                let status_cb = status.clone();
                let res = download_and_replace_sync(&download_url, move |p| {
                    if let Ok(mut s) = status_cb.lock() {
                        *s = UpdateStatus::Downloading { progress: p };
                    }
                });
                match res {
                    Ok(exe_path) => {
                        if let Ok(mut s) = status.lock() {
                            *s = UpdateStatus::ReadyToRestart;
                        }
                        // 稍候 500ms 重启启动新版本
                        thread::sleep(Duration::from_millis(500));
                        let _ = restart_app(&exe_path);
                    }
                    Err(e) => {
                        if let Ok(mut s) = status.lock() {
                            *s = UpdateStatus::Failed(e);
                        }
                    }
                }
            })
            .expect("无法创建安装更新线程");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer_version("0.1.2", "0.1.1"));
        assert!(is_newer_version("v1.0.0", "0.1.1"));
        assert!(is_newer_version("0.2.0", "0.1.9"));
        assert!(!is_newer_version("0.1.1", "0.1.1"));
        assert!(!is_newer_version("0.1.0", "0.1.1"));
    }
}
