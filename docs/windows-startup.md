# Windows 启动

## 开发者启动

在仓库根目录双击 `Start Bridge.cmd`。脚本会从自身所在目录启动，检查 Node/npm、Rust/Cargo、Codex CLI 和 `node_modules`；首次缺少依赖时会执行 `npm ci`，随后运行 `npm run tauri dev`。

仅检查环境而不启动应用：

```text
.\Start Bridge.cmd --check
```

若未检测到 Codex，脚本会给出警告但仍允许启动桌面应用，以便查看已有会话和状态。实际运行 Codex 回合前仍需要安装并登录 Codex CLI。

手工验证：双击脚本后确认 Tauri 开发窗口出现；关闭开发控制台会停止开发应用。

## Windows 安装包

执行：

```text
npm run tauri build
```

Tauri 会在 `src-tauri/target/release/bundle/` 下生成 Windows 安装包。安装包用户从 Windows 开始菜单启动应用，不需要 Node、npm、Rust 或 Cargo。

安装后的产品环境仍需要 Chrome、ChatGPT Chrome 扩展，以及已安装并认证的 Codex CLI。
