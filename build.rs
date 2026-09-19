//! 构建脚本：为 Windows 程序嵌入管理员权限清单。
//!
//! 三角洲行动以管理员权限运行（ACE 反作弊），普通权限进程的 `SendInput`
//! 会被 UIPI 隔离而无法注入鼠标事件；因此本清单请求以管理员身份运行。

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
    </application>
  </compatibility>
</assembly>"#;

    let mut res = winres::WindowsResource::new();
    res.set_manifest(manifest);
    res.set("FileDescription", "三角洲行动 · 自动钓鱼");
    res.set("ProductName", "DeltaFishing");
    if let Err(e) = res.compile() {
        println!("cargo:warning=嵌入管理员清单失败: {e}");
    }
}